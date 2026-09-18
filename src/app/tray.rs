//! 系统托盘：图标 + 右键菜单（打开设置 / 退出）+ 双击图标打开设置。
//!
//! 事件投递：tray-icon / muda 通过全局 channel 发送事件；本模块用一个
//! [`slint::Timer`] 在 UI 线程按固定间隔排空 [`MenuEvent`] 与
//! [`TrayIconEvent`] 接收器，转成回调交给上层。返回的 [`Tray`] 需保活
//! （持有 TrayIcon、菜单项与定时器），一旦 drop 托盘即消失。

use std::time::Duration;

use slint::{Timer, TimerMode};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// 托盘事件轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// 托盘保活句柄（drop 即移除托盘与定时器）。
pub struct Tray {
    _tray: TrayIcon,
    _timer: Timer,
    // 保留菜单项句柄（其原生项已加入菜单，这里仅确保 Rust 侧不被提前回收）
    _items: (MenuItem, MenuItem),
}

/// 生成一个 32×32 的粉色圆点图标（无外部资源依赖）。
fn build_icon() -> Icon {
    const SIZE: usize = 32;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let c = SIZE as f32 / 2.0 - 0.5;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let i = (y * SIZE + x) * 4;
            if dx * dx + dy * dy <= (SIZE as f32 * 0.44).powi(2) {
                rgba[i] = 0xF8; // 粉
                rgba[i + 1] = 0xAF;
                rgba[i + 2] = 0xDB;
                rgba[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("托盘图标构建失败")
}

/// 安装托盘并注册事件轮询定时器。
///
/// - `on_settings`：点击"打开设置"或左键双击托盘图标时调用；
/// - `on_exit`：点击"退出"时调用（通常内部执行 `slint::quit_event_loop`）。
pub fn install<F, G>(on_settings: F, on_exit: G) -> anyhow::Result<Tray>
where
    F: Fn() + 'static,
    G: Fn() + 'static,
{
    let menu = Menu::new();
    let settings_item = MenuItem::with_id("settings", "打开设置", true, None);
    let quit_item = MenuItem::with_id("quit", "退出", true, None);
    let separator = PredefinedMenuItem::separator();
    menu.append(&separator)?;
    menu.append(&settings_item)?;
    menu.append(&quit_item)?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_title("音乐频谱 · Music Spectrum")
        .with_icon(build_icon())
        .build()
        .map_err(|e| anyhow::anyhow!("创建系统托盘失败: {e}"))?;

    let settings_id = settings_item.id().clone();
    let quit_id = quit_item.id().clone();
    let menu_rx = MenuEvent::receiver();
    let tray_rx = TrayIconEvent::receiver();

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, POLL_INTERVAL, move || {
        // 菜单事件
        while let Ok(ev) = menu_rx.try_recv() {
            if ev.id() == &settings_id {
                on_settings();
            } else if ev.id() == &quit_id {
                on_exit();
            }
        }
        // 托盘图标事件：左键双击打开设置
        while let Ok(ev) = tray_rx.try_recv() {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = ev
            {
                on_settings();
            }
        }
    });

    Ok(Tray {
        _tray,
        _timer: timer,
        _items: (settings_item, quit_item),
    })
}
