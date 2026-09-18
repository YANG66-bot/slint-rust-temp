//! 设置面板控制器：SettingsPanel ↔ config::Settings 的双向绑定与落盘。
//!
//! 职责：
//! - 打开时把持久化的 `Settings.panel` 填充进面板各属性 / 调色板模型；
//! - 保存时读回面板值 → 写 `Settings.panel`，并把与引擎功能重叠的字段
//!   （柱数 / 柱增益 / 倒影 / 基线 / 调色板）镜像进 `spectrum` / `visual`，
//!   随后 `save()` 落盘（下次启动生效）；
//! - 恢复默认把面板重置为 `PanelSettings::default()`；取消则隐藏窗口。
//!
//! 仅在 UI 线程访问（Slint 回调都运行在事件循环线程）。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::ColorStop;
use crate::SettingsPanel;
use crate::config::{PaletteStop, PanelSettings, Settings, VisualizerConfig, parse_hex_color};

/// HEX 字符串 → Slint 颜色（非法值回落白色）。
fn hex_to_color(hex: &str) -> Color {
    match parse_hex_color(hex) {
        Some([r, g, b]) => Color::from_rgb_u8(r, g, b),
        None => Color::from_rgb_u8(255, 255, 255),
    }
}

/// 面板 → PanelSettings（读取全部可编辑字段）。
fn read_panel(panel: &SettingsPanel, palette: &Rc<VecModel<ColorStop>>) -> PanelSettings {
    let stops = (0..palette.row_count())
        .filter_map(|i| palette.row_data(i))
        .map(|s| PaletteStop {
            enabled: s.enabled,
            hex: s.hex.to_string(),
        })
        .collect();
    PanelSettings {
        pos_x: panel.get_pos_x(),
        pos_y: panel.get_pos_y(),
        size_width: panel.get_size_width(),
        size_height: panel.get_size_height(),
        master_opacity: panel.get_master_opacity(),
        palette: stops,
        auto_start: panel.get_auto_start(),
        hide_on_launch: panel.get_hide_on_launch(),
        click_through: panel.get_click_through(),
        game_mode: panel.get_game_mode(),
        active_preset: panel.get_active_preset(),
        bar_count: panel.get_bar_count(),
        bar_width: panel.get_bar_width(),
        bar_radius: panel.get_bar_radius(),
        bar_gain: panel.get_bar_gain(),
        peak_gain: panel.get_peak_gain(),
        peak_amplitude: panel.get_peak_amplitude(),
        smoothing: panel.get_smoothing(),
        enable_reflection: panel.get_enable_reflection(),
        enable_peak_line: panel.get_enable_peak_line(),
    }
}

/// PanelSettings → 面板（写入标量属性并刷新调色板模型）。
fn write_panel(panel: &SettingsPanel, palette: &Rc<VecModel<ColorStop>>, p: &PanelSettings) {
    panel.set_pos_x(p.pos_x);
    panel.set_pos_y(p.pos_y);
    panel.set_size_width(p.size_width);
    panel.set_size_height(p.size_height);
    panel.set_master_opacity(p.master_opacity);
    panel.set_auto_start(p.auto_start);
    panel.set_hide_on_launch(p.hide_on_launch);
    panel.set_click_through(p.click_through);
    panel.set_game_mode(p.game_mode);
    panel.set_active_preset(p.active_preset);
    panel.set_bar_count(p.bar_count);
    panel.set_bar_width(p.bar_width);
    panel.set_bar_radius(p.bar_radius);
    panel.set_bar_gain(p.bar_gain);
    panel.set_peak_gain(p.peak_gain);
    panel.set_peak_amplitude(p.peak_amplitude);
    panel.set_smoothing(p.smoothing);
    panel.set_enable_reflection(p.enable_reflection);
    panel.set_enable_peak_line(p.enable_peak_line);

    let stops: Vec<ColorStop> = p
        .palette
        .iter()
        .map(|s| ColorStop {
            enabled: s.enabled,
            hex: SharedString::from(s.hex.as_str()),
            color: hex_to_color(&s.hex),
        })
        .collect();
    palette.set_vec(stops);
}

/// 把面板中与引擎重叠的字段镜像进 spectrum / visual。
fn mirror_into_settings(s: &mut Settings, p: &PanelSettings) {
    s.spectrum.bar_count = p.bar_count.round().clamp(16.0, 192.0) as usize;
    s.spectrum.height_scale = p.bar_gain;
    s.visual.reflection = p.enable_reflection;
    s.visual.baseline = p.enable_peak_line;
    let colors: Vec<String> = p
        .palette
        .iter()
        .filter(|x| x.enabled)
        .map(|x| x.hex.clone())
        .collect();
    if colors.len() >= 2 {
        s.visual.colors = colors;
    }
}

/// 设置面板窗口控制器（持有窗口强句柄，保活整个生命周期）。
pub struct SettingsWindow {
    panel: SettingsPanel,
    /// 颜色选择器结果取回轮询定时器（需保活，随控制器一同销毁）
    _picker_poll: Timer,
}

/// 从面板当前状态构建一份实时配置快照。
fn live_snapshot(panel: &SettingsPanel, palette: &Rc<VecModel<ColorStop>>) -> VisualizerConfig {
    let colors: Vec<String> = (0..palette.row_count())
        .filter_map(|i| palette.row_data(i))
        .filter(|s| s.enabled)
        .map(|s| s.hex.to_string())
        .collect();
    let base = VisualizerConfig::from_settings(&Settings::default());
    VisualizerConfig {
        bar_count: panel.get_bar_count().round().clamp(16.0, 192.0) as usize,
        bar_width: panel.get_bar_width(),
        bar_radius: panel.get_bar_radius(),
        bar_gain: panel.get_bar_gain(),
        peak_gain: panel.get_peak_gain(),
        peak_amp: panel.get_peak_amplitude(),
        smoothing: panel.get_smoothing(),
        opacity: panel.get_master_opacity(),
        enable_reflection: panel.get_enable_reflection(),
        enable_peak_line: panel.get_enable_peak_line(),
        colors: if colors.len() >= 2 {
            colors
        } else {
            base.colors
        },
    }
}

/// 把一份快照写入共享实时配置（忽略中毒的锁）。
fn publish(live: &Arc<RwLock<VisualizerConfig>>, cfg: VisualizerConfig) {
    if let Ok(mut g) = live.write() {
        *g = cfg;
    }
}

impl SettingsWindow {
    /// 创建并装配面板（填充当前配置 + 注册回调）。
    ///
    /// `live` 为与渲染回路共享的实时配置：滑条 / 开关 / 调色板变更时
    /// 写回该共享状态，下一帧即作用于频谱渲染。
    pub fn new(
        settings: Rc<RefCell<Settings>>,
        live: Arc<RwLock<VisualizerConfig>>,
    ) -> Result<Self, slint::PlatformError> {
        let panel = SettingsPanel::new()?;
        let palette = Rc::new(VecModel::<ColorStop>::default());
        let palette_rc: Rc<VecModel<ColorStop>> = palette.clone();
        let dyn_model: Rc<dyn slint::Model<Data = ColorStop>> = palette_rc;
        panel.set_palette_stops(ModelRc::from(dyn_model));

        // 初始填充 + 发布实时快照
        let initial = settings.borrow().panel.clone();
        write_panel(&panel, &palette, &initial);
        publish(&live, live_snapshot(&panel, &palette));

        // 保存：读回 → 写 panel + 镜像 → 落盘 → 同步实时 → 隐藏
        {
            let weak = panel.as_weak();
            let palette = Rc::clone(&palette);
            let settings = Rc::clone(&settings);
            let live = Arc::clone(&live);
            panel.on_save_settings(move || {
                let Some(panel) = weak.upgrade() else { return };
                let p = read_panel(&panel, &palette);
                {
                    let mut s = settings.borrow_mut();
                    s.panel = p.clone();
                    mirror_into_settings(&mut s, &p);
                    match s.save() {
                        Ok(()) => tracing::info!("设置已保存，下次启动生效"),
                        Err(e) => tracing::warn!("保存设置失败: {e}"),
                    }
                }
                publish(&live, live_snapshot(&panel, &palette));
                if let Err(e) = panel.hide() {
                    tracing::debug!("隐藏设置窗口失败: {e}");
                }
            });
        }

        // 恢复默认：面板重置 → 发布实时快照（不落盘，仍需点保存）
        {
            let weak = panel.as_weak();
            let palette = Rc::clone(&palette);
            let live = Arc::clone(&live);
            panel.on_restore_defaults(move || {
                let Some(panel) = weak.upgrade() else { return };
                write_panel(&panel, &palette, &PanelSettings::default());
                publish(&live, live_snapshot(&panel, &palette));
            });
        }

        // 取消：回退到最近一次保存的面板状态与实时配置，隐藏不保存
        {
            let weak = panel.as_weak();
            let palette = Rc::clone(&palette);
            let settings = Rc::clone(&settings);
            let live = Arc::clone(&live);
            panel.on_cancel_settings(move || {
                let Some(panel) = weak.upgrade() else { return };
                let saved = settings.borrow().panel.clone();
                write_panel(&panel, &palette, &saved);
                publish(&live, live_snapshot(&panel, &palette));
                if let Err(e) = panel.hide() {
                    tracing::debug!("隐藏设置窗口失败: {e}");
                }
            });
        }

        // 调色板某行编辑：更新模型行（启用标志 / HEX / 预览色）+ 实时刷新共享颜色
        {
            let palette = Rc::clone(&palette);
            let live = Arc::clone(&live);
            panel.on_palette_changed(move |idx, enabled, hex| {
                let i = idx.max(0) as usize;
                if let Some(mut row) = palette.row_data(i) {
                    row.enabled = enabled;
                    row.hex = hex.clone();
                    row.color = hex_to_color(hex.as_str());
                    palette.set_row_data(i, row);
                }
                if let Ok(mut g) = live.write() {
                    let colors: Vec<String> = (0..palette.row_count())
                        .filter_map(|k| palette.row_data(k))
                        .filter(|s| s.enabled)
                        .map(|s| s.hex.to_string())
                        .collect();
                    if colors.len() >= 2 {
                        g.colors = colors;
                    }
                }
            });
        }

        // 滑条实时变更（拖动 / 释放）：非阻塞写入共享配置，下一帧生效
        {
            let live = Arc::clone(&live);
            panel.on_parameter_changed(move |name, v| {
                if let Ok(mut g) = live.write() {
                    g.set_param(name.as_str(), v);
                }
                tracing::debug!("参数实时变更: {name} = {v}");
            });
        }

        // 开关实时变更（倒影 / 横线）：写入共享配置
        {
            let live = Arc::clone(&live);
            panel.on_toggle_changed(move |name, on| {
                if let Ok(mut g) = live.write() {
                    g.set_toggle(name.as_str(), on);
                }
                tracing::debug!("开关实时变更: {name} = {on}");
            });
        }

        // 点击颜色预览块 → 弹原生颜色选择器（Win32 ChooseColor）。
        // 模态对话框必须离开 UI 线程：否则事件循环（含频谱 tick）会被整个
        // 阻塞，表现为界面卡死且对话框迟迟不弹。结果经共享队列由下方
        // 轮询 Timer 送回 UI 线程 → color-updated → palette-changed → 模型 + 实时配置。
        let picker_queue: Arc<Mutex<Vec<(i32, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let picker_busy = Arc::new(AtomicBool::new(false));
        {
            let queue = Arc::clone(&picker_queue);
            let busy = Arc::clone(&picker_busy);
            panel.on_request_color_picker(move |idx, hex| {
                // 已有选择器打开时忽略重复请求（工作线程对话框无父窗口，
                // 不阻塞本线程，连点会叠开多个）
                if busy.swap(true, Ordering::SeqCst) {
                    return;
                }
                let i = idx.max(0);
                let hex = hex.to_string();
                let q = Arc::clone(&queue);
                let b = Arc::clone(&busy);
                std::thread::spawn(move || {
                    // RAII：无论正常/异常退出都释放占用标志
                    struct BusyGuard<'a>(&'a AtomicBool);
                    impl Drop for BusyGuard<'_> {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::SeqCst);
                        }
                    }
                    let _guard = BusyGuard(&b);

                    let [r, g, bl] = parse_hex_color(&hex).unwrap_or([255, 255, 255]);
                    let picked = rustydialogs::ColorPicker {
                        title: "选择颜色 · Pick a Color",
                        value: rustydialogs::ColorValue {
                            red: r,
                            green: g,
                            blue: bl,
                        },
                        owner: None,
                    }
                    .show();
                    if let Some(c) = picked {
                        let new_hex = format!("#{:02X}{:02X}{:02X}", c.red, c.green, c.blue);
                        tracing::debug!("颜色选择: 行 {i} → {new_hex}");
                        if let Ok(mut v) = q.lock() {
                            v.push((i, new_hex));
                        }
                    }
                });
            });
        }

        // 低频轮询取回工作线程的颜色选择结果（队列为空时仅一次加锁判断）：
        // 收到后回到 UI 线程走既有 color-updated 链路应用到模型与实时配置。
        let picker_poll = Timer::default();
        {
            let weak = panel.as_weak();
            let queue = Arc::clone(&picker_queue);
            picker_poll.start(
                TimerMode::Repeated,
                std::time::Duration::from_millis(80),
                move || {
                    let items: Vec<(i32, String)> = {
                        let Ok(mut q) = queue.lock() else { return };
                        if q.is_empty() {
                            return;
                        }
                        std::mem::take(&mut *q)
                    };
                    let Some(panel) = weak.upgrade() else { return };
                    for (i, hex) in items {
                        panel.invoke_color_updated(i, SharedString::from(hex.as_str()));
                    }
                },
            );
        }

        Ok(Self {
            panel,
            _picker_poll: picker_poll,
        })
    }

    /// 显示（前置）设置面板。
    pub fn show(&self) {
        if let Err(e) = self.panel.show() {
            tracing::warn!("显示设置窗口失败: {e}");
        }
    }
}
