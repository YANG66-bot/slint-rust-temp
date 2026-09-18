//! 文件音频源：解码文件 → 单声道混缩 → 线性重采样 → 写入环形缓冲。
//!
//! 设计要点：
//! - 统一输出采样率为 [`TARGET_SAMPLE_RATE`]（与测试信号一致），
//!   分析线程的频段映射按此构建，切源时无需重建流水线
//! - 暂停通过共享 [`AtomicBool`](std::sync::atomic::AtomicBool) 实现，
//!   由 `Player` 控制（跨源切换后状态保持）
//! - 播放结束（EOF）时置位 finished 标志，UI 侧据此更新状态

use crate::audio::buffer::PcmRingBuffer;
use crate::audio::resample::{mixdown, resample};
use crate::audio::source::AudioSource;
use crate::error::AppResult;
use crate::player::decoder::AudioDecoder;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// 统一的输出采样率（Hz）。
///
/// 与 `audio::source::TEST_SIGNAL_SAMPLE_RATE` 保持一致，
/// 使频段映射在整个应用生命周期内固定。
pub const TARGET_SAMPLE_RATE: u32 = 44100;

/// 解码线程的暂停轮询间隔。
const PAUSE_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 文件播放音频源。
pub struct FileAudioSource {
    ring: Arc<PcmRingBuffer>,
    path: PathBuf,
    /// 源文件采样率（用于状态显示）。
    source_sample_rate: u32,
    source_channels: u16,
    stop: Arc<AtomicBool>,
    /// 共享暂停标志（与 `Player` 共用同一实例）。
    pause: Arc<AtomicBool>,
    /// 播放结束标志（EOF 或出错）。
    finished: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FileAudioSource {
    /// 创建并校验文件可解码（立即返回错误，避免切换源失败后才报错）。
    pub fn new(ring: Arc<PcmRingBuffer>, path: PathBuf, pause: Arc<AtomicBool>) -> AppResult<Self> {
        // 预打开解码器校验格式，拿到源格式信息后丢弃（线程内重新打开）
        let probe = AudioDecoder::open(&path)?;
        let source_sample_rate = probe.sample_rate();
        let source_channels = probe.channels();

        Ok(Self {
            ring,
            path,
            source_sample_rate,
            source_channels,
            stop: Arc::new(AtomicBool::new(false)),
            pause,
            finished: Arc::new(AtomicBool::new(false)),
            handle: None,
        })
    }

    /// 是否已播放完毕（EOF）。
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// 播放结束标志的共享句柄（供 `Player` 做 EOF 检测）。
    pub fn finished_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.finished)
    }
}

impl AudioSource for FileAudioSource {
    fn start(&mut self) -> AppResult<()> {
        if self.handle.is_some() {
            return Ok(());
        }
        self.stop.store(false, Ordering::Release);
        self.finished.store(false, Ordering::Release);
        let ring = Arc::clone(&self.ring);
        let stop = Arc::clone(&self.stop);
        let pause = Arc::clone(&self.pause);
        let finished = Arc::clone(&self.finished);
        let path = self.path.clone();
        let handle = std::thread::Builder::new()
            .name("file-playback".to_string())
            .spawn(move || {
                run_file_playback(ring, stop, pause, finished, path);
            })
            .map_err(|e| {
                crate::error::AppError::AudioStreamError(format!("启动播放线程失败: {e}"))
            })?;
        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) -> AppResult<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        TARGET_SAMPLE_RATE
    }

    fn channels(&self) -> u16 {
        1
    }

    fn describe(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string());
        format!(
            "文件播放 {name}（{}Hz / {}ch → {TARGET_SAMPLE_RATE}Hz 单声道）",
            self.source_sample_rate, self.source_channels
        )
    }
}

impl Drop for FileAudioSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// 播放线程主循环：解码 → 混缩 → 重采样 → 写环形缓冲。
fn run_file_playback(
    ring: Arc<PcmRingBuffer>,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    path: PathBuf,
) {
    let mut decoder = match AudioDecoder::open(&path) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("打开 {} 失败: {e}", path.display());
            finished.store(true, Ordering::Release);
            return;
        }
    };
    let src_rate = f64::from(decoder.sample_rate().max(1));
    let dst_rate = f64::from(TARGET_SAMPLE_RATE);
    tracing::debug!(
        "播放线程启动: {} ({}Hz → {TARGET_SAMPLE_RATE}Hz)",
        path.display(),
        src_rate
    );

    let mut packet: Vec<f32> = Vec::with_capacity(8192);
    let mut mono: Vec<f32> = Vec::with_capacity(8192);
    let mut resampled: Vec<f32> = Vec::with_capacity(8192);
    // 重采样浮点游标（相对当前 packet 的样本位置）
    let mut cursor: f64 = 0.0;

    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        if pause.load(Ordering::Acquire) {
            std::thread::sleep(PAUSE_POLL_INTERVAL);
            continue;
        }
        match decoder.next_packet(&mut packet) {
            Ok(true) => {
                mixdown(&packet, decoder.channels(), &mut mono);
                resample(&mono, src_rate / dst_rate, &mut cursor, &mut resampled);
                if !resampled.is_empty() {
                    ring.write(&resampled);
                }
            }
            Ok(false) => {
                tracing::debug!("播放结束: {}", path.display());
                break;
            }
            Err(e) => {
                tracing::error!("解码错误，停止播放 {}: {e}", path.display());
                break;
            }
        }
    }
    finished.store(true, Ordering::Release);
    tracing::debug!("播放线程退出");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_thread_processes_decoded_packets() {
        // 0.05 秒 WAV：验证 run_file_playback 链路（解码 → 混缩 → 重采样 → 写入）
        let dir = std::env::temp_dir().join("lumawave_player_source_test");
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let path = dir.join("short_tone.wav");
        let samples: Vec<f32> = (0..2205)
            .map(|i| (std::f32::consts::TAU * 880.0 * i as f32 / 44100.0).sin())
            .collect();
        let mut wav = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for &s in &samples {
            wav.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(&path, wav).expect("写入 WAV");

        let ring = Arc::new(PcmRingBuffer::new(16384));
        let stop = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        run_file_playback(
            Arc::clone(&ring),
            Arc::clone(&stop),
            pause,
            Arc::clone(&finished),
            path.clone(),
        );
        assert!(finished.load(Ordering::Acquire), "短文件应自然结束");
        assert!(ring.available() > 0, "应已写入 PCM 数据");
    }
}
