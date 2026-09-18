//! 音频文件解码器（symphonia 封装）。
//!
//! 支持 WAV / MP3 / FLAC / OGG / Vorbis（由 Cargo features 决定）。
//! 解码输出为交错 f32（`[-1, 1]`），声道布局与源文件一致；
//! 单声道混缩与重采样由 [`crate::player::source::FileAudioSource`] 完成，
//! 保持 decoder 与音频路由职责分离。

use crate::error::{AppError, AppResult};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::{Channels, SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Duration as SymphoniaDuration;

/// 解码缓冲预分配帧数（覆盖常见编码器的单包上限）。
const PACKET_BUFFER_FRAMES: u64 = 8192;

/// 已打开的音频文件解码器。
pub struct AudioDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    /// 复用的样本缓冲（避免每包分配）。
    buffer: SampleBuffer<f32>,
}

impl AudioDecoder {
    /// 打开音频文件并定位第一条可用音频轨道。
    pub fn open(path: &Path) -> AppResult<Self> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| {
                AppError::DecoderError(format!("识别 {} 格式失败: {e}", path.display()))
            })?;

        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| {
                AppError::DecoderError(format!("{} 中未找到音频轨道", path.display()))
            })?;

        let track_id = track.id;
        let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
        // 保留原始声道布局用于 SignalSpec；计数用于状态显示
        let spec_channels = track
            .codec_params
            .channels
            .unwrap_or(Channels::FRONT_LEFT | Channels::FRONT_RIGHT);
        let channels = spec_channels.count() as u16;

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| AppError::DecoderError(format!("创建解码器失败: {e}")))?;

        let spec = SignalSpec::new(sample_rate, spec_channels);
        let buffer = SampleBuffer::<f32>::new(SymphoniaDuration::from(PACKET_BUFFER_FRAMES), spec);

        Ok(Self {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            buffer,
        })
    }

    /// 源文件采样率（Hz）。
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 源文件声道数。
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// 解码下一个数据包，把交错 f32 样本追加到 `out`。
    ///
    /// 返回 `Ok(false)` 表示文件播放完毕（EOF / 流结束）。
    /// 损坏的数据包被跳过（记录日志），不中断播放。
    pub fn next_packet(&mut self, out: &mut Vec<f32>) -> AppResult<bool> {
        out.clear();
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                // EOF（正常结束）与流重置都视为播放结束
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(false);
                }
                Err(SymphoniaError::ResetRequired) => return Ok(false),
                Err(e) => return Err(AppError::DecoderError(format!("读取数据包失败: {e}"))),
            };

            if packet.track_id() != self.track_id {
                continue; // 跳过非音频轨道（如封面）
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.buffer.copy_interleaved_ref(decoded);
                    out.extend_from_slice(self.buffer.samples());
                    return Ok(true);
                }
                Err(SymphoniaError::DecodeError(e)) => {
                    tracing::warn!("跳过损坏数据包: {e}");
                    continue;
                }
                Err(e) => {
                    return Err(AppError::DecoderError(format!("解码失败: {e}")));
                }
            }
        }
    }
}
