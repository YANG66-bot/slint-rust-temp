//! dB 归一化与高频倾斜补偿。
//!
//! 不要直接 `height = magnitude`：先转 dB（`20*log10`），
//! 再线性映射到 `[min_db, max_db]` → `[0, 1]`。
//! 幅度为 0 / 非有限值时安全地映射为 0，不产生 NaN / Infinity。

/// 把幅度转换为 dB 后归一化到 `[0, 1]`。
pub fn normalize_db(magnitude: f32, min_db: f32, max_db: f32) -> f32 {
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * magnitude.log10();
    ((db - min_db) / (max_db - min_db)).clamp(0.0, 1.0)
}

/// 高频倾斜增益（dB）。
///
/// 真实音乐的频谱能量大致随频率衰减（粉噪声约 -3dB/倍频程），
/// 不补偿的话高频柱子几乎不动。以 100Hz 为基准，向上每倍频程
/// 增加 `tilt_db_per_octave` dB（向下不衰减），上限 30dB。
pub fn tilt_gain_db(center_hz: f32, tilt_db_per_octave: f32) -> f32 {
    if center_hz <= 100.0 || tilt_db_per_octave <= 0.0 {
        return 0.0;
    }
    let octaves = (center_hz / 100.0).log2();
    (octaves * tilt_db_per_octave).min(30.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_negative_magnitudes_map_to_zero_safely() {
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(normalize_db(bad, -80.0, 0.0), 0.0);
        }
    }

    #[test]
    fn full_scale_maps_to_one() {
        assert!((normalize_db(1.0, -80.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn midpoint_maps_to_half() {
        // -40dB 是 [-80, 0] 的中点
        let mag = 10f32.powf(-40.0 / 20.0);
        assert!((normalize_db(mag, -80.0, 0.0) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn very_quiet_signal_is_clamped() {
        let mag = 10f32.powf(-120.0 / 20.0); // -120dB，低于下限
        assert_eq!(normalize_db(mag, -80.0, 0.0), 0.0);
    }

    #[test]
    fn tilt_is_zero_below_reference() {
        assert_eq!(tilt_gain_db(20.0, 3.0), 0.0);
        assert_eq!(tilt_gain_db(100.0, 3.0), 0.0);
    }

    #[test]
    fn tilt_grows_with_frequency() {
        let low = tilt_gain_db(200.0, 3.0);
        let high = tilt_gain_db(20000.0, 3.0);
        assert!(low > 0.0 && high > low);
        assert!(high <= 30.0 + 1e-6);
    }
}
