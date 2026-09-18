//! 用户配置：加载、保存、校验。
//!
//! 配置文件为 JSON，位于：
//! - Windows: `%APPDATA%\music-spectrum\settings.json`
//! - macOS:   `~/Library/Application Support/music-spectrum/settings.json`
//! - Linux:   `$XDG_CONFIG_HOME/music-spectrum/settings.json`（或 `~/.config/...`）
//!
//! 所有 DSP / 视觉参数集中于此，禁止把魔法数字散落在业务代码里。

use crate::audio::source::AudioSourceKind;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 默认五色调色板：粉 → 浅粉 → 天蓝 → 浅紫 → 青（与参考视觉稿一致）。
pub const DEFAULT_COLORS: [&str; 5] = ["#F8AFDB", "#F8A3C8", "#87CEEB", "#D8BFDB", "#00FFFF"];

/// 顶层配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub audio: AudioSettings,
    pub spectrum: SpectrumSettings,
    pub visual: VisualSettings,
    /// 设置面板的完整状态（忠实持久化面板字段）。
    pub panel: PanelSettings,
}

impl Settings {
    /// 加载配置；文件不存在或非法时返回默认值（并记录警告）。
    pub fn load_or_default() -> Settings {
        match Settings::load() {
            Ok(s) => s.sanitized(),
            Err(e) => {
                tracing::warn!("加载配置失败，使用默认配置: {e}");
                Settings::default()
            }
        }
    }

    /// 从默认配置路径加载。
    fn load() -> AppResult<Settings> {
        let path = config_path()?;
        Settings::load_from(&path)
    }

    /// 从指定路径加载（供测试使用）。
    fn load_from(path: &Path) -> AppResult<Settings> {
        let raw = std::fs::read_to_string(path)?;
        let parsed: Settings = serde_json::from_str(&raw)
            .map_err(|e| AppError::ConfigError(format!("解析 {} 失败: {e}", path.display())))?;
        tracing::info!("已加载配置: {}", path.display());
        Ok(parsed)
    }

    /// 保存配置（pretty JSON）。
    pub fn save(&self) -> AppResult<()> {
        let path = config_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| AppError::ConfigError(format!("创建配置目录失败: {e}")))?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| AppError::ConfigError(format!("序列化配置失败: {e}")))?;
        std::fs::write(&path, body)
            .map_err(|e| AppError::ConfigError(format!("写入 {} 失败: {e}", path.display())))?;
        tracing::info!("配置已保存: {}", path.display());
        Ok(())
    }

    /// 对所有数值做范围校验/夹取，防止非法配置破坏 DSP 流水线。
    pub fn sanitized(self) -> Settings {
        Settings {
            audio: AudioSettings {
                volume: self.audio.volume.clamp(0.0, 1.0),
                source: self.audio.source,
            },
            spectrum: self.spectrum.sanitized(),
            visual: self.visual.sanitized(),
            panel: self.panel.sanitized(),
        }
    }
}

/// 音频相关配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// 音频源类型。
    pub source: AudioSourceKind,
    /// 播放音量（0~1）。
    pub volume: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            source: AudioSourceKind::Test,
            volume: 0.8,
        }
    }
}

/// 频谱分析配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumSettings {
    /// 频谱柱数量（参考视觉稿为 88）。
    pub bar_count: usize,
    /// FFT 尺寸（支持 1024 / 2048 / 4096）。
    pub fft_size: usize,
    /// 分析频段下限（Hz）。
    pub min_frequency: f32,
    /// 分析频段上限（Hz）。
    pub max_frequency: f32,
    /// 归一化 dB 下限。
    pub min_db: f32,
    /// 归一化 dB 上限。
    pub max_db: f32,
    /// 平滑 attack 系数（上升速度，越大越快）。
    pub attack: f32,
    /// 平滑 release 系数（下降速度，越小越慢）。
    pub release: f32,
    /// Peak 衰减系数（每帧乘法衰减）。
    pub peak_decay: f32,
    /// 频谱分析帧率（Hz）。
    pub fps: u32,
    /// 柱高增益（参考视觉稿"矩形柱增益"）。
    pub height_scale: f32,
    /// 高频倾斜补偿（dB/倍频程）：真实音乐高频能量低，适度补偿让高低频都有明显变化。
    pub tilt_db_per_octave: f32,
    /// 窗函数类型。
    pub window: WindowKind,
}

impl Default for SpectrumSettings {
    fn default() -> Self {
        Self {
            bar_count: 88,
            fft_size: 2048,
            min_frequency: 20.0,
            max_frequency: 20000.0,
            min_db: -80.0,
            max_db: 0.0,
            attack: 0.65,
            release: 0.12,
            peak_decay: 0.985,
            fps: 40,
            height_scale: 1.0,
            tilt_db_per_octave: 2.5,
            window: WindowKind::Hann,
        }
    }
}

impl SpectrumSettings {
    fn sanitized(self) -> SpectrumSettings {
        let fft_size = match self.fft_size {
            1024 | 2048 | 4096 => self.fft_size,
            _ => 2048,
        };
        let min_frequency = self.min_frequency.clamp(10.0, 1000.0);
        let max_frequency = self
            .max_frequency
            .clamp((min_frequency * 2.0).max(1000.0), 48000.0);
        let min_db = self.min_db.clamp(-120.0, -20.0);
        let max_db = self.max_db.clamp(min_db + 10.0, 6.0);
        SpectrumSettings {
            bar_count: self.bar_count.clamp(16, 192),
            fft_size,
            min_frequency,
            max_frequency,
            min_db,
            max_db,
            attack: self.attack.clamp(0.05, 1.0),
            release: self.release.clamp(0.02, 1.0),
            peak_decay: self.peak_decay.clamp(0.5, 0.9999),
            fps: self.fps.clamp(15, 120),
            height_scale: self.height_scale.clamp(0.1, 4.0),
            tilt_db_per_octave: self.tilt_db_per_octave.clamp(0.0, 8.0),
            window: self.window,
        }
    }
}

/// 窗函数类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowKind {
    Hann,
    Hamming,
    Blackman,
}

/// 视觉配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualSettings {
    /// 频谱渐变调色板（`#RRGGBB` 列表，至少 2 个）。
    pub colors: Vec<String>,
    /// 柱体不透明度。
    pub bar_opacity: f32,
    /// Peak 指示条不透明度。
    pub peak_opacity: f32,
    /// Glow 强度（0~1，轻微即可，避免霓虹爆炸感）。
    pub glow_intensity: f32,
    /// 是否启用倒影。
    pub reflection: bool,
    /// 倒影高度 / 主频谱高度。
    pub reflection_ratio: f32,
    /// 倒影不透明度。
    pub reflection_opacity: f32,
    /// 是否启用底部基线。
    pub baseline: bool,
    /// 无信号时是否启用 idle 呼吸动画。
    pub idle_animation: bool,
}

impl Default for VisualSettings {
    fn default() -> Self {
        Self {
            colors: DEFAULT_COLORS.iter().map(|s| s.to_string()).collect(),
            bar_opacity: 0.78,
            peak_opacity: 0.95,
            glow_intensity: 0.55,
            reflection: true,
            reflection_ratio: 0.30,
            reflection_opacity: 0.22,
            baseline: true,
            idle_animation: true,
        }
    }
}

impl VisualSettings {
    fn sanitized(self) -> VisualSettings {
        let colors: Vec<String> = self
            .colors
            .iter()
            .filter(|c| parse_hex_color(c).is_some())
            .cloned()
            .collect();
        let colors = if colors.len() >= 2 {
            colors
        } else {
            DEFAULT_COLORS.iter().map(|s| s.to_string()).collect()
        };
        VisualSettings {
            colors,
            bar_opacity: self.bar_opacity.clamp(0.05, 1.0),
            peak_opacity: self.peak_opacity.clamp(0.05, 1.0),
            glow_intensity: self.glow_intensity.clamp(0.0, 1.0),
            reflection: self.reflection,
            reflection_ratio: self.reflection_ratio.clamp(0.05, 0.6),
            reflection_opacity: self.reflection_opacity.clamp(0.0, 0.8),
            baseline: self.baseline,
            idle_animation: self.idle_animation,
        }
    }
}

/// 调色板停靠点：一个颜色位（启用标志 + HEX）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaletteStop {
    pub enabled: bool,
    pub hex: String,
}

/// 设置面板的完整状态（与 .slint 面板 1:1 映射）。
///
/// 面板是可持久化的 UI 真值；其中与引擎功能重叠的字段
/// （柱数 / 柱增益 / 倒影 / 基线 / 调色板）在保存时镜像写回
/// `spectrum` / `visual`，供下次启动生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSettings {
    // 位置 / 尺寸
    pub pos_x: i32,
    pub pos_y: i32,
    pub size_width: i32,
    pub size_height: i32,
    // 整体透明度
    pub master_opacity: f32,
    // 颜色渐变调色板
    pub palette: Vec<PaletteStop>,
    // 系统选项
    pub auto_start: bool,
    pub hide_on_launch: bool,
    pub click_through: bool,
    pub game_mode: bool,
    // 样式预设（0=自定义，1..5=默认样式）
    pub active_preset: i32,
    // 频谱参数
    pub bar_count: f32,
    pub bar_width: f32,
    pub bar_radius: f32,
    pub bar_gain: f32,
    pub peak_gain: f32,
    pub peak_amplitude: f32,
    pub smoothing: f32,
    // 功能开关
    pub enable_reflection: bool,
    pub enable_peak_line: bool,
}

impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            pos_x: 1280,
            pos_y: 1200,
            size_width: 1500,
            size_height: 500,
            master_opacity: 0.85,
            palette: DEFAULT_COLORS
                .iter()
                .map(|h| PaletteStop {
                    enabled: true,
                    hex: h.to_string(),
                })
                .collect(),
            auto_start: false,
            hide_on_launch: false,
            click_through: false,
            game_mode: false,
            active_preset: 0,
            bar_count: 88.0,
            bar_width: 0.61,
            bar_radius: 0.38,
            bar_gain: 1.0,
            peak_gain: 3.0,
            peak_amplitude: 0.25,
            smoothing: 0.90,
            enable_reflection: true,
            enable_peak_line: true,
        }
    }
}

impl PanelSettings {
    fn sanitized(mut self) -> PanelSettings {
        let palette: Vec<PaletteStop> = self
            .palette
            .into_iter()
            .filter(|s| parse_hex_color(&s.hex).is_some())
            .collect();
        self.palette = if palette.len() >= 2 {
            palette
        } else {
            PanelSettings::default().palette
        };
        self.master_opacity = self.master_opacity.clamp(0.05, 1.0);
        self.active_preset = self.active_preset.clamp(0, 5);
        self.bar_count = self.bar_count.clamp(16.0, 192.0);
        self.bar_width = self.bar_width.clamp(0.1, 1.0);
        self.bar_radius = self.bar_radius.clamp(0.0, 1.0);
        self.bar_gain = self.bar_gain.clamp(0.1, 5.0);
        self.peak_gain = self.peak_gain.clamp(0.1, 5.0);
        self.peak_amplitude = self.peak_amplitude.clamp(0.0, 1.0);
        self.smoothing = self.smoothing.clamp(0.0, 0.99);
        self
    }

    /// 按参数名实时更新一个浮点字段（含范围夹取）。
    /// 名称同时接受 slint kebab-case 与 rust snake_case；未知名忽略。
    pub fn set_param(&mut self, name: &str, v: f32) {
        match name {
            "master-opacity" | "master_opacity" => self.master_opacity = v.clamp(0.05, 1.0),
            "bar-count" | "bar_count" => self.bar_count = v.clamp(16.0, 192.0),
            "bar-width" | "bar_width" => self.bar_width = v.clamp(0.1, 1.0),
            "bar-radius" | "bar_radius" => self.bar_radius = v.clamp(0.0, 1.0),
            "bar-gain" | "bar_gain" => self.bar_gain = v.clamp(0.1, 5.0),
            "peak-gain" | "peak_gain" => self.peak_gain = v.clamp(0.1, 5.0),
            "peak-amplitude" | "peak_amplitude" => self.peak_amplitude = v.clamp(0.0, 1.0),
            "smoothing" => self.smoothing = v.clamp(0.0, 0.99),
            _ => {}
        }
    }
}

/// 实时渲染配置：渲染线程与 UI 刷新回路每帧读取的可视化参数。
///
/// 与持久化的 [`Settings`] 分离：`Settings` 是落盘真值（下次启动生效），
/// 而本结构是“当前正在呈现什么”的运行时快照，由设置面板回调写入、
/// 由 60Hz 刷新定时器每帧读取并作用于 Slint。包装在 `Arc<RwLock<..>>`
/// 中作为跨模块共享的单一真值（便于未来分析线程也可读取）。
#[derive(Debug, Clone)]
pub struct VisualizerConfig {
    /// 展示柱数（可能与分析器固定分辨率不同，由展示层重采样对齐）。
    pub bar_count: usize,
    /// 柱宽占槽位比例（0.1~1）。
    pub bar_width: f32,
    /// 柱圆角比例（0~1）。
    pub bar_radius: f32,
    /// 柱高增益（UI 侧乘法，分析器已预归一化到 0..1）。
    pub bar_gain: f32,
    /// 峰值增益。
    pub peak_gain: f32,
    /// 峰值幅度（0~1，映射为峰点与柱顶间距）。
    pub peak_amp: f32,
    /// 平滑系数（0~0.99，映射为插值时间常数）。
    pub smoothing: f32,
    /// 整体不透明度（0.05~1）。
    pub opacity: f32,
    /// 启用倒影。
    pub enable_reflection: bool,
    /// 启用峰值基线（横线）。
    pub enable_peak_line: bool,
    /// 调色板（启用的 HEX 列表）。
    pub colors: Vec<String>,
}

impl VisualizerConfig {
    /// 从持久化配置构建初始实时快照（面板优先，回退 visual/spectrum）。
    pub fn from_settings(s: &Settings) -> Self {
        let p = &s.panel;
        let enabled: Vec<String> = p
            .palette
            .iter()
            .filter(|x| x.enabled)
            .map(|x| x.hex.clone())
            .collect();
        let colors = if enabled.len() >= 2 {
            enabled
        } else {
            s.visual.colors.clone()
        };
        Self {
            bar_count: p.bar_count.round().clamp(16.0, 192.0) as usize,
            bar_width: p.bar_width,
            bar_radius: p.bar_radius,
            bar_gain: p.bar_gain,
            peak_gain: p.peak_gain,
            peak_amp: p.peak_amplitude,
            smoothing: p.smoothing,
            opacity: p.master_opacity,
            enable_reflection: p.enable_reflection,
            enable_peak_line: p.enable_peak_line,
            colors,
        }
    }

    /// 峰点与柱顶间距（逻辑像素）：peak_amp 0~1 → 2~16px。
    pub fn peak_gap_px(&self) -> f32 {
        2.0 + self.peak_amp.clamp(0.0, 1.0) * 14.0
    }

    /// 插值时间常数（秒）：smoothing 0~0.99 → 0.006~0.22s。
    pub fn time_constant(&self) -> f32 {
        0.006 + self.smoothing.clamp(0.0, 0.99) * 0.22
    }

    /// 按参数名实时应用一个滑条值（含范围夹取）。
    /// 名称同时接受 slint kebab-case 与 rust snake_case；未知名忽略。
    pub fn set_param(&mut self, name: &str, v: f32) {
        match name {
            "master-opacity" | "master_opacity" => self.opacity = v.clamp(0.05, 1.0),
            "bar-count" | "bar_count" => self.bar_count = v.clamp(16.0, 192.0).round() as usize,
            "bar-width" | "bar_width" => self.bar_width = v.clamp(0.1, 1.0),
            "bar-radius" | "bar_radius" => self.bar_radius = v.clamp(0.0, 1.0),
            "bar-gain" | "bar_gain" => self.bar_gain = v.clamp(0.1, 5.0),
            "peak-gain" | "peak_gain" => self.peak_gain = v.clamp(0.1, 5.0),
            "peak-amplitude" | "peak_amplitude" => self.peak_amp = v.clamp(0.0, 1.0),
            "smoothing" => self.smoothing = v.clamp(0.0, 0.99),
            _ => {}
        }
    }

    /// 按参数名实时应用一个开关（倒影 / 横线）。
    pub fn set_toggle(&mut self, name: &str, on: bool) {
        match name {
            "enable-reflection" | "enable_reflection" => self.enable_reflection = on,
            "enable-peak-line" | "enable_peak_line" => self.enable_peak_line = on,
            _ => {}
        }
    }
}

/// 解析 `#RRGGBB`（或 `RRGGBB`）十六进制颜色，返回 `[r, g, b]`。
pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let s = value.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// 配置目录（平台相关）。
fn config_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|v| PathBuf::from(v).join("music-spectrum"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|v| {
            PathBuf::from(v)
                .join("Library")
                .join("Application Support")
                .join("music-spectrum")
        })
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("music-spectrum"))
    }
}

/// 配置文件完整路径。
fn config_path() -> AppResult<PathBuf> {
    config_dir()
        .map(|d| d.join("settings.json"))
        .ok_or_else(|| AppError::ConfigError("无法确定配置目录".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.spectrum.bar_count, 88);
        assert_eq!(s.spectrum.fft_size, 2048);
        assert_eq!(s.spectrum.window, WindowKind::Hann);
        assert_eq!(s.visual.colors.len(), 5);
        assert!(s.visual.reflection && s.visual.baseline);
        assert!((s.audio.volume - 0.8).abs() < 1e-6);
    }

    #[test]
    fn json_roundtrip() {
        let s = Settings::default();
        let text = serde_json::to_string(&s).expect("serialize");
        let back: Settings = serde_json::from_str(&text).expect("deserialize");
        let text2 = serde_json::to_string(&back).expect("serialize2");
        assert_eq!(text, text2);
    }

    #[test]
    fn hex_color_parsing() {
        assert_eq!(parse_hex_color("#F8AFDB"), Some([0xF8, 0xAF, 0xDB]));
        assert_eq!(parse_hex_color("00FFFF"), Some([0x00, 0xFF, 0xFF]));
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("#GGGGGG"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn sanitize_clamps_out_of_range_values() {
        let s = Settings {
            spectrum: SpectrumSettings {
                bar_count: 9999,
                fft_size: 1234,
                min_frequency: -5.0,
                max_frequency: 999999.0,
                min_db: 0.0,
                max_db: -100.0,
                attack: 5.0,
                release: -1.0,
                peak_decay: 2.0,
                fps: 3,
                height_scale: 100.0,
                tilt_db_per_octave: -3.0,
                window: WindowKind::Blackman,
            },
            ..Settings::default()
        };
        let t = s.sanitized();
        assert_eq!(t.spectrum.bar_count, 192);
        assert_eq!(t.spectrum.fft_size, 2048);
        assert!(t.spectrum.min_frequency >= 10.0);
        assert!(t.spectrum.max_frequency <= 48000.0);
        assert!(t.spectrum.max_frequency > t.spectrum.min_frequency);
        assert!(t.spectrum.min_db < t.spectrum.max_db);
        assert!(t.spectrum.attack <= 1.0);
        assert!(t.spectrum.release >= 0.02);
        assert!(t.spectrum.peak_decay < 1.0);
        assert_eq!(t.spectrum.fps, 15);
        assert!(t.spectrum.height_scale <= 4.0);
        assert!(t.spectrum.tilt_db_per_octave >= 0.0);
    }

    #[test]
    fn sanitize_bad_colors_falls_back_to_default() {
        let s = Settings {
            visual: VisualSettings {
                colors: vec!["nope".to_string(), "#12345".to_string()],
                ..VisualSettings::default()
            },
            ..Settings::default()
        };
        let t = s.sanitized();
        assert_eq!(t.visual.colors.len(), 5);
        assert!(t.visual.colors.iter().all(|c| parse_hex_color(c).is_some()));
    }

    #[test]
    fn load_missing_file_returns_error() {
        let path = std::env::temp_dir().join("music_spectrum_missing_settings.json");
        let result = Settings::load_from(&path);
        assert!(result.is_err());
    }
}
