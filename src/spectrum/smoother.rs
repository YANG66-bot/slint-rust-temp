//! 频谱柱高度平滑（attack / release 双时间常数）。
//!
//! 视觉要求"上升快、下降慢、绝不瞬间归零"：
//! - attack：目标高于当前时，单帧向目标靠近的比例（大 = 快速上冲）
//! - release：目标低于当前时，单帧向目标靠近的比例（小 = 缓慢回落）
//!
//! 采用一阶指数逼近（低通），逐柱独立、无柱间串扰；
//! 内部状态缓冲复用，`process` 每帧零分配。

/// 逐柱 attack/release 指数平滑器。
pub struct SpectrumSmoother {
    attack: f32,
    release: f32,
    current: Vec<f32>,
}

impl SpectrumSmoother {
    /// 创建 `bar_count` 柱的平滑器，初始高度为 0。
    pub fn new(bar_count: usize, attack: f32, release: f32) -> Self {
        Self {
            attack: attack.clamp(0.0, 1.0),
            release: release.clamp(0.0, 1.0),
            current: vec![0.0; bar_count],
        }
    }

    /// 更新平滑系数（配置热更新时使用）。
    pub fn update_coeffs(&mut self, attack: f32, release: f32) {
        self.attack = attack.clamp(0.0, 1.0);
        self.release = release.clamp(0.0, 1.0);
    }

    /// 输入新的目标高度，返回平滑后的高度（长度取双方较小值）。
    ///
    /// 上升用 `attack`、下降用 `release`，因此不会瞬间归零，
    /// 也不会对快速瞬态（鼓点）反应迟钝。
    pub fn process(&mut self, target: &[f32]) -> &[f32] {
        for (cur, &t) in self.current.iter_mut().zip(target) {
            if !t.is_finite() {
                // 非有限目标按静音处理，防 NaN 传播
                continue;
            }
            let coeff = if t > *cur { self.attack } else { self.release };
            *cur += (t - *cur) * coeff;
        }
        &self.current
    }

    /// 当前平滑后的高度（只读）。
    pub fn values(&self) -> &[f32] {
        &self.current
    }

    /// 重置为全 0（音源切换时避免旧状态残留）。
    pub fn reset(&mut self) {
        self.current.iter_mut().for_each(|v| *v = 0.0);
    }

    /// 柱数。
    pub fn bar_count(&self) -> usize {
        self.current.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_rises_quickly_toward_target() {
        let mut smoother = SpectrumSmoother::new(1, 0.65, 0.12);
        let out = smoother.process(&[1.0]);
        // 一帧后应达到 attack 比例
        assert!((out[0] - 0.65).abs() < 1e-6);
        let out = smoother.process(&[1.0]);
        // 第二帧继续逼近但不过冲
        assert!(out[0] > 0.65 && out[0] < 1.0);
    }

    #[test]
    fn release_falls_slowly_and_never_drops_to_zero_instantly() {
        let mut smoother = SpectrumSmoother::new(1, 0.65, 0.12);
        // attack 0.65：每帧残留 35%，5 帧后残留 0.35^5 ≈ 0.005
        for _ in 0..5 {
            smoother.process(&[1.0]);
        }
        assert!(smoother.values()[0] > 0.99);
        // 目标骤降为 0：单帧只回落 release 比例（1 - 0.12 ≈ 0.88）
        let out = smoother.process(&[0.0]);
        assert!((out[0] - 0.88).abs() < 0.02, "one frame drop: {}", out[0]);
        assert!(out[0] > 0.0);
    }

    #[test]
    fn falling_is_slower_than_rising() {
        let mut smoother = SpectrumSmoother::new(1, 0.65, 0.12);
        smoother.process(&[1.0]);
        let rise = smoother.values()[0];
        smoother.process(&[0.0]);
        let fall = 1.0 - smoother.values()[0];
        assert!(rise > fall + 0.1, "rise {rise} should exceed fall {fall}");
    }

    #[test]
    fn non_finite_targets_are_treated_as_silence() {
        let mut smoother = SpectrumSmoother::new(3, 0.65, 0.12);
        // 先推进到稳态 0.5
        for _ in 0..8 {
            smoother.process(&[0.5, 0.5, 0.5]);
        }
        let out = smoother.process(&[f32::NAN, f32::INFINITY, 0.5]);
        // NaN/Inf 不传播，保持原值
        assert!(out.iter().all(|v| v.is_finite()));
        assert!((out[0] - 0.5).abs() < 1e-3);
        assert!((out[1] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn reset_clears_state() {
        let mut smoother = SpectrumSmoother::new(2, 0.65, 0.12);
        smoother.process(&[1.0, 1.0]);
        smoother.process(&[1.0, 1.0]);
        smoother.reset();
        assert!(smoother.values().iter().all(|v| *v == 0.0));
        let out = smoother.process(&[1.0, 1.0]);
        assert!((out[0] - 0.65).abs() < 1e-6);
    }

    #[test]
    fn shorter_targets_are_processed_partially() {
        let mut smoother = SpectrumSmoother::new(4, 0.65, 0.12);
        let out = smoother.process(&[1.0]);
        assert_eq!(out.len(), 4);
        assert!((out[0] - 0.65).abs() < 1e-6);
        assert_eq!(out[1], 0.0);
    }
}
