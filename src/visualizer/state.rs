//! 可视化状态：分析帧 → 展示值的 60Hz 插值层。
//!
//! 分析线程以 40Hz 产出 [`SpectrumFrame`]，UI 以约 60Hz 刷新；
//! 直接把分析帧贴到屏幕会出现明显阶梯感。本结构保存
//! "目标值"（最新分析帧）与"展示值"（持续向目标指数逼近），
//! 每次 [`VisualizerState::tick`] 用真实流逝时间做帧率无关的插值，
//! 保证任何刷新率下动画速度一致、无锯齿感。

use crate::spectrum::SpectrumFrame;

/// 默认插值时间常数（秒）：约 1~2 个 UI 帧内完成大部分过渡。
///
/// 分析端已有 attack/release 平滑，这里只需轻量插值消除
/// 40Hz→60Hz 的阶梯，取值过大会产生拖沓感。
const DEFAULT_TIME_CONSTANT: f32 = 0.018;

/// 最大步进间隔（秒）：调试器暂停 / 窗口最小化等场景下
/// 防止一次性大跳，保持视觉连续。
const MAX_STEP_SECS: f32 = 0.25;

/// 超过此时长未收到分析帧则进入 idle 呼吸模式（秒）。
const IDLE_THRESHOLD_SECS: f32 = 2.0;

/// idle 呼吸的柱高范围（无信号时轻柔起伏，不喧宾夺主）。
const IDLE_MIN_HEIGHT: f32 = 0.03;
const IDLE_MAX_HEIGHT: f32 = 0.11;

/// 呼吸频率（Hz）与柱间相位跨度（弧度）。
const IDLE_BREATH_HZ: f32 = 0.35;
const IDLE_PHASE_SPAN: f32 = 6.0;

/// 频谱展示状态（仅 UI 线程访问）。
pub struct VisualizerState {
    /// 当前展示高度（插值后，0~1）。
    heights: Vec<f32>,
    /// 当前展示峰值位置（插值后，0~1）。
    peaks: Vec<f32>,
    /// 目标高度（最新分析帧）。
    target_heights: Vec<f32>,
    /// 目标峰值（最新分析帧）。
    target_peaks: Vec<f32>,
    /// 插值时间常数（秒）。
    time_constant: f32,
    /// 是否启用 idle 呼吸动画（无信号时轻柔起伏）。
    idle_enabled: bool,
    /// 距上一帧的累计秒数。
    since_last_frame: f32,
    /// idle 模式的累计时钟（秒）。
    idle_clock: f32,
}

impl VisualizerState {
    /// 创建 `bar_count` 柱的状态（全部归零）。
    pub fn new(bar_count: usize) -> Self {
        Self {
            heights: vec![0.0; bar_count],
            peaks: vec![0.0; bar_count],
            target_heights: vec![0.0; bar_count],
            target_peaks: vec![0.0; bar_count],
            time_constant: DEFAULT_TIME_CONSTANT,
            idle_enabled: true,
            since_last_frame: f32::INFINITY,
            idle_clock: 0.0,
        }
    }

    /// 柱数。
    pub fn bar_count(&self) -> usize {
        self.heights.len()
    }

    /// 设置插值时间常数（秒）。
    pub fn set_time_constant(&mut self, time_constant: f32) {
        self.time_constant = time_constant.clamp(0.001, 1.0);
    }

    /// 是否启用 idle 呼吸动画。
    pub fn set_idle_enabled(&mut self, enabled: bool) {
        self.idle_enabled = enabled;
    }

    /// 接收一帧分析结果作为新的插值目标。
    ///
    /// 帧的柱数与当前状态不一致时以较小者为准（配置热切换
    /// 期间防止越界；柱数变更需由持有方重建状态）。
    pub fn push_frame(&mut self, frame: &SpectrumFrame) {
        let n = frame.heights.len().min(self.target_heights.len());
        for (t, &h) in self.target_heights.iter_mut().zip(&frame.heights).take(n) {
            *t = h;
        }
        for (t, &p) in self.target_peaks.iter_mut().zip(&frame.peaks).take(n) {
            *t = p;
        }
        self.since_last_frame = 0.0;
        self.idle_clock = 0.0;
    }

    /// 按流逝时间 `dt_secs` 向目标插值一步。
    ///
    /// 指数逼近（帧率无关）：`k = 1 - exp(-dt / time_constant)`，
    /// 高度与峰值共用同一时间常数，保持峰点与柱顶的相对关系稳定。
    pub fn tick(&mut self, dt_secs: f32) {
        let dt = dt_secs.clamp(0.0, MAX_STEP_SECS);
        // f32 加法溢出只会变为 INFINITY（安全），此后恒触发 idle
        self.since_last_frame += dt;

        // idle 呼吸：长时间无信号时生成轻柔的柱间相位差正弦目标，
        // 让频谱"活着"但不喧宾夺主（收到新帧后自动退出）
        if self.idle_enabled && self.since_last_frame > IDLE_THRESHOLD_SECS {
            self.idle_clock += dt;
            let t = self.idle_clock;
            let n = self.target_heights.len();
            for (i, th) in self.target_heights.iter_mut().enumerate() {
                let phase = i as f32 / n.max(1) as f32;
                let wave = 0.5
                    * (1.0
                        + (std::f32::consts::TAU * IDLE_BREATH_HZ * t + phase * IDLE_PHASE_SPAN)
                            .sin());
                let h = IDLE_MIN_HEIGHT + (IDLE_MAX_HEIGHT - IDLE_MIN_HEIGHT) * wave;
                *th = h;
                self.target_peaks[i] = h + 0.015;
            }
        }

        let k = 1.0 - (-dt / self.time_constant).exp();
        for (cur, &t) in self.heights.iter_mut().zip(&self.target_heights) {
            *cur += (t - *cur) * k;
        }
        for (cur, &t) in self.peaks.iter_mut().zip(&self.target_peaks) {
            *cur += (t - *cur) * k;
        }
    }

    /// 展示高度（只读）。
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// 展示峰值（只读）。
    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }

    /// 重置为全 0（音源切换时清空旧状态）。
    pub fn reset(&mut self) {
        self.heights.iter_mut().for_each(|v| *v = 0.0);
        self.peaks.iter_mut().for_each(|v| *v = 0.0);
        self.target_heights.iter_mut().for_each(|v| *v = 0.0);
        self.target_peaks.iter_mut().for_each(|v| *v = 0.0);
        self.since_last_frame = f32::INFINITY;
        self.idle_clock = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(heights: &[f32], peaks: &[f32]) -> SpectrumFrame {
        SpectrumFrame {
            heights: heights.to_vec(),
            peaks: peaks.to_vec(),
        }
    }

    #[test]
    fn tick_interpolates_toward_target_without_overshoot() {
        let mut state = VisualizerState::new(4);
        state.push_frame(&frame(&[1.0; 4], &[1.0; 4]));
        // 单步插值：应向 1 靠近但不过冲
        state.tick(0.016);
        assert!(
            state.heights().iter().all(|h| *h > 0.0 && *h < 1.0),
            "单步应在 0~1 之间"
        );
        // 多步收敛
        for _ in 0..40 {
            state.tick(0.016);
        }
        assert!(state.heights().iter().all(|h| *h > 0.99), "0.64s 后应收敛");
    }

    #[test]
    fn interpolation_is_frame_rate_independent() {
        // 4 次 60Hz 步进 ≈ 2 次 30Hz 步进（同样的总时长）
        let mut fine = VisualizerState::new(1);
        fine.push_frame(&frame(&[1.0], &[1.0]));
        for _ in 0..4 {
            fine.tick(1.0 / 60.0);
        }
        let mut coarse = VisualizerState::new(1);
        coarse.push_frame(&frame(&[1.0], &[1.0]));
        for _ in 0..2 {
            coarse.tick(2.0 / 60.0);
        }
        let a = fine.heights()[0];
        let b = coarse.heights()[0];
        assert!((a - b).abs() < 0.01, "60Hz {a} vs 30Hz {b} 应基本一致");
    }

    #[test]
    fn huge_dt_is_clamped_to_avoid_jump() {
        let mut state = VisualizerState::new(1);
        state.push_frame(&frame(&[1.0], &[1.0]));
        // 暂停 10 秒后一步：不应瞬间贴满
        state.tick(10.0);
        assert!(state.heights()[0] < 1.0);
    }

    #[test]
    fn shorter_frame_data_is_handled_safely() {
        let mut state = VisualizerState::new(8);
        state.push_frame(&frame(&[0.5], &[0.5])); // 帧只有 1 根柱
        assert_eq!(state.bar_count(), 8);
        state.tick(0.016);
        assert!((state.heights()[0] - 0.5f32 * 0.58).abs() < 0.1);
        assert_eq!(state.heights()[1], 0.0);
    }

    #[test]
    fn reset_clears_everything() {
        let mut state = VisualizerState::new(2);
        state.push_frame(&frame(&[1.0, 1.0], &[1.0, 1.0]));
        for _ in 0..20 {
            state.tick(0.016);
        }
        state.reset();
        assert!(state.heights().iter().all(|v| *v == 0.0));
        assert!(state.peaks().iter().all(|v| *v == 0.0));
    }

    #[test]
    fn idle_animation_starts_after_long_silence() {
        let mut state = VisualizerState::new(4);
        // 无帧累计 3.2 秒 → 进入 idle 呼吸
        for _ in 0..200 {
            state.tick(0.016);
        }
        assert!(
            state.heights().iter().any(|h| *h > 0.001),
            "idle 模式应产生呼吸高度"
        );
        // 呼吸高度远小于有效信号高度
        assert!(state.heights().iter().all(|h| *h < 0.3));
        // 柱间相位差：不是所有柱同一高度
        let first = state.heights()[0];
        let last = state.heights()[3];
        assert!((first - last).abs() > 1e-4, "idle 波形应有柱间差异");
    }

    #[test]
    fn new_frame_exits_idle_mode() {
        let mut state = VisualizerState::new(2);
        for _ in 0..150 {
            state.tick(0.016);
        }
        state.push_frame(&frame(&[0.9, 0.9], &[0.9, 0.9]));
        for _ in 0..30 {
            state.tick(0.016);
        }
        assert!(
            state.heights()[0] > 0.8,
            "新帧应覆盖 idle 目标，实际 {}",
            state.heights()[0]
        );
    }

    #[test]
    fn idle_disabled_stays_flat() {
        let mut state = VisualizerState::new(2);
        state.set_idle_enabled(false);
        for _ in 0..220 {
            state.tick(0.016);
        }
        assert!(state.heights().iter().all(|h| *h == 0.0));
    }
}
