//! 统一错误模块。
//!
//! 底层各模块返回 [`AppError`]（thiserror 派生，错误信息明确）；
//! 应用层（`app`）统一转换为 `anyhow::Result`。

pub mod app_error;

pub use app_error::{AppError, AppResult};
