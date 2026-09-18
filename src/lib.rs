//! music-spectrum：基于 Rust + Slint 的桌面实时音乐频谱可视化应用。
//!
//! 模块划分（职责隔离）：
//! - [`error`]：统一错误类型（thiserror）
//! - [`config`]：用户配置（加载/保存/校验）
//! - [`audio`]：音频输入层（音频源抽象、无锁环形缓冲、采集与测试信号）
//! - [`player`]：音频播放（解码、播放核心、播放列表）
//! - [`spectrum`]：频谱分析（FFT、频段映射、归一化、平滑、Peak Hold）
//! - [`visualizer`]：频谱视觉状态（纯 Rust 状态模型、调色、动画）
//! - [`ui`]：Slint 桥接（UI Bridge 是 Rust ↔ Slint 的唯一主要边界）
//! - [`app`]：应用生命周期与装配
//!
//! 线程模型：音频生产者线程 / 频谱分析线程 / Slint UI 线程三者通过
//! 无锁环形缓冲与 channel 通信，音频线程禁止直接操作 Slint。

slint::include_modules!();

pub mod app;
pub mod audio;
pub mod config;
pub mod error;
pub mod player;
pub mod spectrum;
pub mod ui;
pub mod visualizer;

/// 初始化 tracing 日志系统。
///
/// 默认级别 `info`，可通过 `RUST_LOG` 环境变量覆盖
/// （例如 `RUST_LOG=music_spectrum=debug`）。
pub fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
