//! 用户配置模块。

pub mod settings;

pub use settings::{
    AudioSettings, DEFAULT_COLORS, Settings, SpectrumSettings, VisualSettings, WindowKind,
    parse_hex_color,
};
