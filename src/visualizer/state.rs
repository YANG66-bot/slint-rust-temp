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

/// 低频能量 EMA 时间常数（秒）：比柱高插值更慢，
/// 让反应式基线的伸缩平滑庄重，不随单帧噪声闪烁。
const BASS_EMA_TAU: f32 = 0.12;

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
    /// 低频能量目标（最新分析帧）。
    target_bass: f32,
    /// 低频能量展示值（EMA 平滑后，供反应式基线使用）。
    smoothed_bass: f32,
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
            target_bass: 0.0,
            smoothed_bass: 0.0,
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
    /// 分析器固定分辨率与展示柱数可能不一致（柱数滑条实时改变时），
    /// 此处按线性插值把帧重采样到展示长度，实现柱数实时可调而无需
    /// 重启分析线程。长度相同时为恒等拷贝（零开销）。
    pub fn push_frame(&mut self, frame: &SpectrumFrame) {
        let n = self.target_heights.len();
        self.target_heights
            .copy_from_slice(&resample_to(&frame.heights, n));
        self.target_peaks
            .copy_from_slice(&resample_to(&frame.peaks, n));
        self.target_bass = frame.bass.clamp(0.0, 1.0);
        self.since_last_frame = 0.0;
        self.idle_clock = 0.0;
    }

    /// 改变展示柱数 `n`：按线性插值重采样当前展示值与目标值，
    /// 避免柱数变化瞬间的高度跳变。
    pub fn resize(&mut self, n: usize) {
        if n == self.heights.len() {
            return;
        }
        self.heights = resample_to(&self.heights, n);
        self.peaks = resample_to(&self.peaks, n);
        self.target_heights = resample_to(&self.target_heights, n);
        self.target_peaks = resample_to(&self.target_peaks, n);
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

        // 低频能量单独走一条更慢的 EMA（帧率无关）：
        // smooth = smooth·f + current·(1-f)，f = exp(-dt/tau)，
        // 防止基线长度随单帧抖动产生“频闪”式伸缩
        let bk = 1.0 - (-dt / BASS_EMA_TAU).exp();
        self.smoothed_bass += (self.target_bass - self.smoothed_bass) * bk;
    }

    /// 平滑后的低频能量（0~1，只读）。
    pub fn bass_level(&self) -> f32 {
        self.smoothed_bass
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
        self.target_bass = 0.0;
        self.smoothed_bass = 0.0;
        self.since_last_frame = f32::INFINITY;
        self.idle_clock = 0.0;
    }
}

/// 把 `src` 线性重采样到长度 `n`。长度相同直接返回副本；
/// 空输入或 `n == 0` 返回 `n` 个 0；单点输入返回 `n` 个相同值。
fn resample_to(src: &[f32], n: usize) -> Vec<f32> {
    if src.len() == n {
        return src.to_vec();
    }
    if n == 0 {
        return Vec::new();
    }
    if src.is_empty() {
        return vec![0.0; n];
    }
    if src.len() == 1 {
        return vec![src[0]; n];
    }
    let last = src.len() - 1;
    (0..n)
        .map(|i| {
            let t = if n == 1 {
                0.0
            } else {
                i as f32 / (n - 1) as f32
            } * last as f32;
            let i0 = t.floor() as usize;
            let i1 = (i0 + 1).min(last);
            let f = t - i0 as f32;
            src[i0] + (src[i1] - src[i0]) * f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(heights: &[f32], peaks: &[f32]) -> SpectrumFrame {
        SpectrumFrame {
            heights: heights.to_vec(),
            peaks: peaks.to_vec(),
            bass: 0.0,
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
    fn shorter_frame_is_resampled_to_display_length() {
        let mut state = VisualizerState::new(8);
        state.push_frame(&frame(&[0.5], &[0.5])); // 帧只有 1 根柱 → 重采样为常量
        assert_eq!(state.bar_count(), 8);
        for _ in 0..40 {
            state.tick(0.016);
        }
        // 单点帧均匀重采样：所有柱应收敛到 0.5
        assert!(
            state.heights().iter().all(|h| (h - 0.5).abs() < 0.02),
            "单点帧应重采样为常量，实际 {:?}",
            state.heights()
        );
    }

    #[test]
    fn resize_preserves_shape_by_resampling() {
        let mut state = VisualizerState::new(4);
        state.push_frame(&frame(&[0.0, 0.33, 0.66, 1.0], &[0.0, 0.33, 0.66, 1.0]));
        for _ in 0..40 {
            state.tick(0.016);
        }
        state.resize(8);
        assert_eq!(state.bar_count(), 8);
        // 重采样后单调不减，首尾仍接近 0 / 1
        let h = state.heights();
        assert!(h.windows(2).all(|w| w[1] >= w[0] - 1e-3), "应保序：{:?}", h);
        assert!(h[0] < 0.1 && h[7] > 0.9, "端点应保留，实际 {:?}", h);
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

    #[test]
    fn bass_ema_converges_and_decays_smoothly() {
        let mut state = VisualizerState::new(2);
        let mut f = frame(&[0.5, 0.5], &[0.5, 0.5]);
        f.bass = 0.8;
        state.push_frame(&f);
        // 单步：向目标靠近但远未到达（EMA 防抖）
        state.tick(0.016);
        let after_one = state.bass_level();
        assert!(after_one > 0.0 && after_one < 0.4, "单步不应过冲: {after_one}");
        // 多步收敛到目标
        for _ in 0..60 {
            state.tick(0.016);
        }
        assert!(
            (state.bass_level() - 0.8).abs() < 0.02,
            "~1s 后应收敛到 0.8，实际 {}",
            state.bass_level()
        );
        // 目标归零：衰减同样平滑（约 2 倍 tau 后仍有余值）
        f.bass = 0.0;
        state.push_frame(&f);
        for _ in 0..8 {
            state.tick(0.016);
        }
        let decaying = state.bass_level();
        assert!(
            decaying > 0.2 && decaying < 0.8,
            "衰减应渐进而非跳零，实际 {decaying}"
        );
    }
}
