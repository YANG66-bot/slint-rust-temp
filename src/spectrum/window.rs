//! 窗函数（Hann / Hamming / Blackman）。

use crate::config::WindowKind;

/// 预计算系数表的窗函数。
///
/// 通过 [`WindowFunction::new`] 创建后可反复调用
/// [`WindowFunction::apply`]，避免每帧重新计算系数。
pub struct WindowFunction {
    table: Vec<f32>,
}

impl WindowFunction {
    /// 按 `kind` 创建长度为 `size` 的窗函数。
    pub fn new(kind: WindowKind, size: usize) -> Self {
        let table: Vec<f32> = (0..size)
            .map(|i| match kind {
                WindowKind::Hann => hann(i, size),
                WindowKind::Hamming => hamming(i, size),
                WindowKind::Blackman => blackman(i, size),
            })
            .collect();
        Self { table }
    }

    /// 就地加窗（`samples.len()` 应等于窗长，超出部分忽略）。
    pub fn apply(&self, samples: &mut [f32]) {
        for (s, w) in samples.iter_mut().zip(&self.table) {
            *s *= w;
        }
    }

    /// 窗长。
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// 窗是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

fn hann(i: usize, n: usize) -> f32 {
    if n <= 1 {
        return 1.0;
    }
    0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos()
}

fn hamming(i: usize, n: usize) -> f32 {
    if n <= 1 {
        return 1.0;
    }
    0.54 - 0.46 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos()
}

fn blackman(i: usize, n: usize) -> f32 {
    if n <= 1 {
        return 1.0;
    }
    let x = std::f32::consts::TAU * i as f32 / (n - 1) as f32;
    0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_endpoints_and_midpoint() {
        let w = WindowFunction::new(WindowKind::Hann, 1024);
        let mut samples = [1.0f32; 1024];
        w.apply(&mut samples);
        assert!(samples[0].abs() < 1e-6);
        assert!(samples[1023].abs() < 1e-6);
        assert!((samples[512] - 1.0).abs() < 1e-3);
        assert!(samples.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn all_windows_bounded_and_correct_length() {
        for kind in [WindowKind::Hann, WindowKind::Hamming, WindowKind::Blackman] {
            let w = WindowFunction::new(kind, 512);
            assert_eq!(w.len(), 512);
            assert!(!w.is_empty());
            let mut samples = [1.0f32; 512];
            w.apply(&mut samples);
            assert!(samples.iter().all(|v| (-1e-6..=1.0 + 1e-6).contains(v)));
        }
    }

    #[test]
    fn apply_is_in_place_multiplication() {
        let w = WindowFunction::new(WindowKind::Hann, 8);
        let mut samples = [2.0f32; 8];
        w.apply(&mut samples);
        assert_eq!(samples.len(), 8);
        assert!(samples[0].abs() < 1e-6);
        assert!(samples[4] > 0.9);
    }
}
