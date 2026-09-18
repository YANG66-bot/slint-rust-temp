//! 系统音频环回采集（WASAPI Loopback，Windows 专属实现）。
//!
//! 在默认渲染设备（扬声器）上以共享模式 + `AUDCLNT_STREAMFLAGS_LOOPBACK`
//! 打开采集客户端，捕获本机所有正在播放的音频（音乐播放器、浏览器、
//! 游戏等），混缩为单声道并重采样到 44.1kHz 后写入环形缓冲。
//! cpal 不暴露环回能力，这里直接调用 WASAPI COM 接口。
//!
//! COM 只在专用线程内初始化（预检线程 / 采集线程），接口对象不跨线程；
//! 采集循环以 8ms 轮询排空数据包，停止由 [`AudioSource::stop`] 置位标志。

use crate::audio::buffer::PcmRingBuffer;
use crate::audio::resample::{mixdown, resample};
use crate::audio::source::{AudioSource, TEST_SIGNAL_SAMPLE_RATE};
use crate::error::{AppError, AppResult};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};

/// 环回缓冲时长（100ns 单位）：2 秒，容忍短暂卡顿期间的数据积压。
const LOOPBACK_BUFFER_100NS: i64 = 20_000_000;
/// 采集轮询间隔：远小于分析粒度，保证数据包不积压。
const POLL_INTERVAL: Duration = Duration::from_millis(8);

/// 系统音频环回源：捕获默认输出设备正在播放的全部声音。
pub struct SystemAudioSource {
    ring: Arc<PcmRingBuffer>,
    /// 设备混合格式（采样率，声道数），预检时读取
    mix_format: (u32, u16),
    info: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SystemAudioSource {
    /// 创建环回源（预检默认输出设备，失败立即报错以便上层回退）。
    pub fn new(ring: Arc<PcmRingBuffer>) -> AppResult<Self> {
        let mix_format = probe_mix_format()?;
        let (rate, channels) = mix_format;
        let info =
            format!("系统音频环回 {rate}Hz {channels}ch → {TEST_SIGNAL_SAMPLE_RATE}Hz 单声道");
        Ok(Self {
            ring,
            mix_format,
            info,
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        })
    }
}

impl AudioSource for SystemAudioSource {
    fn start(&mut self) -> AppResult<()> {
        self.stop.store(false, Ordering::Release);
        let ring = Arc::clone(&self.ring);
        let stop = Arc::clone(&self.stop);
        let (rate, channels) = self.mix_format;
        let handle = thread::Builder::new()
            .name("wasapi-loopback".into())
            .spawn(move || run_loopback(ring, stop, rate, channels))
            .map_err(|e| AppError::AudioStreamError(format!("启动环回采集线程失败: {e}")))?;
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
        // 与测试信号 / 文件源一致：分析线程的频段映射全生命周期固定
        TEST_SIGNAL_SAMPLE_RATE
    }

    fn channels(&self) -> u16 {
        1
    }

    fn describe(&self) -> String {
        self.info.clone()
    }
}

impl Drop for SystemAudioSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// 预检：在临时线程读取默认渲染设备的混合格式
///（COM 就地初始化、线程退出即随线程回收）。
fn probe_mix_format() -> AppResult<(u32, u16)> {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(probe_inner());
    });
    rx.recv()
        .map_err(|e| AppError::AudioStreamError(format!("环回预检线程失败: {e}")))?
}

fn probe_inner() -> AppResult<(u32, u16)> {
    unsafe {
        init_com()?;
        let client = open_render_client()?;
        let wfx = client
            .GetMixFormat()
            .map_err(|e| AppError::UnsupportedFormat(format!("读取混合格式失败: {e}")))?;
        let format = ((*wfx).nSamplesPerSec, (*wfx).nChannels);
        CoTaskMemFree(Some(wfx.cast()));
        Ok(format)
    }
}

/// COM 初始化（仅在专用线程内调用）。
unsafe fn init_com() -> AppResult<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| AppError::AudioStreamError(format!("COM 初始化失败: {e}")))
    }
}

/// 打开默认渲染设备的音频客户端（仅枚举与激活，不初始化流）。
unsafe fn open_render_client() -> AppResult<IAudioClient> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| AppError::AudioDeviceNotFound(format!("创建设备枚举器失败: {e}")))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| AppError::AudioDeviceNotFound(format!("没有默认输出设备: {e}")))?;
        device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| AppError::AudioStreamError(format!("激活音频客户端失败: {e}")))
    }
}

/// 采集线程入口。
fn run_loopback(ring: Arc<PcmRingBuffer>, stop: Arc<AtomicBool>, rate: u32, channels: u16) {
    if let Err(e) = capture_loopback(&ring, &stop, rate, channels) {
        tracing::error!("系统音频环回采集异常退出: {e}");
    }
    tracing::debug!("环回采集线程退出");
}

/// 环回采集主循环（阻塞至 stop 置位）。
fn capture_loopback(
    ring: &Arc<PcmRingBuffer>,
    stop: &AtomicBool,
    device_rate: u32,
    device_channels: u16,
) -> AppResult<()> {
    unsafe {
        init_com()?;
        let client = open_render_client()?;
        let wfx = client
            .GetMixFormat()
            .map_err(|e| AppError::UnsupportedFormat(format!("读取混合格式失败: {e}")))?;
        let bits = (*wfx).wBitsPerSample;
        // 共享模式混合格式按位深分派：32 位为引擎内部 float32，16 位为整型
        let float_samples = bits == 32;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                LOOPBACK_BUFFER_100NS,
                0,
                wfx,
                None,
            )
            .map_err(|e| AppError::AudioStreamError(format!("初始化环回流失败: {e}")))?;
        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| AppError::AudioStreamError(format!("获取采集客户端失败: {e}")))?;
        client
            .Start()
            .map_err(|e| AppError::AudioStreamError(format!("启动环回流失败: {e}")))?;
        tracing::info!("系统音频环回采集启动: {device_rate}Hz {device_channels}ch ({bits}bit)");

        let step = f64::from(device_rate) / f64::from(TEST_SIGNAL_SAMPLE_RATE);
        // 实时采集循环零分配：缓冲预分配并跨包复用
        let mut interleaved: Vec<f32> = Vec::with_capacity(8192);
        let mut mono: Vec<f32> = Vec::with_capacity(8192);
        let mut out: Vec<f32> = Vec::with_capacity(8192);
        let mut cursor: f64 = 0.0;

        while !stop.load(Ordering::Acquire) {
            thread::sleep(POLL_INTERVAL);
            // 排空当前全部数据包（每次 GetBuffer 后必须 ReleaseBuffer）
            loop {
                let mut frames = capture
                    .GetNextPacketSize()
                    .map_err(|e| AppError::AudioStreamError(format!("查询数据包失败: {e}")))?;
                if frames == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut flags = 0u32;
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| AppError::AudioStreamError(format!("读取数据包失败: {e}")))?;
                if !data.is_null() && flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 {
                    let sample_count = frames as usize * device_channels as usize;
                    if float_samples {
                        let floats = std::slice::from_raw_parts(data.cast::<f32>(), sample_count);
                        interleaved.clear();
                        interleaved.extend_from_slice(floats);
                    } else {
                        let ints = std::slice::from_raw_parts(data.cast::<i16>(), sample_count);
                        interleaved.clear();
                        interleaved.extend(ints.iter().map(|&s| f32::from(s) / 32768.0));
                    }
                    mixdown(&interleaved, device_channels, &mut mono);
                    resample(&mono, step, &mut cursor, &mut out);
                    if !out.is_empty() {
                        ring.write(&out);
                    }
                }
                capture
                    .ReleaseBuffer(frames)
                    .map_err(|e| AppError::AudioStreamError(format!("释放数据包失败: {e}")))?;
            }
        }

        let _ = client.Stop();
        CoTaskMemFree(Some(wfx.cast()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precheck_finds_default_output_device() {
        // 有默认输出设备的机器预检应成功（无音频设备的环境会失败）
        if let Err(e) = SystemAudioSource::new(Arc::new(PcmRingBuffer::new(8192))) {
            panic!("预检失败: {e}");
        }
    }

    #[test]
    fn loopback_start_stop_round_trip() {
        // 桌面静音时环回无数据包，仅验证启停路径不 panic；有播放时自然有数据
        let ring = Arc::new(PcmRingBuffer::new(8192));
        let mut source = SystemAudioSource::new(Arc::clone(&ring)).expect("预检失败");
        assert!(source.start().is_ok());
        thread::sleep(Duration::from_millis(150));
        assert!(source.stop().is_ok());
        assert_eq!(source.sample_rate(), TEST_SIGNAL_SAMPLE_RATE);
        assert_eq!(source.channels(), 1);
        assert!(!source.describe().is_empty());
    }
}
