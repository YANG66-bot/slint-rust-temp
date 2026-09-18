//! 应用生命周期模块。
//!
//! - `application`：装配各模块并驱动事件循环
//! - `state`：主线程持有的顶层应用状态
//! - `tray`：系统托盘图标与菜单（设置面板入口）

pub mod application;
pub mod state;
pub mod tray;
