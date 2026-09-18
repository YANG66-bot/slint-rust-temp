//! FFT 处理器（rustfft 封装）。
//!
//! FftPlanner / 输入缓冲 / scratch 全部复用，避免每帧分配。

use crate::error::{AppError, AppResult};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// 可复用的实数输入 FFT 处理器。
pub struct FftProcessor {
    size: usize,
    fft: Arc<dyn Fft<f32>>,
    buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    magnitudes: Vec<f32>,
}

impl FftProcessor {
    /// 创建 FFT 尺寸为 `size` 的处理器（建议 2 的幂）。
    pub fn new(size: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(size);
        let scratch_len = fft.get_inplace_scratch_len();
        Self {
            size,
            fft,
            buffer: vec![Complex::new(0.0, 0.0); size],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            magnitudes: vec![0.0; size / 2],
        }
    }

    /// FFT 尺寸。
    pub fn size(&self) -> usize {
        self.size
    }

    /// 计算实数输入的幅度谱。
    ///
    /// 返回 `size/2` 个 bin 的幅度（已按 `size/2` 归一化：
    /// 恰好位于 bin 中心的满幅正弦对应幅度 1.0）。
    /// 输入长度必须等于 FFT 尺寸，否则返回 [`AppError::FftError`]。
    pub fn magnitude_spectrum(&mut self, samples: &[f32]) -> AppResult<&[f32]> {
        if samples.len() != self.size {
            return Err(AppError::FftError(format!(
                "FFT 输入长度 {} 与尺寸 {} 不匹配",
                samples.len(),
                self.size
            )));
        }
        for (c, &s) in self.buffer.iter_mut().zip(samples) {
            c.re = s;
            c.im = 0.0;
        }
        self.fft
            .process_with_scratch(&mut self.buffer, &mut self.scratch);
        let norm = self.size as f32 / 2.0;
        for (i, m) in self.magnitudes.iter_mut().enumerate() {
            let c = self.buffer[i];
            *m = (c.re * c.re + c.im * c.im).sqrt() / norm;
        }
        Ok(&self.magnitudes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn zero_input_produces_zero_without_nan() {
        let mut fft = FftProcessor::new(2048);
        let input = [0.0f32; 2048];
        let mags = fft.magnitude_spectrum(&input).expect("fft");
        assert!(mags.iter().all(|m| m.is_finite()));
        assert!(mags.iter().all(|m| *m < 1e-6));
    }

    #[test]
    fn length_mismatch_is_an_error() {
        let mut fft = FftProcessor::new(1024);
        assert!(fft.magnitude_spectrum(&[0.0; 512]).is_err());
        assert!(fft.magnitude_spectrum(&[0.0; 2048]).is_err());
    }

    #[test]
    fn sine_wave_peaks_at_expected_bin() {
        let size = 2048usize;
        let bin = 100usize;
        let rate = 44100.0f32;
        let freq = bin as f32 * rate / size as f32;
        let input: Vec<f32> = (0..size)
            .map(|i| 0.5 * (TAU * freq * i as f32 / rate).sin())
            .collect();
        let mut fft = FftProcessor::new(size);
        let mags = fft.magnitude_spectrum(&input).expect("fft");
        // 峰值应出现在 bin 100 且幅度接近 0.5
        let (mut best_i, mut best_v) = (0usize, 0.0f32);
        for (i, &m) in mags.iter().enumerate() {
            if m > best_v {
                best_v = m;
                best_i = i;
            }
        }
        assert_eq!(best_i, bin);
        assert!((best_v - 0.5).abs() < 0.02, "peak magnitude {best_v}");
    }

    #[test]
    fn magnitude_spectrum_is_reusable() {
        let mut fft = FftProcessor::new(512);
        let silence = [0.0f32; 512];
        let m1 = fft.magnitude_spectrum(&silence).expect("fft");
        assert!(m1.iter().all(|m| *m < 1e-6));
        let sine: Vec<f32> = (0..512)
            .map(|i| (TAU * 10.0 * i as f32 / 512.0).sin())
            .collect();
        let m2 = fft.magnitude_spectrum(&sine).expect("fft");
        assert!(m2[10] > 0.5);
    }
}
