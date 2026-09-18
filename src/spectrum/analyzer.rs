//! 频谱分析流水线与后台分析线程。
//!
//! 完整链路（所有缓冲复用，稳定运行时每帧零分配）：
//!
//! ```text
//! PCM 环形缓冲 → 加窗 → FFT → 幅度谱 → 对数频段聚合
//!   → 高频倾斜补偿 + dB 归一化 → attack/release 平滑
//!   → 峰值保持 → height_scale 增益 → SpectrumFrame
//! ```
//!
//! [`AnalysisPipeline`] 是纯同步组件（可独立测试）；
//! [`AnalyzerHandle`] 在独立线程上以配置帧率驱动流水线，
//! 通过 crossbeam-channel 把 owned 的 [`SpectrumFrame`] 发往消费侧
//! （UI 线程绝不回头进入音频/分析路径，满足单向数据流约束）。
//!
//! 本模块不依赖 Slint。

use crate::audio::buffer::PcmRingBuffer;
use crate::config::SpectrumSettings;
use crate::spectrum::bands::BandMapper;
use crate::spectrum::fft::FftProcessor;
use crate::spectrum::normalize::{normalize_db, tilt_gain_db};
use crate::spectrum::peak::PeakHold;
use crate::spectrum::smoother::SpectrumSmoother;
use crate::spectrum::window::WindowFunction;
use crossbeam_channel::{Receiver, SendTimeoutError, bounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 低频（Bass）判定上限（Hz）：中心频率低于此值的频段计入 bass 能量。
pub const BASS_CEILING_HZ: f32 = 250.0;

/// 一帧频谱数据：逐柱归一化高度与峰值位置（均被夹在 `[0, 1]`）。
///
/// owned 发送（跨线程无共享可变状态）；柱数恒等于配置的频谱柱数。
#[derive(Debug, Clone)]
pub struct SpectrumFrame {
    /// 平滑后的柱高（0 = 贴地，1 = 满高）。
    pub heights: Vec<f32>,
    /// 每柱峰顶位置（恒 >= 对应柱高）。
    pub peaks: Vec<f32>,
    /// 低频能量（0~250Hz 频段平滑后均值，0 = 静音，1 = 满幅）。
    ///
    /// 供反应式基线等需要“整体响度”而非逐柱细节的消费方使用。
    pub bass: f32,
}

impl SpectrumFrame {
    /// 柱数。
    pub fn bar_count(&self) -> usize {
        self.heights.len()
    }
}

/// 同步频谱分析流水线（单线程拥有，可反复调用）。
pub struct AnalysisPipeline {
    ring: Arc<PcmRingBuffer>,
    window: WindowFunction,
    fft: FftProcessor,
    bands: BandMapper,
    smoother: SpectrumSmoother,
    peaks: PeakHold,
    /// FFT 输入窗口缓冲（从环形缓冲读出后就地加窗）。
    sample_buffer: Vec<f32>,
    /// 频段幅度缓冲（复用，避免每帧分配）。
    band_mags: Vec<f32>,
    /// 归一化目标高度缓冲（复用）。
    normalized: Vec<f32>,
    /// 最终输出高度缓冲（复用）。
    heights: Vec<f32>,
    min_db: f32,
    max_db: f32,
    tilt_db_per_octave: f32,
    height_scale: f32,
}

impl AnalysisPipeline {
    /// 依据频谱配置与音频采样率构建流水线。
    pub fn new(ring: Arc<PcmRingBuffer>, settings: &SpectrumSettings, sample_rate: u32) -> Self {
        let fft_size = settings.fft_size.max(16);
        let bar_count = settings.bar_count.max(1);
        Self {
            ring,
            window: WindowFunction::new(settings.window, fft_size),
            fft: FftProcessor::new(fft_size),
            bands: BandMapper::new(settings, sample_rate),
            smoother: SpectrumSmoother::new(bar_count, settings.attack, settings.release),
            peaks: PeakHold::new(bar_count, settings.peak_decay),
            sample_buffer: vec![0.0; fft_size],
            band_mags: Vec::with_capacity(bar_count),
            normalized: vec![0.0; bar_count],
            heights: vec![0.0; bar_count],
            min_db: settings.min_db,
            max_db: settings.max_db,
            tilt_db_per_octave: settings.tilt_db_per_octave,
            height_scale: settings.height_scale,
        }
    }

    /// FFT 窗口大小。
    pub fn fft_size(&self) -> usize {
        self.fft.size()
    }

    /// 频谱柱数。
    pub fn bar_count(&self) -> usize {
        self.smoother.bar_count()
    }

    /// 重置平滑与峰值状态（音源切换时由持有方调用）。
    pub fn reset(&mut self) {
        self.smoother.reset();
        self.peaks.reset();
        self.ring.clear();
    }

    /// 消费环形缓冲中的可用数据，返回最新一帧。
    ///
    /// 数据不足一个 FFT 窗口时返回 `None`（调用方按自己的节奏轮询）；
    /// 积压多个窗口时全部处理并只返回最后一帧（频谱展示关心"现在"，
    /// 不需要回放历史帧）。
    pub fn process_available(&mut self) -> Option<SpectrumFrame> {
        let fft_size = self.fft_size();
        if self.ring.available() < fft_size {
            return None;
        }
        let mut frame = None;
        while self.ring.available() >= fft_size {
            let n = self.ring.read_into(&mut self.sample_buffer);
            if n < fft_size {
                break; // 竞态下读到不足一窗，留待下轮
            }
            frame = Some(self.process_block());
        }
        frame
    }

    /// 处理 `sample_buffer` 中的一整窗样本（已由调用方填满）。
    fn process_block(&mut self) -> SpectrumFrame {
        // 1. 加窗（sample_buffer 是从环形缓冲读出的副本，可安全就地修改）
        self.window.apply(&mut self.sample_buffer);

        // 2. FFT 幅度谱（长度与 fft_size 恒一致，构造时已对齐）
        let mags = self
            .fft
            .magnitude_spectrum(&self.sample_buffer)
            .expect("内部不变量：sample_buffer 长度与 FFT 尺寸在构造时对齐");

        // 3. 对数频段聚合（复用 band_mags）
        self.bands.values_into(mags, &mut self.band_mags);

        // 4. 高频倾斜补偿 + dB 归一化（复用 normalized）
        let bar_count = self.bar_count();
        for i in 0..bar_count {
            let center_hz = self.bands.center_hz(i);
            let gain = db_to_linear(tilt_gain_db(center_hz, self.tilt_db_per_octave));
            let mag = self.band_mags[i] * gain;
            self.normalized[i] = normalize_db(mag, self.min_db, self.max_db);
        }

        // 5. attack/release 平滑（复用 smoother 内部状态）
        let smoothed = self.smoother.process(&self.normalized);

        // 6. 增益 + 夹取（复用 heights）；顺带累计低频（<=250Hz）平滑能量
        let mut bass_sum = 0.0f32;
        let mut bass_bins = 0usize;
        for (i, &s) in smoothed.iter().enumerate() {
            self.heights[i] = (s * self.height_scale).clamp(0.0, 1.0);
            if self.bands.center_hz(i) <= BASS_CEILING_HZ {
                bass_sum += s;
                bass_bins += 1;
            }
        }
        // 无低频柱时（极端配置）退化为全谱最大值，避免 bass 恒为 0
        let bass = if bass_bins > 0 {
            bass_sum / bass_bins as f32
        } else {
            smoothed.iter().copied().fold(0.0f32, f32::max)
        }
        .clamp(0.0, 1.0);

        // 7. 峰值保持（输入为最终展示高度，峰点恒 >= 柱高）
        let peaks = self.peaks.process(&self.heights);

        SpectrumFrame {
            heights: self.heights.clone(),
            peaks: peaks.to_vec(),
            bass,
        }
    }
}

/// dB 转线性增益（`10^(db/20)`）。
fn db_to_linear(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

/// 后台频谱分析线程句柄。
///
/// 线程以配置帧率（`SpectrumSettings::fps`）轮询流水线；
/// 帧经有界 channel 发送（容量 4），消费侧卡顿时分析线程自然
/// 背压而非无限堆积。Drop 时停止并 join 线程。
pub struct AnalyzerHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    frames: Receiver<SpectrumFrame>,
}

impl AnalyzerHandle {
    /// 启动分析线程。
    pub fn spawn(
        ring: Arc<PcmRingBuffer>,
        settings: &SpectrumSettings,
        sample_rate: u32,
    ) -> crate::error::AppResult<Self> {
        let fps = settings.fps.clamp(15, 120) as f32;
        let period = Duration::from_secs_f32(1.0 / fps);
        let (frame_tx, frames) = bounded::<SpectrumFrame>(4);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);

        let mut pipeline = AnalysisPipeline::new(ring, settings, sample_rate);
        let join = std::thread::Builder::new()
            .name("spectrum-analyzer".to_string())
            .spawn(move || {
                let mut next_tick = Instant::now() + period;
                loop {
                    if stop_for_thread.load(Ordering::Acquire) {
                        break;
                    }
                    if let Some(frame) = pipeline.process_available() {
                        // 接收端已关闭（应用退出）则结束线程；
                        // 队列满（消费侧卡顿）时丢弃本帧继续节拍，
                        // 避免阻塞线程导致 Drop 的 join 死锁
                        match frame_tx.send_timeout(frame, period) {
                            Ok(()) => {}
                            Err(SendTimeoutError::Disconnected(_)) => break,
                            Err(SendTimeoutError::Timeout(_)) => {
                                tracing::trace!("频谱帧队列满，丢弃本帧");
                            }
                        }
                    }
                    // 固定节拍休眠；落后过多时重新对齐，避免突发追帧
                    let now = Instant::now();
                    if next_tick > now {
                        std::thread::sleep(next_tick - now);
                    } else {
                        next_tick = now;
                    }
                    next_tick += period;
                }
                tracing::debug!("频谱分析线程退出");
            })
            .map_err(|e| {
                crate::error::AppError::AudioStreamError(format!("启动分析线程失败: {e}"))
            })?;

        Ok(Self {
            stop,
            join: Some(join),
            frames,
        })
    }

    /// 非阻塞地排空积压帧，返回最新一帧（无新帧返回 `None`）。
    pub fn latest_frame(&self) -> Option<SpectrumFrame> {
        let mut latest = None;
        while let Ok(frame) = self.frames.try_recv() {
            latest = Some(frame);
        }
        latest
    }

    /// 是否仍有新帧待消费（诊断用）。
    pub fn has_pending_frames(&self) -> bool {
        !self.frames.is_empty()
    }
}

impl Drop for AnalyzerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::source::TestSignalGenerator;
    use crate::config::Settings;

    fn pipeline(sample_rate: u32) -> (Arc<PcmRingBuffer>, AnalysisPipeline) {
        let settings = Settings::default();
        let ring = Arc::new(PcmRingBuffer::new(settings.spectrum.fft_size * 4));
        let pipeline = AnalysisPipeline::new(Arc::clone(&ring), &settings.spectrum, sample_rate);
        (ring, pipeline)
    }

    #[test]
    fn insufficient_data_returns_none() {
        let (ring, mut pipeline) = pipeline(44100);
        assert!(pipeline.process_available().is_none());
        ring.write(&[0.1; 100]); // 不足一窗
        assert!(pipeline.process_available().is_none());
    }

    #[test]
    fn silence_produces_zero_frame_without_nan() {
        let (ring, mut pipeline) = pipeline(44100);
        let silence = vec![0.0f32; pipeline.fft_size()];
        for _ in 0..4 {
            ring.write(&silence);
            let frame = pipeline.process_available().expect("full window");
            assert!(frame.heights.iter().all(|h| h.is_finite()));
            assert!(frame.heights.iter().all(|h| *h < 1e-6));
            assert!(frame.peaks.iter().all(|p| p.is_finite()));
            assert!(frame.bass < 1e-6, "静音时 bass 应接近 0");
        }
    }

    #[test]
    fn test_signal_produces_bounded_active_frame() {
        let (ring, mut pipeline) = pipeline(44100);
        let mut generator = TestSignalGenerator::new(44100);
        let mut chunk = vec![0.0f32; pipeline.fft_size()];
        // 推进若干帧让平滑收敛
        let mut frame = None;
        for _ in 0..8 {
            generator.generate(&mut chunk);
            ring.write(&chunk);
            frame = pipeline.process_available();
        }
        let frame = frame.expect("frame after 8 windows");
        assert_eq!(frame.bar_count(), 88);
        assert!(frame.heights.iter().all(|h| (0.0..=1.0).contains(h)));
        // 鼓点/扫频信号应让低频能量非零且在合法范围
        assert!(
            (0.0..=1.0).contains(&frame.bass),
            "bass 应在 0..=1，实际 {}",
            frame.bass
        );
        // 鼓点/扫频信号应让至少一根柱明显抬升
        assert!(
            frame.heights.iter().any(|h| *h > 0.2),
            "heights: {:?}",
            &frame.heights[..10]
        );
        // 峰值恒不低于柱高
        for (h, p) in frame.heights.iter().zip(&frame.peaks) {
            assert!(p >= h, "peak {p} < height {h}");
        }
    }

    #[test]
    fn spike_then_silence_decays_smoothly_not_instantly() {
        let (ring, mut pipeline) = pipeline(44100);
        let mut generator = TestSignalGenerator::new(44100);
        let mut chunk = vec![0.0f32; pipeline.fft_size()];
        // 灌入信号
        for _ in 0..6 {
            generator.generate(&mut chunk);
            ring.write(&chunk);
            let _ = pipeline.process_available();
        }
        // 静音一帧：高度不应瞬间归零
        ring.write(&vec![0.0f32; pipeline.fft_size()]);
        let frame = pipeline
            .process_available()
            .expect("silence window processed");
        let max_height = frame.heights.iter().copied().fold(0.0f32, f32::max);
        assert!(
            max_height > 0.05,
            "heights dropped instantly to {max_height}"
        );
    }

    #[test]
    fn analyzer_thread_produces_frames_and_stops() {
        let settings = Settings::default();
        let ring = Arc::new(PcmRingBuffer::new(settings.spectrum.fft_size * 8));
        let mut generator = TestSignalGenerator::new(44100);
        let mut chunk = vec![0.0f32; 2048];
        generator.generate(&mut chunk);
        ring.write(&chunk);

        let handle = AnalyzerHandle::spawn(Arc::clone(&ring), &settings.spectrum, 44100)
            .expect("spawn analyzer");
        // 持续喂数据一小段时间
        std::thread::sleep(Duration::from_millis(350));
        generator.generate(&mut chunk);
        ring.write(&chunk);
        std::thread::sleep(Duration::from_millis(350));
        let got_frame = handle.latest_frame().is_some();
        drop(handle); // Drop 应在分析线程节拍内完成 join
        assert!(got_frame, "analyzer thread should emit frames");
    }
}
