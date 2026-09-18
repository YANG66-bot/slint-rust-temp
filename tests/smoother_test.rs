//! 平滑与峰值保持集成测试：上升快/下降慢/峰值缓降的视觉动力学。

use music_spectrum::spectrum::{PeakHold, SpectrumSmoother};

/// 用 `frames` 帧把目标值 `target` 从 0 推进到稳态（attack=1 直接到位）。
fn settle(attack: f32, release: f32, target: f32, frames: usize) -> SpectrumSmoother {
    let mut smoother = SpectrumSmoother::new(1, attack, release);
    for _ in 0..frames {
        smoother.process(&[target]);
    }
    smoother
}

#[test]
fn attack_reaches_steady_state_within_few_frames() {
    // attack 0.65：每帧残留 35%，8 帧后残留 0.35^8 < 0.001
    let smoother = settle(0.65, 0.12, 1.0, 8);
    let h = smoother.values()[0];
    assert!(h > 0.999, "8 帧后应达稳态，实际 {h}");
}

#[test]
fn release_decays_exponentially_and_slowly() {
    let mut smoother = settle(0.65, 0.12, 1.0, 8);
    // 释放：每帧只下降 12% 的差距
    smoother.process(&[0.0]);
    let h1 = smoother.values()[0];
    assert!((h1 - 0.88).abs() < 0.005, "释放一帧后应约 0.88，实际 {h1}");
    smoother.process(&[0.0]);
    let h2 = smoother.values()[0];
    assert!(
        (h2 - 0.88 * 0.88).abs() < 0.005,
        "释放两帧后应约 0.774，实际 {h2}"
    );
    assert!(h2 > 0.5, "两帧内不应掉到一半以下");
}

#[test]
fn rising_is_faster_than_falling() {
    let mut smoother = SpectrumSmoother::new(1, 0.65, 0.12);
    smoother.process(&[1.0]);
    let rise = smoother.values()[0];
    let before_fall = smoother.values()[0];
    smoother.process(&[0.0]);
    let fall = before_fall - smoother.values()[0];
    assert!(
        rise > fall * 2.0,
        "单帧上升 {rise} 应显著快于单帧下降 {fall}"
    );
}

#[test]
fn heights_stay_in_unit_range_with_extreme_targets() {
    let mut smoother = SpectrumSmoother::new(3, 0.65, 0.12);
    // 输入恒为归一化后的 [0,1]，但极端值也不应产生发散
    smoother.process(&[1.0, 0.5, 0.0]);
    smoother.process(&[0.0, 0.5, 1.0]);
    let values = smoother.values();
    assert!(values.iter().all(|v| (0.0..=1.0).contains(v)));
    // 指数逼近不过冲目标
    assert!(values[0] <= 1.0 + 1e-6);
    assert!(values[2] >= 0.0 - 1e-6);
}

#[test]
fn peak_updates_immediately_and_decays_slowly() {
    let mut peaks = PeakHold::new(1, 0.985);
    // 峰值立即刷新
    let out = peaks.process(&[0.6]);
    assert_eq!(out[0], 0.6);
    // 柱高下落后峰值缓降，且不低于柱高
    let out = peaks.process(&[0.4]);
    assert!(
        (out[0] - 0.591).abs() < 0.005,
        "峰值应缓降，实际 {}",
        out[0]
    );
    assert!(out[0] >= 0.4);
    // 多帧持续衰减
    let mut last = out[0];
    for _ in 0..10 {
        let out = peaks.process(&[0.4]);
        assert!(out[0] >= 0.4);
        assert!(out[0] <= last + 1e-6, "峰值不应回升");
        last = out[0];
    }
    // 10 帧后仍明显高于柱高（0.985^11 ≈ 0.85 × 初始峰值）
    assert!(last > 0.5, "10 帧后峰值 {last} 应仍悬停于 0.4 之上");
}

#[test]
fn peak_always_at_or_above_current_height() {
    let mut peaks = PeakHold::new(4, 0.985);
    let heights = [0.9, 0.5, 0.2, 0.0];
    let out = peaks.process(&heights);
    for (h, p) in heights.iter().zip(out) {
        assert!(p >= h, "peak {p} 低于柱高 {h}");
    }
    // 剧烈波动下保持不变量
    let heights = [0.1, 0.95, 0.0, 0.7];
    let out = peaks.process(&heights);
    for (h, p) in heights.iter().zip(out) {
        assert!(p >= h, "peak {p} 低于柱高 {h}");
    }
}

#[test]
fn smoother_and_peak_pipeline_visual_contract() {
    // 模拟真实流水线的视觉契约：高度 ∈ [0,1]，峰值 >= 高度，
    // 瞬态消失后高度缓降、峰值更缓降
    let mut smoother = SpectrumSmoother::new(8, 0.65, 0.12);
    let mut peaks = PeakHold::new(8, 0.985);

    // 稳态：随机音乐状目标
    let targets: [f32; 8] = [0.2, 0.8, 0.5, 0.9, 0.3, 0.7, 0.4, 0.6];
    for _ in 0..6 {
        let heights = smoother.process(&targets);
        let _ = peaks.process(heights);
    }
    // 静音
    let heights = smoother.process(&[0.0; 8]);
    let peak_values = peaks.process(heights);
    let max_height = heights.iter().copied().fold(0.0f32, f32::max);
    let max_peak = peak_values.iter().copied().fold(0.0f32, f32::max);
    assert!(max_height > 0.05, "静音一帧后柱高 {max_height} 不应骤归零");
    assert!(
        max_peak > max_height,
        "峰值 {max_peak} 应高于柱高 {max_height}"
    );
    for (h, p) in heights.iter().zip(peak_values) {
        assert!(p >= h);
        assert!((0.0..=1.0).contains(h));
        assert!((0.0..=1.0).contains(p));
    }
}
