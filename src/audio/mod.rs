//! 音频输入层：负责向频谱分析器持续提供 PCM 数据。
//!
//! - [`source`]：音频源抽象（`AudioSource` trait、测试信号）
//! - [`buffer`]：无锁 SPSC 环形缓冲
//! - [`engine`]：音频引擎，管理音频源生命周期
//! - [`capture`]：cpal 麦克风采集源
//! - [`loopback`]：WASAPI 系统音频环回采集源（本机播放器声音）
//! - [`resample`]：声道混缩与线性重采样工具（文件源 / 采集源共用）
//!
//! 本模块禁止直接依赖 Slint；cpal / WASAPI 类型不对外泄漏。

pub mod buffer;
pub mod capture;
pub mod engine;
pub mod loopback;
pub mod resample;
pub mod source;

pub use buffer::PcmRingBuffer;
pub use capture::MicrophoneCapture;
pub use engine::AudioEngine;
pub use loopback::SystemAudioSource;
pub use source::{AudioSource, AudioSourceKind, TestSignalSource};
