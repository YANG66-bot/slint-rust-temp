//! 应用装配与生命周期。
//!
//! 启动顺序（单向数据流）：
//! 1. 加载配置并夹取非法值
//! 2. 音频层：共享环形缓冲 + 音频引擎（按配置选默认源，不可用则回退测试信号）
//! 3. 分析层：40Hz 频谱分析线程（owned 帧经 channel 输出）
//! 4. UI 层：AppWindow + 频谱模型桥 + 60Hz 刷新定时器
//! 5. 事件循环运行；退出时保存配置
//!
//! 音频/分析线程绝不接触 Slint；UI 线程通过 channel 拉取帧数据。
//!
//! 纯频谱挂件形态：UI 不含任何播放控件；文件/麦克风播放能力保留在
//! `player` 模块与音频源配置（`audio.source`）中，可经配置文件启用。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use slint::ComponentHandle;

use crate::AppWindow;
use crate::app::tray;
use crate::audio::SystemAudioSource;
use crate::audio::buffer::PcmRingBuffer;
use crate::audio::engine::AudioEngine;
use crate::audio::source::{AudioSource, AudioSourceKind, TestSignalSource};
use crate::config::{Settings, SpectrumSettings, VisualizerConfig};
use crate::spectrum::AnalyzerHandle;
use crate::ui::{SettingsWindow, SpectrumBridge, UiUpdater};
use crate::visualizer::{Palette, VisualizerState};

/// 依据配置构建默认音频源：系统音频环回优先，不可用时回退测试信号。
///
/// 挂件形态不提供播放控制 UI，`File` / `Microphone` 配置不再生效：
/// 统一捕获本机播放器正在输出的声音（WASAPI loopback），
/// 环回不可用（无输出设备 / COM 失败）时回退测试信号保证可运行。
fn build_default_source(
    settings: &Settings,
    ring: &Arc<PcmRingBuffer>,
) -> (Box<dyn AudioSource>, AudioSourceKind) {
    // source 字段保留序列化兼容，挂件形态不据此选源
    let _ = settings.audio.source;
    let test = || Box::new(TestSignalSource::new(Arc::clone(ring))) as Box<dyn AudioSource>;
    match SystemAudioSource::new(Arc::clone(ring)) {
        Ok(source) => (Box::new(source), AudioSourceKind::System),
        Err(e) => {
            tracing::warn!("系统音频环回不可用（{e}），回退测试信号");
            (test(), AudioSourceKind::Test)
        }
    }
}

/// 启动应用：装配各层 → 事件循环 → 退出时保存配置。
pub fn run() -> anyhow::Result<()> {
    let settings = Rc::new(RefCell::new(Settings::load_or_default().sanitized()));
    // 启动只读快照：分析器固定分辨率（面板改柱数时展示层重采样，无需重启分析）
    let spectrum = settings.borrow().spectrum.clone();
    let idle_animation = settings.borrow().visual.idle_animation;
    // 实时可视化配置：设置面板写、渲染回路每帧读（跨模块共享单一真值）
    let live = Arc::new(RwLock::new(VisualizerConfig::from_settings(
        &settings.borrow(),
    )));
    let init = live.read().unwrap().clone();
    tracing::info!(
        "Music Spectrum 启动 | 展示柱数={} 分析分辨率={} FFT={} 帧率={}Hz 音源=系统音频环回",
        init.bar_count,
        spectrum.bar_count,
        spectrum.fft_size,
        spectrum.fps
    );

    // ---------- 音频层 ----------
    let ring = Arc::new(PcmRingBuffer::new(spectrum.fft_size * 4));
    let mut engine = AudioEngine::new(Arc::clone(&ring));
    let (source, active_kind) = build_default_source(&settings.borrow(), &ring);
    engine
        .switch_source(source)
        .map_err(|e| anyhow::anyhow!("启动音频源失败: {e}"))?;
    let (sample_rate, _channels) = engine.source_format().unwrap_or((44100, 1));
    tracing::info!("音频源就绪: {}", engine.source_description());

    // ---------- 分析层（固定分辨率，与展示柱数解耦） ----------
    let analyzer = spawn_analyzer(&spectrum, sample_rate, &ring)?;

    // ---------- UI 层 ----------
    let window = AppWindow::new().map_err(|e| anyhow::anyhow!("创建窗口失败: {e}"))?;
    // 初始桥 / 模型按实时配置的柱数与调色板构建；后续由定时器按配置增量更新
    let mut bridge = SpectrumBridge::new(init.bar_count, &Palette::from_hex(&init.colors));
    window.set_bars(bridge.model().clone().into());
    window.set_bar_width_ratio(init.bar_width);
    window.set_bar_roundness(init.bar_radius);
    window.set_master_opacity(init.opacity);
    window.set_peak_gap(init.peak_gap_px());
    window.set_show_reflection(init.enable_reflection);
    window.set_show_baseline(init.enable_peak_line);

    let visual_state = Rc::new(RefCell::new({
        let mut s = VisualizerState::new(init.bar_count);
        s.set_idle_enabled(idle_animation);
        s
    }));
    let analyzer = Rc::new(analyzer);
    let last_tick = Rc::new(Cell::new(Instant::now()));
    let window_weak = window.as_weak();
    let tick_state = Rc::clone(&visual_state);
    let tick_last = Rc::clone(&last_tick);
    let tick_analyzer = Rc::clone(&analyzer);
    let tick_live = Arc::clone(&live);
    // 缓存上一帧已应用的配置，仅在变化时写窗口属性 / 重建模型，避免无谓重排
    let mut applied = init.clone();
    let mut last_colors = init.colors.clone();
    let ui_updater = UiUpdater::start(Duration::from_millis(16), move || {
        let cfg = tick_live
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| applied.clone());
        if let Some(w) = window_weak.upgrade() {
            if cfg.bar_width != applied.bar_width {
                w.set_bar_width_ratio(cfg.bar_width);
            }
            if cfg.bar_radius != applied.bar_radius {
                w.set_bar_roundness(cfg.bar_radius);
            }
            if cfg.opacity != applied.opacity {
                w.set_master_opacity(cfg.opacity);
            }
            if cfg.peak_amp != applied.peak_amp {
                w.set_peak_gap(cfg.peak_gap_px());
            }
            if cfg.enable_reflection != applied.enable_reflection {
                w.set_show_reflection(cfg.enable_reflection);
            }
            if cfg.enable_peak_line != applied.enable_peak_line {
                w.set_show_baseline(cfg.enable_peak_line);
            }
            // 柱数变化：重建桥 + 重设模型 + 缩放展示状态；否则调色板变化仅重算颜色
            if cfg.bar_count != bridge.bar_count() {
                bridge = SpectrumBridge::new(cfg.bar_count, &Palette::from_hex(&cfg.colors));
                tick_state.borrow_mut().resize(cfg.bar_count);
                w.set_bars(bridge.model().clone().into());
                last_colors = cfg.colors.clone();
            } else if cfg.colors != last_colors {
                bridge.apply_palette(&Palette::from_hex(&cfg.colors));
                last_colors = cfg.colors.clone();
            }
        }
        applied = cfg.clone();
        // 非阻塞拉取最新分析帧（push_frame 内部按展示柱数重采样）
        if let Some(frame) = tick_analyzer.latest_frame() {
            tick_state.borrow_mut().push_frame(&frame);
        }
        // 平滑 → 插值时间常数；帧率无关插值 + 增益/AGC 映射写模型
        let now = Instant::now();
        let dt = now.duration_since(tick_last.get()).as_secs_f32();
        tick_last.set(now);
        {
            let mut state = tick_state.borrow_mut();
            state.set_time_constant(cfg.time_constant());
            state.tick(dt);
        }
        bridge.update(&tick_state.borrow(), cfg.bar_gain, cfg.peak_gain, dt);
    });

    // ---------- 设置面板 + 系统托盘 ----------
    let settings_win = Rc::new(RefCell::new(
        SettingsWindow::new(Rc::clone(&settings), Arc::clone(&live))
            .map_err(|e| anyhow::anyhow!("创建设置窗口失败: {e}"))?,
    ));
    let tray = tray::install(
        {
            let win = Rc::clone(&settings_win);
            move || win.borrow().show()
        },
        || {
            let _ = slint::quit_event_loop();
        },
    )?;
    // 未勾选“启动时隐藏设置窗口” → 启动即弹出面板
    if !settings.borrow().panel.hide_on_launch {
        settings_win.borrow().show();
    }

    // ---------- 事件循环 ----------
    let run_result = window.run();

    // 退出清理：先停 UI 定时器与窗口，再停分析线程，最后停音频源
    // （engine 最后 drop → AudioEngine Drop 停源）
    drop(tray);
    drop(settings_win);
    drop(ui_updater);
    drop(window);
    drop(analyzer);
    drop(engine);

    if let Err(e) = run_result {
        return Err(anyhow::anyhow!("事件循环异常退出: {e}"));
    }
    // 把本次会话实际使用的音源写回配置并落盘
    {
        let mut s = settings.borrow_mut();
        s.audio.source = active_kind;
        if let Err(e) = s.save() {
            tracing::warn!("退出时保存配置失败: {e}");
        }
    }
    Ok(())
}

/// 启动频谱分析线程。
fn spawn_analyzer(
    spectrum: &SpectrumSettings,
    sample_rate: u32,
    ring: &Arc<PcmRingBuffer>,
) -> anyhow::Result<AnalyzerHandle> {
    AnalyzerHandle::spawn(Arc::clone(ring), spectrum, sample_rate)
        .map_err(|e| anyhow::anyhow!("启动频谱分析线程失败: {e}"))
}
