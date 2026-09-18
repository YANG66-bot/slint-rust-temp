//! 频谱分析模块。
//!
//! 完整处理流程：PCM → 窗函数 → FFT → Magnitude → 对数频段 → dB →
//! 归一化 → 平滑 → Peak Hold → [`analyzer::SpectrumFrame`]。
//! 本模块不得依赖 Slint。

pub mod analyzer;
pub mod bands;
pub mod fft;
pub mod normalize;
pub mod peak;
pub mod smoother;
pub mod window;

pub use analyzer::{AnalysisPipeline, AnalyzerHandle, SpectrumFrame};
pub use bands::BandMapper;
pub use fft::FftProcessor;
pub use peak::PeakHold;
pub use smoother::SpectrumSmoother;
pub use window::WindowFunction;
