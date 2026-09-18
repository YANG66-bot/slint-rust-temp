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

/// 频谱模型桥（仅 UI 线程访问）。
pub struct SpectrumBridge {
    model: Rc<VecModel<SpectrumBarData>>,
    /// 逐柱颜色缓存（预计算，避免每次刷新插值）。
    colors: Vec<slint::Color>,
    /// 逐柱峰点颜色（亮化版）。
    peak_colors: Vec<slint::Color>,
}

impl SpectrumBridge {
    /// 创建 `bar_count` 行的模型桥，按调色板预计算逐柱颜色。
    pub fn new(bar_count: usize, palette: &crate::visualizer::Palette) -> Self {
        let to_color = |[r, g, b]: [f32; 3]| {
            slint::Color::from_rgb_u8((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
        };
        let colors: Vec<slint::Color> = palette
            .bar_colors(bar_count)
            .into_iter()
            .map(to_color)
            .collect();
        let peak_colors: Vec<slint::Color> = palette
            .peak_colors(bar_count, PEAK_LIGHTEN)
            .into_iter()
            .map(to_color)
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
        }
    }

    /// Slint 模型（交给 `AppWindow::set_bars`）。
    pub fn model(&self) -> &Rc<VecModel<SpectrumBarData>> {
        &self.model
    }

    /// 把展示状态增量写入模型（高度 / 峰值逐行更新）。
    pub fn update(&self, state: &VisualizerState) {
        let heights = state.heights();
        let peaks = state.peaks();
        let rows = self.model.row_count();
        for i in 0..rows {
            let h = heights.get(i).copied().unwrap_or(0.0);
            let p = peaks.get(i).copied().unwrap_or(0.0);
            self.model.set_row_data(
                i,
                SpectrumBarData {
                    height: h,
                    peak: p,
                    color: self.colors[i],
                    peak_color: self.peak_colors[i],
                },
            );
        }
    }

    /// 柱数。
    pub fn bar_count(&self) -> usize {
        self.model.row_count()
    }
}
