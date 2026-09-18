//! Peak Hold：频谱峰顶指示点的保持与缓慢衰减。
//!
//! 视觉要求（参考稿）：每根柱顶上方悬浮一个更亮的小点/短线，
//! - 柱高刷新峰值时立即上跳
//! - 之后按每帧乘法系数缓慢下落（0.985）
//! - 永不低于当前柱高（峰点始终悬浮于柱顶之上或重合）

/// 逐柱峰值保持器。
pub struct PeakHold {
    decay: f32,
    peaks: Vec<f32>,
}

impl PeakHold {
    /// 创建 `bar_count` 柱的峰值保持器，初始峰值 0。
    ///
    /// `decay` 为每帧乘法衰减系数（如 0.985），越接近 1 峰点悬停越久。
    pub fn new(bar_count: usize, decay: f32) -> Self {
        Self {
            decay: decay.clamp(0.0, 1.0),
            peaks: vec![0.0; bar_count],
        }
    }

    /// 更新衰减系数（配置热更新时使用）。
    pub fn update_decay(&mut self, decay: f32) {
        self.decay = decay.clamp(0.0, 1.0);
    }

    /// 输入当前柱高，返回峰顶位置（长度取双方较小值）。
    pub fn process(&mut self, heights: &[f32]) -> &[f32] {
        for (peak, &h) in self.peaks.iter_mut().zip(heights) {
            if !h.is_finite() {
                continue; // 防止 NaN 污染峰值状态
            }
            if h >= *peak {
                // 新峰值立即刷新
                *peak = h;
            } else {
                // 乘法衰减，但不低于当前柱高
                *peak = (*peak * self.decay).max(h);
            }
        }
        &self.peaks
    }

    /// 当前峰值（只读）。
    pub fn values(&self) -> &[f32] {
        &self.peaks
    }

    /// 重置为全 0。
    pub fn reset(&mut self) {
        self.peaks.iter_mut().for_each(|v| *v = 0.0);
    }

    /// 柱数。
    pub fn bar_count(&self) -> usize {
        self.peaks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_updates_immediately_on_new_high() {
        let mut peaks = PeakHold::new(1, 0.985);
        let out = peaks.process(&[0.7]);
        assert_eq!(out[0], 0.7);
        let out = peaks.process(&[0.9]);
        assert_eq!(out[0], 0.9);
    }

    #[test]
    fn peak_decays_gradually_not_instantly() {
        let mut peaks = PeakHold::new(1, 0.985);
        peaks.process(&[1.0]);
        // 柱高骤降为 0：峰值单帧只衰减 1.5%
        let out = peaks.process(&[0.0]);
        assert!((out[0] - 0.985).abs() < 1e-6, "first decay: {}", out[0]);
        let out = peaks.process(&[0.0]);
        assert!((out[0] - 0.985 * 0.985).abs() < 1e-6);
    }

    #[test]
    fn peak_never_falls_below_current_height() {
        let mut peaks = PeakHold::new(1, 0.5); // 极快的衰减系数
        peaks.process(&[1.0]);
        // 柱高 0.8：即使衰减一半（0.5）也不会低于 0.8
        let out = peaks.process(&[0.8]);
        assert_eq!(out[0], 0.8);
        // 柱高 0.6：衰减后 0.8*0.5=0.4，被夹回 0.6
        let out = peaks.process(&[0.6]);
        assert_eq!(out[0], 0.6);
    }

    #[test]
    fn non_finite_heights_keep_previous_peak() {
        let mut peaks = PeakHold::new(2, 0.985);
        peaks.process(&[0.5, 0.5]);
        let out = peaks.process(&[f32::NAN, 0.4]);
        // NaN 柱保持原峰值不变
        assert!((out[0] - 0.5).abs() < 1e-6);
        // 柱 1 正常衰减：峰值仍有限且在 [柱高, 原峰值] 之间
        assert!(out[1].is_finite());
        assert!(out[1] >= 0.4 && out[1] <= 0.5);
    }

    #[test]
    fn reset_clears_peaks() {
        let mut peaks = PeakHold::new(2, 0.985);
        peaks.process(&[1.0, 1.0]);
        peaks.reset();
        assert!(peaks.values().iter().all(|v| *v == 0.0));
    }
}
