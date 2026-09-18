//! 应用统一错误类型。

use thiserror::Error;

/// 底层模块共用的错误类型。
#[derive(Debug, Error)]
pub enum AppError {
    /// 找不到可用的音频设备。
    #[error("audio device not found: {0}")]
    AudioDeviceNotFound(String),

    /// 音频流创建或运行失败。
    #[error("audio stream error: {0}")]
    AudioStreamError(String),

    /// 不支持或不可用的音频格式/能力。
    #[error("unsupported audio format: {0}")]
    UnsupportedFormat(String),

    /// 音频文件解码失败。
    #[error("decoder error: {0}")]
    DecoderError(String),

    /// FFT 计算错误（如输入长度与 FFT 尺寸不匹配）。
    #[error("fft error: {0}")]
    FftError(String),

    /// 配置读写或校验失败。
    #[error("config error: {0}")]
    ConfigError(String),

    /// UI / 桥接层错误。
    #[error("ui error: {0}")]
    UiError(String),

    /// IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 底层模块统一 Result 别名。
pub type AppResult<T> = Result<T, AppError>;
