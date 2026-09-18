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
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

use crate::AppWindow;
use crate::audio::MicrophoneCapture;
use crate::audio::buffer::PcmRingBuffer;
use crate::audio::engine::AudioEngine;
use crate::audio::source::{AudioSource, AudioSourceKind, TestSignalSource};
use crate::config::{Settings, SpectrumSettings};
use crate::spectrum::AnalyzerHandle;
use crate::ui::{SpectrumBridge, UiUpdater};
use crate::visualizer::{Palette, VisualizerState};

/// 依据配置构建默认音频源，返回（源，实际生效的音源类型）。
///
/// 实际生效类型可能因回退而与配置不同：配置为 `File` 但挂件形态
/// 不加载文件、麦克风不可用、系统环回不支持时，均回退测试信号，
/// 保证"无音频设备/未实现能力时仍能运行"。
fn build_default_source(
    settings: &Settings,
    ring: &Arc<PcmRingBuffer>,
) -> (Box<dyn AudioSource>, AudioSourceKind) {
    let test = || Box::new(TestSignalSource::new(Arc::clone(ring))) as Box<dyn AudioSource>;
    match settings.audio.source {
        AudioSourceKind::Test => (test(), AudioSourceKind::Test),
        AudioSourceKind::File => {
            tracing::info!("挂件形态不加载音频文件，使用测试信号");
            (test(), AudioSourceKind::Test)
        }
        AudioSourceKind::Microphone => match MicrophoneCapture::new(Arc::clone(ring), None) {
            Ok(capture) => (Box::new(capture), AudioSourceKind::Microphone),
            Err(e) => {
                tracing::warn!("麦克风不可用（{e}），回退测试信号");
                (test(), AudioSourceKind::Test)
            }
        },
        AudioSourceKind::System => {
            tracing::warn!("系统音频环回不可用，回退测试信号");
            (test(), AudioSourceKind::Test)
        }
    }
}

/// 启动应用：装配各层 → 事件循环 → 退出时保存配置。
pub fn run() -> anyhow::Result<()> {
    let mut settings = Settings::load_or_default().sanitized();
    tracing::info!(
        "Music Spectrum 启动 | 柱数={} FFT={} 分析帧率={}Hz 音源={}",
        settings.spectrum.bar_count,
        settings.spectrum.fft_size,
        settings.spectrum.fps,
        settings.audio.source
    );

    // ---------- 音频层 ----------
    let ring = Arc::new(PcmRingBuffer::new(settings.spectrum.fft_size * 4));
    let mut engine = AudioEngine::new(Arc::clone(&ring));
    let (source, active_kind) = build_default_source(&settings, &ring);
    engine
        .switch_source(source)
        .map_err(|e| anyhow::anyhow!("启动音频源失败: {e}"))?;
    let (sample_rate, _channels) = engine.source_format().unwrap_or((44100, 1));
    tracing::info!("音频源就绪: {}", engine.source_description());

    // ---------- 分析层 ----------
    let analyzer = spawn_analyzer(&settings.spectrum, sample_rate, &ring)?;

    // ---------- UI 层 ----------
    let window = AppWindow::new().map_err(|e| anyhow::anyhow!("创建窗口失败: {e}"))?;
    let palette = Palette::from_hex(&settings.visual.colors);
    let bridge = SpectrumBridge::new(settings.spectrum.bar_count, &palette);
    // 频谱模型交给窗口（后续每次刷新由 bridge 增量写入同一模型）
    window.set_bars(bridge.model().clone().into());
    // 视觉开关（倒影 / 基线）来自配置
    window.set_show_reflection(settings.visual.reflection);
    window.set_show_baseline(settings.visual.baseline);

    // 展示状态 + 60Hz 刷新定时器（全部运行在 UI 线程）
    let visual_state = Rc::new(RefCell::new({
        let mut s = VisualizerState::new(settings.spectrum.bar_count);
        s.set_idle_enabled(settings.visual.idle_animation);
        s
    }));
    let analyzer = Rc::new(analyzer);
    let last_tick = Rc::new(Cell::new(Instant::now()));
    let tick_state = Rc::clone(&visual_state);
    let tick_last = Rc::clone(&last_tick);
    let tick_analyzer = Rc::clone(&analyzer);
    let ui_updater = UiUpdater::start(Duration::from_millis(16), move || {
        let mut state = tick_state.borrow_mut();
        // 非阻塞拉取最新分析帧（排空积压，只保留最新）
        if let Some(frame) = tick_analyzer.latest_frame() {
            state.push_frame(&frame);
        }
        // 帧率无关插值 + 模型增量写入
        let now = Instant::now();
        let dt = now.duration_since(tick_last.get()).as_secs_f32();
        tick_last.set(now);
        state.tick(dt);
        bridge.update(&state);
    });

    // ---------- 事件循环 ----------
    let run_result = window.run();

    // 退出清理：先停 UI 定时器与窗口，再停分析线程，最后停音频源
    // （engine 最后 drop → AudioEngine Drop 停源）
    drop(ui_updater);
    drop(window);
    drop(analyzer);
    drop(engine);

    if let Err(e) = run_result {
        return Err(anyhow::anyhow!("事件循环异常退出: {e}"));
    }
    // 把本次会话实际使用的音源写回配置
    settings.audio.source = active_kind;
    if let Err(e) = settings.save() {
        tracing::warn!("退出时保存配置失败: {e}");
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
