//! 应用状态（主线程持有的顶层状态视图）。

use crate::audio::source::AudioSourceKind;
use std::path::PathBuf;

/// 主线程维护的应用状态。
///
/// 仅由 UI 线程读写；音频 / 分析线程不接触此结构，
/// 播放细节状态由 `player::Player` 的快照接口提供。
#[derive(Debug, Clone)]
pub struct AppState {
    /// 当前音频源类型。
    pub source_kind: AudioSourceKind,
    /// 频谱是否启用。
    pub spectrum_enabled: bool,
    /// 当前主题（v1 固定为 dark）。
    pub theme: String,
    /// 当前音频设备描述。
    pub device_name: String,
    /// 当前音频文件（若有）。
    pub current_file: Option<PathBuf>,
    /// 是否正在播放。
    pub is_playing: bool,
    /// 当前音量（0~1）。
    pub volume: f32,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            source_kind: AudioSourceKind::Test,
            spectrum_enabled: true,
            theme: "dark".to_string(),
            device_name: String::new(),
            current_file: None,
            is_playing: false,
            volume: 0.8,
        }
    }
}
