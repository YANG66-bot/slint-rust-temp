//! 音频输入层：负责向频谱分析器持续提供 PCM 数据。
//!
//! - [`source`]：音频源抽象（`AudioSource` trait、测试信号、系统环回预留）
//! - [`buffer`]：无锁 SPSC 环形缓冲
//! - [`engine`]：音频引擎，管理音频源生命周期
//! - [`capture`]：cpal 麦克风采集源
//! - [`resample`]：声道混缩与线性重采样工具（文件源 / 采集源共用）
//!
//! 本模块禁止直接依赖 Slint；cpal 类型不对外泄漏。

pub mod buffer;
pub mod capture;
pub mod engine;
pub mod resample;
pub mod source;

pub use buffer::PcmRingBuffer;
pub use capture::MicrophoneCapture;
pub use engine::AudioEngine;
pub use source::{AudioSource, AudioSourceKind, SystemAudioSource, TestSignalSource};
