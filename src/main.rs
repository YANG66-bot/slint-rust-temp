// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 程序入口：仅负责初始化日志、启动应用、处理顶层错误。
//! 所有业务逻辑位于库模块中。

fn main() -> anyhow::Result<()> {
    lumawave::init_logging();
    lumawave::app::application::run()
}
