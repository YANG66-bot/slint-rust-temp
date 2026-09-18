//! Rust ↔ Slint 桥接：把展示状态写入 Slint Model。
//!
//! 使用 `VecModel` + `set_row_data` 做逐行增量更新：
//! 60Hz 刷新时只更新变化的数值，不重建模型，
//! 避免 Slint 全量重传导致的 GC 压力与布局抖动。

use crate::visualizer::state::VisualizerState;
use slint::{Model, VecModel};
use std::rc::Rc;

use crate::SpectrumBarData;

/// 峰点颜色亮化强度（柱色向白色混合的比例）。
const PEAK_LIGHTEN: f32 = 0.35;

/// 画布顶部留白（headroom）：柱高/峰线最大只占主谱区 85%，
/// 任何增益下峰值都不会触碰或超出上边界。
const BAR_HEADROOM: f32 = 0.85;

/// AGC：加增益后信号归一化到的目标峰值（只削不抬）。
const AGC_TARGET_PEAK: f32 = 0.90;

/// AGC：移动峰值追踪窗口（秒，指数释放）：短暂瞬态不削，
/// 持续过响（≈2s）才整体压低，音量回落后再缓慢恢复。
const AGC_WINDOW_SECS: f32 = 2.0;

/// `[f32;3]`（0~1）→ Slint 颜色。
fn to_slint_color([r, g, b]: [f32; 3]) -> slint::Color {
    slint::Color::from_rgb_u8((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// 频谱模型桥（仅 UI 线程访问）。
pub struct SpectrumBridge {
    model: Rc<VecModel<SpectrumBarData>>,
    /// 逐柱颜色缓存（预计算，避免每次刷新插值）。
    colors: Vec<slint::Color>,
    /// 逐柱峰点颜色（亮化版）。
    peak_colors: Vec<slint::Color>,
    /// AGC 移动峰值（加增益后信号的滑动最大值，指数衰减）。
    agc_peak: f32,
}

impl SpectrumBridge {
    /// 创建 `bar_count` 行的模型桥，按调色板预计算逐柱颜色。
    pub fn new(bar_count: usize, palette: &crate::visualizer::Palette) -> Self {
        let colors: Vec<slint::Color> = palette
            .bar_colors(bar_count)
            .into_iter()
            .map(to_slint_color)
            .collect();
        let peak_colors: Vec<slint::Color> = palette
            .peak_colors(bar_count, PEAK_LIGHTEN)
            .into_iter()
            .map(to_slint_color)
            .collect();
        let rows: Vec<SpectrumBarData> = (0..bar_count)
            .map(|i| SpectrumBarData {
                height: 0.0,
                peak: 0.0,
                color: colors[i],
                peak_color: peak_colors[i],
            })
            .collect();
        let model = Rc::new(VecModel::from(rows));
        Self {
            model,
            colors,
            peak_colors,
            agc_peak: 0.0,
        }
    }

    /// Slint 模型（交给 `AppWindow::set_bars`）。
    pub fn model(&self) -> &Rc<VecModel<SpectrumBarData>> {
        &self.model
    }

    /// 把展示状态增量写入模型（高度 / 峰值逐行更新）。
    ///
    /// 映射链：`raw × gain × agc_norm → tanh 软压缩 → × headroom`。
    /// - tanh 取代硬 clamp：高增益下平滑饱和，无平顶削波；
    /// - [`BAR_HEADROOM`] 保证顶部 15% 留白，柱与峰线永不触顶；
    /// - AGC 用 [`AGC_WINDOW_SECS`] 窗口追踪加增益后的移动峰值，
    ///   持续过响时整体压低（只削不抬），瞬态尖峰不受影响。
    ///
    /// `dt_secs` 为距上次更新的秒数（驱动 AGC 释放）。
    pub fn update(
        &mut self,
        state: &VisualizerState,
        height_gain: f32,
        peak_gain: f32,
        dt_secs: f32,
    ) {
        let heights = state.heights();
        let peaks = state.peaks();
        let rows = self.model.row_count();

        // AGC：本帧加增益后的最大瞬时值，快攻慢放更新移动峰值
        let mut frame_max = 0.0f32;
        for i in 0..rows {
            frame_max = frame_max
                .max(heights.get(i).copied().unwrap_or(0.0) * height_gain)
                .max(peaks.get(i).copied().unwrap_or(0.0) * peak_gain);
        }
        let dt = dt_secs.clamp(0.0, 0.25);
        let decay = (-dt / AGC_WINDOW_SECS).exp();
        self.agc_peak = frame_max.max(self.agc_peak * decay);
        // 只在超过目标峰值时衰减，绝不提升（避免静音放大噪底）
        let norm = if self.agc_peak > AGC_TARGET_PEAK {
            AGC_TARGET_PEAK / self.agc_peak
        } else {
            1.0
        };

        for i in 0..rows {
            let h =
                (heights.get(i).copied().unwrap_or(0.0) * height_gain * norm).tanh() * BAR_HEADROOM;
            let p = (peaks.get(i).copied().unwrap_or(0.0) * peak_gain * norm).tanh() * BAR_HEADROOM;
            // 峰线不低于柱顶（tanh 单调，max 即可）
            self.model.set_row_data(
                i,
                SpectrumBarData {
                    height: h,
                    peak: p.max(h),
                    color: self.colors[i],
                    peak_color: self.peak_colors[i],
                },
            );
        }
    }

    /// 实时刷新调色板（柱数不变时）：重算逐柱颜色与峰点颜色，
    /// 下帧 `update` 即以新颜色写入模型。
    pub fn apply_palette(&mut self, palette: &crate::visualizer::Palette) {
        let n = self.model.row_count();
        self.colors = palette
            .bar_colors(n)
            .into_iter()
            .map(to_slint_color)
            .collect();
        self.peak_colors = palette
            .peak_colors(n, PEAK_LIGHTEN)
            .into_iter()
            .map(to_slint_color)
            .collect();
    }

    /// 柱数。
    pub fn bar_count(&self) -> usize {
        self.model.row_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::SpectrumFrame;
    use crate::visualizer::Palette;

    fn palette() -> Palette {
        Palette::from_hex(&[
            "#FF0000".to_string(),
            "#00FF00".to_string(),
            "#0000FF".to_string(),
            "#FFFFFF".to_string(),
        ])
    }

    /// 构造一个展示值已收敛到 vals 的状态。
    fn settled_state(vals: &[f32]) -> VisualizerState {
        let mut state = VisualizerState::new(vals.len());
        state.push_frame(&SpectrumFrame {
            heights: vals.to_vec(),
            peaks: vals.to_vec(),
            bass: 0.0,
        });
        for _ in 0..120 {
            state.tick(0.016);
        }
        state
    }

    #[test]
    fn extreme_gain_never_touches_ceiling() {
        // 修复 2：极端增益下柱与峰线均不超过 85% headroom，无平顶削波
        let mut bridge = SpectrumBridge::new(4, &palette());
        let state = settled_state(&[1.0; 4]);
        bridge.update(&state, 5.0, 8.0, 0.016);
        for i in 0..4 {
            let row = bridge.model.row_data(i).unwrap();
            assert!(row.height > 0.0);
            assert!(
                row.height <= BAR_HEADROOM + 1e-6,
                "柱高触顶: {}",
                row.height
            );
            assert!(row.peak <= BAR_HEADROOM + 1e-6, "峰线触顶: {}", row.peak);
            assert!(row.peak >= row.height);
        }
    }

    #[test]
    fn agc_ducks_sustained_loudness_and_releases_slowly() {
        // 修复 2：持续过响被压到 AGC 目标峰值；音量回落后 2s 窗口缓慢释放
        let mut bridge = SpectrumBridge::new(4, &palette());
        let loud = settled_state(&[1.0; 4]);
        // 稳定响 2 秒：agc_peak → 1×gain，输出钉在 tanh(0.9)×headroom
        for _ in 0..125 {
            bridge.update(&loud, 2.0, 2.0, 0.016);
        }
        let ducked = bridge.model.row_data(0).unwrap().height;
        let expect_ducked = AGC_TARGET_PEAK.tanh() * BAR_HEADROOM;
        assert!(
            (ducked - expect_ducked).abs() < 0.01,
            "持续响度应归一到 {expect_ducked:.3}，实际 {ducked:.3}"
        );
        // 切换到安静信号：刚切换时仍被缓慢释放的 AGC 压制
        let quiet = settled_state(&[0.1; 4]);
        bridge.update(&quiet, 2.0, 2.0, 0.016);
        let still_ducked = bridge.model.row_data(0).unwrap().height;
        let unprocessed = (0.1 * 2.0f32).tanh() * BAR_HEADROOM;
        assert!(
            still_ducked < unprocessed * 0.7,
            "回落后应仍有残留压制：{still_ducked:.3} vs {unprocessed:.3}"
        );
        // 安静持续 6 秒（≫ 2s 窗口）：AGC 完全恢复，不再施加衰减
        for _ in 0..375 {
            bridge.update(&quiet, 2.0, 2.0, 0.016);
        }
        let recovered = bridge.model.row_data(0).unwrap().height;
        assert!(
            (recovered - unprocessed).abs() < 0.01,
            "应恢复至无衰减高度 {unprocessed:.3}，实际 {recovered:.3}"
        );
    }

    #[test]
    fn quiet_signal_passes_through_without_agc() {
        // 只削不抬：安静信号不受 AGC 干预（不提升不衰减）
        let mut bridge = SpectrumBridge::new(4, &palette());
        let quiet = settled_state(&[0.1; 4]);
        bridge.update(&quiet, 2.0, 2.0, 0.016);
        let expect = (0.1 * 2.0f32).tanh() * BAR_HEADROOM;
        let row = bridge.model.row_data(0).unwrap();
        assert!(
            (row.height - expect).abs() < 1e-4,
            "安静信号应直接映射 {expect:.4}，实际 {:.4}",
            row.height
        );
    }

    #[test]
    fn palette_change_keeps_heights() {
        // 调色板热替换只改颜色不改映射链行为
        let mut bridge = SpectrumBridge::new(4, &palette());
        let state = settled_state(&[0.5; 4]);
        bridge.update(&state, 2.0, 2.0, 0.016);
        let before = bridge.model.row_data(0).unwrap().height;
        bridge.apply_palette(&palette());
        bridge.update(&state, 2.0, 2.0, 0.016);
        let after = bridge.model.row_data(0).unwrap().height;
        assert!((before - after).abs() < 1e-6);
    }
}
