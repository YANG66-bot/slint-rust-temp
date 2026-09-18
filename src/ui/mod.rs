//! Slint UI 桥接模块。
//!
//! - [`bridge`]：构建窗口 / 模型 / 属性装配
//! - [`updater`]：60Hz 定时器驱动的模型增量更新（插值 + idle 动画）
//!
//! 本模块是 Rust ↔ Slint 的唯一主要边界；音频与分析线程禁止直接操作 Slint。

pub mod bridge;
pub mod updater;

pub use bridge::SpectrumBridge;
pub use updater::UiUpdater;
