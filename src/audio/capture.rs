//! cpal 麦克风采集源。
//!
//! cpal 的 `Stream` 在部分平台不是 `Send`，因此采集流在专用线程内
//! 构建并持有（`Device` 同理不跨线程传递，线程内重新查找设备）；
//! 停止控制通过共享 `AtomicBool` 完成。设备与格式在
//! [`MicrophoneCapture::new`] 时预检，错误尽早暴露给调用方。

use crate::audio::buffer::PcmRingBuffer;
use crate::audio::resample::{mixdown, resample};
use crate::audio::source::{AudioSource, TEST_SIGNAL_SAMPLE_RATE};
use crate::error::{AppError, AppResult};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// 采集线程检查停止标志的轮询间隔。
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 麦克风采集源：cpal 输入流 → 单声道混缩 → 线性重采样 → 环形缓冲。
pub struct MicrophoneCapture {
    ring: Arc<PcmRingBuffer>,
    /// 设备名（`None` = 系统默认输入设备）。
    device_name: Option<String>,
    /// 预检到的设备信息（用于状态显示）。
    info: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MicrophoneCapture {
    /// 创建采集源并预检设备可用性。
    ///
    /// `device_name` 为 `None` 时使用系统默认输入设备；
    /// 指定名称但设备不存在时返回 [`AppError::AudioDeviceNotFound`]。
    pub fn new(ring: Arc<PcmRingBuffer>, device_name: Option<&str>) -> AppResult<Self> {
        let (found_name, config) = {
            let (name, device) = find_device(device_name)?;
            let config = default_input_config(&device)?;
            (name, config)
        };
        let info = format!(
            "麦克风 {found_name}（{}Hz / {}ch → {TEST_SIGNAL_SAMPLE_RATE}Hz 单声道）",
            config.sample_rate().0,
            config.channels()
        );
        Ok(Self {
            ring,
            device_name: device_name.map(|s| s.to_string()),
            info,
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        })
    }
}

// 手动实现：环形缓冲内部含 UnsafeCell，不便自动派生
impl std::fmt::Debug for MicrophoneCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrophoneCapture")
            .field("device_name", &self.device_name)
            .field("info", &self.info)
            .field("running", &self.handle.is_some())
            .finish()
    }
}

impl AudioSource for MicrophoneCapture {
    fn start(&mut self) -> AppResult<()> {
        if self.handle.is_some() {
            return Ok(());
        }
        self.stop.store(false, Ordering::Release);
        let ring = Arc::clone(&self.ring);
        let stop = Arc::clone(&self.stop);
        let device_name = self.device_name.clone();
        let handle = std::thread::Builder::new()
            .name("mic-capture".to_string())
            .spawn(move || run_capture(ring, stop, device_name))
            .map_err(|e| AppError::AudioStreamError(format!("启动采集线程失败: {e}")))?;
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

impl Drop for MicrophoneCapture {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// 按名称（或默认）查找输入设备。设备不跨线程传递：采集线程内重新查找。
fn find_device(device_name: Option<&str>) -> AppResult<(String, cpal::Device)> {
    let host = cpal::default_host();
    match device_name {
        Some(name) => {
            let devices = host
                .input_devices()
                .map_err(|e| AppError::AudioDeviceNotFound(format!("枚举输入设备失败: {e}")))?;
            for device in devices {
                if let Ok(n) = device.name()
                    && n == name
                {
                    return Ok((n, device));
                }
            }
            Err(AppError::AudioDeviceNotFound(format!(
                "找不到输入设备 {name}"
            )))
        }
        None => {
            let device = host
                .default_input_device()
                .ok_or_else(|| AppError::AudioDeviceNotFound("没有可用的输入设备".to_string()))?;
            let name = device.name().unwrap_or_else(|_| "默认设备".to_string());
            Ok((name, device))
        }
    }
}

/// 读取设备默认输入配置。
fn default_input_config(device: &cpal::Device) -> AppResult<cpal::SupportedStreamConfig> {
    device
        .default_input_config()
        .map_err(|e| AppError::UnsupportedFormat(format!("读取输入配置失败: {e}")))
}

/// 采集线程主循环：构建输入流 → 播放 → 等待停止信号。
fn run_capture(ring: Arc<PcmRingBuffer>, stop: Arc<AtomicBool>, device_name: Option<String>) {
    let result = capture_stream(&ring, &stop, device_name.as_deref());
    if let Err(e) = result {
        tracing::error!("麦克风采集异常退出: {e}");
    }
    tracing::debug!("采集线程退出");
}

/// 构建并驱动采集流（阻塞至 stop 置位）。
fn capture_stream(
    ring: &Arc<PcmRingBuffer>,
    stop: &AtomicBool,
    device_name: Option<&str>,
) -> AppResult<()> {
    let (name, device) = find_device(device_name)?;
    let config = default_input_config(&device)?;
    let channels = config.channels();
    let step = f64::from(config.sample_rate().0) / f64::from(TEST_SIGNAL_SAMPLE_RATE);
    let sample_format = config.sample_format();
    tracing::info!("麦克风采集启动: {name} ({sample_format:?})");
    // build_input_stream 接受基础 StreamConfig（SupportedStreamConfig 转换而来）
    let config: cpal::StreamConfig = config.into();

    let on_error = |e| tracing::error!("采集流错误: {e}");

    // 实时回调内不做分配：缓冲在闭包中预分配并跨调用复用
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let ring = Arc::clone(ring);
            let mut mono: Vec<f32> = Vec::with_capacity(4096);
            let mut out: Vec<f32> = Vec::with_capacity(4096);
            let mut cursor: f64 = 0.0;
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    mixdown(data, channels, &mut mono);
                    resample(&mono, step, &mut cursor, &mut out);
                    if !out.is_empty() {
                        ring.write(&out);
                    }
                },
                on_error,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let ring = Arc::clone(ring);
            let mut samples: Vec<f32> = Vec::with_capacity(4096);
            let mut mono: Vec<f32> = Vec::with_capacity(4096);
            let mut out: Vec<f32> = Vec::with_capacity(4096);
            let mut cursor: f64 = 0.0;
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    samples.clear();
                    samples.extend(data.iter().map(|&s| s as f32 / 32768.0));
                    mixdown(&samples, channels, &mut mono);
                    resample(&mono, step, &mut cursor, &mut out);
                    if !out.is_empty() {
                        ring.write(&out);
                    }
                },
                on_error,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let ring = Arc::clone(ring);
            let mut samples: Vec<f32> = Vec::with_capacity(4096);
            let mut mono: Vec<f32> = Vec::with_capacity(4096);
            let mut out: Vec<f32> = Vec::with_capacity(4096);
            let mut cursor: f64 = 0.0;
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    samples.clear();
                    samples.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                    mixdown(&samples, channels, &mut mono);
                    resample(&mono, step, &mut cursor, &mut out);
                    if !out.is_empty() {
                        ring.write(&out);
                    }
                },
                on_error,
                None,
            )
        }
        other => {
            return Err(AppError::UnsupportedFormat(format!(
                "不支持的输入采样格式: {other:?}"
            )));
        }
    }
    .map_err(|e| AppError::AudioStreamError(format!("创建采集流失败: {e}")))?;

    stream
        .play()
        .map_err(|e| AppError::AudioStreamError(format!("启动采集流失败: {e}")))?;

    // 阻塞等待停止信号（Stream 在本线程内 drop，规避平台 Send 限制）
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(STOP_POLL_INTERVAL);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_enumeration_does_not_panic() {
        // 无设备环境（CI）下迭代器为空也应正常返回
        let host = cpal::default_host();
        let count = host.input_devices().map(|d| d.count()).unwrap_or(0);
        eprintln!("输入设备数: {count}");
    }

    #[test]
    fn new_reports_missing_or_succeeds() {
        let ring = Arc::new(PcmRingBuffer::new(16384));
        match MicrophoneCapture::new(Arc::clone(&ring), None) {
            Ok(cap) => {
                // 有默认输入设备：描述应包含统一输出采样率
                assert!(cap.describe().contains("44100"), "{}", cap.describe());
            }
            Err(e) => {
                // 无设备环境：应返回设备缺失类错误
                assert!(matches!(e, AppError::AudioDeviceNotFound(_)), "{e}");
                eprintln!("无可用输入设备（{e}），跳过后续断言");
            }
        }
    }

    #[test]
    fn named_device_lookup_fails_for_bogus_name() {
        let ring = Arc::new(PcmRingBuffer::new(16384));
        let err = MicrophoneCapture::new(ring, Some("不存在的设备名_xyz")).unwrap_err();
        assert!(matches!(err, AppError::AudioDeviceNotFound(_)));
    }

    #[test]
    fn capture_writes_samples_when_device_available() {
        let ring = Arc::new(PcmRingBuffer::new(16384));
        let mut cap = match MicrophoneCapture::new(Arc::clone(&ring), None) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("无可用输入设备（{e}），跳过采集测试");
                return;
            }
        };
        cap.start().expect("启动采集");
        std::thread::sleep(Duration::from_millis(300));
        assert!(ring.available() > 0, "采集流应已写入样本（静音也算样本）");
        assert_eq!(cap.sample_rate(), TEST_SIGNAL_SAMPLE_RATE);
        assert_eq!(cap.channels(), 1);
        cap.stop().expect("停止采集");
        // 重复 stop 幂等
        cap.stop().expect("重复停止采集");
    }
}
