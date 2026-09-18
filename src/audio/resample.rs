//! 块级声道混缩与线性重采样工具。
//!
//! 供文件播放源与麦克风采集源共用。可视化用途的精度要求较低：
//! 块边界处不跨块插值，单样本级的不连续对频谱分析完全不可感知。

/// 交错多声道 → 单声道（等权平均）。
pub fn mixdown(interleaved: &[f32], channels: u16, out: &mut Vec<f32>) {
    out.clear();
    let ch = usize::from(channels.max(1));
    if interleaved.is_empty() {
        return;
    }
    if ch == 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let mut acc = 0.0f32;
    for (i, &s) in interleaved.iter().enumerate() {
        acc += s;
        if (i + 1) % ch == 0 {
            out.push(acc / ch as f32);
            acc = 0.0;
        }
    }
}

/// 线性插值重采样。
///
/// `step` 为源/目标采样率之比（>1 降采样，<1 升采样）。
/// `cursor` 为浮点游标（每次调用后归一到下一块起点）。
pub fn resample(mono: &[f32], step: f64, cursor: &mut f64, out: &mut Vec<f32>) {
    out.clear();
    if mono.is_empty() {
        return;
    }
    if (step - 1.0).abs() < 1e-9 {
        // 采样率一致：直通
        out.extend_from_slice(mono);
        *cursor = 0.0;
        return;
    }
    let len = mono.len();
    while *cursor < len as f64 - 1.0 {
        let i0 = *cursor as usize;
        let frac = (*cursor - i0 as f64) as f32;
        let s0 = mono[i0];
        let s1 = mono[i0 + 1];
        out.push(s0 + (s1 - s0) * frac);
        *cursor += step;
    }
    // 游标回退到剩余不足一整步的位置
    *cursor -= (len as f64 - 1.0).floor();
    if *cursor < 0.0 {
        *cursor = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixdown_stereo_averages_channels() {
        let interleaved = [0.8, 0.2, 0.6, 0.4, -1.0, 1.0];
        let mut mono = Vec::new();
        mixdown(&interleaved, 2, &mut mono);
        assert_eq!(mono, vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn mixdown_mono_passthrough() {
        let interleaved = [0.1, 0.2, 0.3];
        let mut mono = Vec::new();
        mixdown(&interleaved, 1, &mut mono);
        assert_eq!(mono, interleaved);
    }

    #[test]
    fn resample_identity_passthrough() {
        let mono = vec![0.1, 0.2, 0.3];
        let mut cursor = 0.0;
        let mut out = Vec::new();
        resample(&mono, 1.0, &mut cursor, &mut out);
        assert_eq!(out, mono);
        assert_eq!(cursor, 0.0);
    }

    #[test]
    fn resample_downsample_produces_expected_count() {
        // 48000 → 44100：step ≈ 1.0884
        let mono: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut cursor = 0.0;
        let mut out = Vec::new();
        resample(&mono, 48000.0 / 44100.0, &mut cursor, &mut out);
        // 4800 样本 @48k ≈ 0.1 秒 → 4410 输出样本左右
        assert!(
            (4300..=4420).contains(&out.len()),
            "输出样本数 {}",
            out.len()
        );
        // 值域仍在 [-1, 1]
        assert!(out.iter().all(|v| v.abs() <= 1.0));
    }

    #[test]
    fn resample_empty_input_is_noop() {
        let mut cursor = 0.0;
        let mut out = Vec::new();
        resample(&[], 1.5, &mut cursor, &mut out);
        assert!(out.is_empty());
    }
}
