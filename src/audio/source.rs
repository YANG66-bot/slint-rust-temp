//! 音频源抽象与各源实现。
//!
//! 所有产生 PCM 数据的途径（测试信号 / 文件播放 / 麦克风 / 系统环回）
//! 都实现 [`AudioSource`]，由音频引擎统一启动与停止。
//! 各音频源把数据统一混叠为单声道 f32 后写入共享环形缓冲。

use crate::audio::buffer::PcmRingBuffer;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// 音频源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSourceKind {
    /// 内置测试信号（多正弦波模拟音乐，无需任何音频设备）。
    Test,
    /// 播放本地音频文件（WAV / MP3 / FLAC / OGG / Vorbis）。
    File,
    /// 麦克风采集（cpal 输入流）。
    Microphone,
    /// 系统音频环回捕获（WASAPI loopback，Windows 专属能力，预留接口）。
    System,
}

impl std::fmt::Display for AudioSourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AudioSourceKind::Test => "测试信号",
            AudioSourceKind::File => "音频文件",
            AudioSourceKind::Microphone => "麦克风",
            AudioSourceKind::System => "系统音频",
        };
        f.write_str(s)
    }
}

/// 音频源抽象：向共享环形缓冲写入单声道 PCM。
pub trait AudioSource: Send {
    /// 开始产生音频数据。
    fn start(&mut self) -> AppResult<()>;

    /// 停止产生音频数据（释放线程 / 流资源）。
    fn stop(&mut self) -> AppResult<()>;

    /// 采样率（Hz）。
    fn sample_rate(&self) -> u32;

    /// 声道数。
    fn channels(&self) -> u16;

    /// 人类可读的描述（用于状态栏）。
    fn describe(&self) -> String;
}

/// 测试信号源的采样率。
pub const TEST_SIGNAL_SAMPLE_RATE: u32 = 44100;

/// 内置测试信号源。
///
/// 在独立线程中持续生成"模拟音乐"（低频鼓点包络 + 中频持续音 +
/// 高频颤动 + 缓慢扫频），写入环形缓冲。无需任何音频设备，
/// 程序启动后即可看到频谱柱动态跳动。
pub struct TestSignalSource {
    ring: Arc<PcmRingBuffer>,
    sample_rate: u32,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TestSignalSource {
    /// 创建测试信号源（写入目标为 `ring`）。
    pub fn new(ring: Arc<PcmRingBuffer>) -> Self {
        Self {
            ring,
            sample_rate: TEST_SIGNAL_SAMPLE_RATE,
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }
}

impl AudioSource for TestSignalSource {
    fn start(&mut self) -> AppResult<()> {
        if self.handle.is_some() {
            return Ok(());
        }
        self.stop.store(false, Ordering::Release);
        let ring = self.ring.clone();
        let stop = self.stop.clone();
        let rate = self.sample_rate;
        let handle = std::thread::Builder::new()
            .name("test-signal".to_string())
            .spawn(move || run_test_signal(ring, stop, rate))
            .map_err(|e| AppError::AudioStreamError(format!("启动测试信号线程失败: {e}")))?;
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
        self.sample_rate
    }

    fn channels(&self) -> u16 {
        1
    }

    fn describe(&self) -> String {
        format!("测试信号 {}Hz 单声道（模拟音乐）", self.sample_rate)
    }
}

impl Drop for TestSignalSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// 测试信号线程主循环。
fn run_test_signal(ring: Arc<PcmRingBuffer>, stop: Arc<AtomicBool>, rate: u32) {
    let mut generator = TestSignalGenerator::new(rate);
    let block = 1024usize;
    let mut buf = vec![0.0f32; block];
    let block_duration = Duration::from_secs_f64(block as f64 / f64::from(rate));
    tracing::debug!("测试信号线程启动: {rate}Hz, 块大小 {block}");
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        generator.generate(&mut buf);
        ring.write(&buf);
        std::thread::sleep(block_duration);
    }
    tracing::debug!("测试信号线程退出");
}

/// 测试信号发生器：多正弦波叠加 + 节奏包络 + 缓慢扫频。
///
/// 频率成分覆盖 55Hz ~ 5kHz，保证低/中/高频段都有明显变化：
/// - 55/110Hz：110 BPM 鼓点包络（指数衰减），低频跳动
/// - 220/440Hz：慢 LFO 调幅持续音，中频起伏
/// - 1760Hz：快速颤动，高频闪烁
/// - 1.2k~2.1kHz：缓慢扫频，中高频连续游走
/// - 5kHz：微小幅度的闪烁点缀
pub struct TestSignalGenerator {
    sample_rate: f32,
    time_secs: f32,
    sweep_phase: f32,
}

impl TestSignalGenerator {
    /// 创建发生器。
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate as f32,
            time_secs: 0.0,
            sweep_phase: 0.0,
        }
    }

    /// 生成一段样本（就地写入 `out`）。
    pub fn generate(&mut self, out: &mut [f32]) {
        const TAU: f32 = std::f32::consts::TAU;
        let dt = 1.0 / self.sample_rate;
        for s in out.iter_mut() {
            let t = self.time_secs;
            // 110 BPM 鼓点包络
            let beat = (t * 110.0 / 60.0).fract();
            let kick = (-beat * 6.0).exp();
            let bass = 0.42 * kick * (TAU * 55.0 * t).sin() + 0.18 * kick * (TAU * 110.0 * t).sin();
            // 中频：慢 LFO 调幅
            let mid = 0.16 * (0.5 + 0.45 * (TAU * 0.11 * t).sin()) * (TAU * 220.0 * t).sin()
                + 0.13 * (0.5 + 0.45 * (TAU * 0.07 * t + 1.7).sin()) * (TAU * 440.0 * t).sin();
            // 高频：快速颤动
            let high = 0.10 * (0.5 + 0.5 * (TAU * 0.9 * t).sin()) * (TAU * 1760.0 * t).sin();
            // 缓慢扫频（相位积分，避免频率跳变）
            let sweep_f = 1200.0 + 900.0 * (TAU * 0.05 * t).sin();
            self.sweep_phase += TAU * sweep_f * dt;
            let sweep = 0.07 * self.sweep_phase.sin();
            // 5kHz 闪烁点缀
            let sparkle = 0.05 * (0.5 + 0.5 * (TAU * 2.3 * t).sin()) * (TAU * 5000.0 * t).sin();

            *s = (bass + mid + high + sweep + sparkle).clamp(-1.0, 1.0);
            self.time_secs += dt;
        }
    }
}

/// 系统音频环回捕获（预留接口，未实现）。
///
/// Windows 需要 WASAPI loopback（cpal 不支持该能力），macOS 需要
/// ScreenCaptureKit/虚拟音频设备，Linux 需要 PulseAudio monitor 源。
/// 为避免伪造实现，此源启动时明确返回错误。
pub struct SystemAudioSource;

impl AudioSource for SystemAudioSource {
    fn start(&mut self) -> AppResult<()> {
        Err(AppError::UnsupportedFormat(
            "系统音频环回捕获尚未实现（需要 WASAPI loopback 等平台专属能力）".to_string(),
        ))
    }

    fn stop(&mut self) -> AppResult<()> {
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        48000
    }

    fn channels(&self) -> u16 {
        2
    }

    fn describe(&self) -> String {
        "系统音频环回（未实现，预留接口）".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_produces_bounded_nonzero_signal() {
        let mut generator = TestSignalGenerator::new(44100);
        let mut buf = [0.0f32; 4096];
        generator.generate(&mut buf);
        assert!(buf.iter().all(|v| v.abs() <= 1.0));
        assert!(buf.iter().any(|v| v.abs() > 1e-3));
        // 长时间窗口无直流偏置（短窗内周期信号均值天然波动，
        // 且 bin 0 直流分量不参与 20Hz 起的频段映射）
        let mut long_term = vec![0.0f32; 132_300]; // ≈3 秒 @ 44.1kHz
        generator.generate(&mut long_term);
        let mean = long_term.iter().sum::<f32>() / long_term.len() as f32;
        assert!(mean.abs() < 1e-2, "mean = {mean}");
    }

    #[test]
    fn generator_energy_varies_over_time() {
        let mut generator = TestSignalGenerator::new(44100);
        let rms = |buf: &[f32]| (buf.iter().map(|v| v * v).sum::<f32>() / buf.len() as f32).sqrt();
        let mut a = [0.0f32; 2048];
        generator.generate(&mut a);
        let mut b = [0.0f32; 2048];
        // 推进约 0.2 秒，鼓点包络应使能量不同
        for _ in 0..4 {
            generator.generate(&mut b);
        }
        assert!((rms(&a) - rms(&b)).abs() > 1e-4);
    }

    #[test]
    fn test_signal_source_starts_and_stops() {
        let ring = Arc::new(PcmRingBuffer::new(8192));
        let mut source = TestSignalSource::new(ring.clone());
        assert!(source.start().is_ok());
        std::thread::sleep(Duration::from_millis(80));
        assert!(ring.available() > 0);
        assert_eq!(source.sample_rate(), TEST_SIGNAL_SAMPLE_RATE);
        assert_eq!(source.channels(), 1);
        assert!(!source.describe().is_empty());
        assert!(source.stop().is_ok());
        let available = ring.available();
        assert!(available > 0, "停止后已生成数据应可读: {available}");
    }

    #[test]
    fn system_audio_source_is_honestly_unimplemented() {
        let mut source = SystemAudioSource;
        let err = source.start().unwrap_err();
        assert!(matches!(err, AppError::UnsupportedFormat(_)));
    }
}
