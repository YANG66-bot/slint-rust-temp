//! 用户配置模块。

pub mod settings;

pub use settings::{
    AudioSettings, DEFAULT_COLORS, PaletteStop, PanelSettings, Settings, SpectrumSettings,
    VisualSettings, VisualizerConfig, WindowKind, parse_hex_color,
};
