//! 频谱流水线集成测试：PCM → FFT → 频段 → 归一化 → 平滑 → 峰值。
//!
//! 通过公共 API（`music_spectrum::spectrum::AnalysisPipeline`）驱动，
//! 验证真实数据流的端到端行为。

use music_spectrum::audio::buffer::PcmRingBuffer;
use music_spectrum::audio::source::TestSignalGenerator;
use music_spectrum::config::Settings;
use music_spectrum::spectrum::{AnalysisPipeline, AnalyzerHandle, SpectrumFrame};
use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::Duration;

const SAMPLE_RATE: u32 = 44100;

fn pipeline_with(settings: &Settings) -> (Arc<PcmRingBuffer>, AnalysisPipeline) {
    let ring = Arc::new(PcmRingBuffer::new(settings.spectrum.fft_size * 8));
    let pipeline = AnalysisPipeline::new(Arc::clone(&ring), &settings.spectrum, SAMPLE_RATE);
    (ring, pipeline)
}

/// 生成 `secs` 秒指定频率的满幅正弦（连续相位）。
fn sine(freq: f32, count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| 0.8 * (TAU * freq * i as f32 / SAMPLE_RATE as f32).sin())
        .collect()
}

/// 找到覆盖 `freq` 的频段索引。
fn band_containing(freq: f32) -> usize {
    // 借助同配置的 BandMapper 查询频段边界
    let settings = Settings::default();
    let mapper = music_spectrum::spectrum::BandMapper::new(&settings.spectrum, SAMPLE_RATE);
    for i in 0..mapper.bar_count() {
        let (lo, hi) = mapper.band_edges_hz(i);
        if freq >= lo && freq < hi {
            return i;
        }
    }
    mapper.bar_count() - 1
}

#[test]
fn fft_pipeline_reports_correct_bar_count() {
    let settings = Settings::default();
    let (_ring, pipeline) = pipeline_with(&settings);
    assert_eq!(pipeline.bar_count(), settings.spectrum.bar_count);
    assert_eq!(pipeline.fft_size(), settings.spectrum.fft_size);
}

#[test]
fn sine_wave_peaks_in_expected_band() {
    let settings = Settings::default();
    let (ring, mut pipeline) = pipeline_with(&settings);
    let window = settings.spectrum.fft_size;
    let target_band = band_containing(440.0);

    // 多写几窗让 attack 平滑收敛到稳态
    let samples = sine(440.0, window);
    let mut frame = None;
    for _ in 0..8 {
        ring.write(&samples);
        frame = pipeline.process_available();
    }
    let frame = frame.expect("processed windows");
    assert_eq!(frame.bar_count(), settings.spectrum.bar_count);

    let max_band = frame
        .heights
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .expect("non-empty");
    assert_eq!(
        max_band, target_band,
        "440Hz 正弦峰值应在 band {target_band}，实际 {max_band}"
    );
    assert!(
        frame.heights[target_band] > 0.3,
        "440Hz band 高度 {}",
        frame.heights[target_band]
    );
}

#[test]
fn silence_yields_all_zero_without_nan() {
    let settings = Settings::default();
    let (ring, mut pipeline) = pipeline_with(&settings);
    let window = settings.spectrum.fft_size;
    let silence = vec![0.0f32; window];

    for _ in 0..4 {
        ring.write(&silence);
        let frame: SpectrumFrame = pipeline.process_available().expect("window");
        assert!(frame.heights.iter().all(|h| h.is_finite()), "出现 NaN/Inf");
        assert!(frame.heights.iter().all(|h| *h < 1e-6), "静音高度非 0");
        assert!(frame.peaks.iter().all(|p| p.is_finite()));
    }
}

#[test]
fn heights_always_bounded_in_unit_range() {
    let settings = Settings::default();
    let (ring, mut pipeline) = pipeline_with(&settings);
    let mut generator = TestSignalGenerator::new(SAMPLE_RATE);
    let mut chunk = vec![0.0f32; settings.spectrum.fft_size];

    for _ in 0..12 {
        generator.generate(&mut chunk);
        ring.write(&chunk);
        if let Some(frame) = pipeline.process_available() {
            assert!(frame.heights.iter().all(|h| (0.0..=1.0).contains(h)));
            assert!(frame.peaks.iter().all(|p| (0.0..=1.0).contains(p)));
        }
    }
}

#[test]
fn abrupt_silence_decays_gradually() {
    // 核心视觉约束：高度绝不允许瞬间从高变 0
    let settings = Settings::default();
    let (ring, mut pipeline) = pipeline_with(&settings);
    let mut generator = TestSignalGenerator::new(SAMPLE_RATE);
    let mut chunk = vec![0.0f32; settings.spectrum.fft_size];

    // 建立稳态能量
    for _ in 0..8 {
        generator.generate(&mut chunk);
        ring.write(&chunk);
        let _ = pipeline.process_available();
    }
    // 骤然静音一窗
    ring.write(&vec![0.0f32; settings.spectrum.fft_size]);
    let frame = pipeline.process_available().expect("silence window");
    let max_height = frame.heights.iter().copied().fold(0.0f32, f32::max);
    assert!(
        max_height > 0.05,
        "静音一帧后最大柱高 {max_height} 应缓降而非瞬间归零"
    );
}

#[test]
fn peaks_never_below_heights() {
    let settings = Settings::default();
    let (ring, mut pipeline) = pipeline_with(&settings);
    let mut generator = TestSignalGenerator::new(SAMPLE_RATE);
    let mut chunk = vec![0.0f32; settings.spectrum.fft_size];

    for _ in 0..10 {
        generator.generate(&mut chunk);
        ring.write(&chunk);
        if let Some(frame) = pipeline.process_available() {
            for (h, p) in frame.heights.iter().zip(&frame.peaks) {
                assert!(p >= h, "peak {p} 低于柱高 {h}");
            }
        }
    }
}

#[test]
fn analyzer_thread_emits_frames_and_shuts_down_cleanly() {
    let settings = Settings::default();
    let ring = Arc::new(PcmRingBuffer::new(settings.spectrum.fft_size * 8));
    let mut generator = TestSignalGenerator::new(SAMPLE_RATE);
    let mut chunk = vec![0.0f32; settings.spectrum.fft_size];
    generator.generate(&mut chunk);
    ring.write(&chunk);

    let handle =
        AnalyzerHandle::spawn(Arc::clone(&ring), &settings.spectrum, SAMPLE_RATE).expect("spawn");
    // 持续供给数据，让分析线程有机会产出多帧
    for _ in 0..10 {
        generator.generate(&mut chunk);
        ring.write(&chunk);
        std::thread::sleep(Duration::from_millis(60));
    }
    let frame = handle.latest_frame();
    assert!(frame.is_some(), "分析线程应产出至少一帧");
    if let Some(frame) = frame {
        assert_eq!(frame.bar_count(), settings.spectrum.bar_count);
    }
    // Drop 触发停止 + join，不应死锁或超时
    drop(handle);
}
