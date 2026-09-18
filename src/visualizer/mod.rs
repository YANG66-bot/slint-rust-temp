//! 频谱视觉层（不依赖 Slint）。
//!
//! - [`state`]：分析帧 → 展示值的 60Hz 插值状态
//! - [`palette`]：五色渐变 → 逐柱颜色映射
//!
//! Slint 侧的数据装配由 `ui::bridge` 完成，本模块只产出
//! 纯 Rust 数据（高度 / 峰值 / RGB），保持渲染细节可测试。

pub mod palette;
pub mod state;

pub use palette::Palette;
pub use state::VisualizerState;
