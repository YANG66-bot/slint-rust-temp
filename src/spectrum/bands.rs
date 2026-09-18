//! 对数频率 → 频谱柱 的频段映射。
//!
//! 直接把 FFT bin 映射到柱子会让低频占据过多柱子、高频挤成一格；
//! 这里使用对数间隔的频段（例如 20Hz~20kHz），并对相邻 bin 做
//! 线性插值采样，消除低频段的"阶梯感"。

use crate::config::SpectrumSettings;

/// 频段映射器：把 FFT bin 聚合为对数间隔的视觉频段。
pub struct BandMapper {
    bar_count: usize,
    bin_count: usize,
    bin_hz: f32,
    edges_hz: Vec<(f32, f32)>,
    start_bin: Vec<f32>,
    end_bin: Vec<f32>,
    center_hz: Vec<f32>,
}

impl BandMapper {
    /// 依据频谱设置与采样率构建映射。
    pub fn new(settings: &SpectrumSettings, sample_rate: u32) -> Self {
        let n = settings.bar_count.max(1);
        let lo = settings.min_frequency.max(1.0);
        let hi = settings.max_frequency.max(lo * 2.0);
        let bin_count = settings.fft_size / 2;
        let bin_hz = sample_rate as f32 / settings.fft_size as f32;

        let ratio = (hi / lo).powf(1.0 / n as f32);
        let mut edges_hz = Vec::with_capacity(n);
        let mut start_bin = Vec::with_capacity(n);
        let mut end_bin = Vec::with_capacity(n);
        let mut center_hz = Vec::with_capacity(n);

        let mut f = lo;
        for _ in 0..n {
            let next = f * ratio;
            edges_hz.push((f, next));
            // 夹到 [0, bin_count-1]，低频频段可能小于一个 bin（亚 bin 插值）
            start_bin.push((f / bin_hz).clamp(0.0, (bin_count - 1) as f32));
            end_bin.push((next / bin_hz).clamp(0.0, (bin_count - 1) as f32));
            center_hz.push((f * next).sqrt());
            f = next;
        }

        Self {
            bar_count: n,
            bin_count,
            bin_hz,
            edges_hz,
            start_bin,
            end_bin,
            center_hz,
        }
    }

    /// 频谱柱数量。
    pub fn bar_count(&self) -> usize {
        self.bar_count
    }

    /// FFT bin 数量（`fft_size/2`）。
    pub fn bin_count(&self) -> usize {
        self.bin_count
    }

    /// 单个 bin 的频率宽度（Hz）。
    pub fn bin_width_hz(&self) -> f32 {
        self.bin_hz
    }

    /// 第 `i` 个频段的（起始频率, 结束频率），单位 Hz。
    pub fn band_edges_hz(&self, i: usize) -> (f32, f32) {
        self.edges_hz[i]
    }

    /// 第 `i` 个频段的中心频率（几何平均，Hz）。
    pub fn center_hz(&self, i: usize) -> f32 {
        self.center_hz[i]
    }

    /// 第 `i` 个频段覆盖的整数 bin 范围（含头含尾），保证不越界。
    pub fn band_bins(&self, i: usize) -> (usize, usize) {
        let start = self.start_bin[i].floor() as usize;
        let end = ((self.end_bin[i].ceil() as usize).saturating_sub(1)).max(start);
        (start.min(self.bin_count - 1), end.min(self.bin_count - 1))
    }

    /// 从幅度谱计算每个频段的取值。
    ///
    /// 每个频段在其 bin 区间内等距采样若干点（点间线性插值），
    /// 取最大值，保证低/中/高频都有明显变化且无阶梯感。
    pub fn values(&self, magnitudes: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.bar_count);
        self.values_into(magnitudes, &mut out);
        out
    }

    /// 与 [`BandMapper::values`] 相同，但写入复用的 `out`（零每帧分配）。
    ///
    /// `out` 会被清空后重新填充，长度恰为频谱柱数。
    pub fn values_into(&self, magnitudes: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.extend((0..self.bar_count).map(|i| self.band_value(magnitudes, i)));
    }

    fn band_value(&self, mags: &[f32], i: usize) -> f32 {
        let s = self.start_bin[i];
        let e = self.end_bin[i];
        if e - s < 1e-3 {
            return interpolated(mags, s);
        }
        // 至少 4 个采样点，宽频段约每 bin 一个点
        let steps = ((e - s).ceil() as usize).max(4);
        let mut best = 0.0f32;
        for k in 0..steps {
            let pos = s + (e - s) * k as f32 / (steps - 1) as f32;
            best = best.max(interpolated(mags, pos));
        }
        best
    }
}

/// 在幅度谱上按浮点位置线性插值取样。
fn interpolated(mags: &[f32], pos: f32) -> f32 {
    if mags.is_empty() {
        return 0.0;
    }
    let last = mags.len() - 1;
    let pos = pos.clamp(0.0, last as f32);
    let i0 = pos.floor() as usize;
    let i1 = (i0 + 1).min(last);
    let frac = pos - i0 as f32;
    mags[i0] * (1.0 - frac) + mags[i1] * frac
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SpectrumSettings;

    fn mapper() -> BandMapper {
        let settings = SpectrumSettings {
            bar_count: 64,
            fft_size: 2048,
            min_frequency: 20.0,
            max_frequency: 20000.0,
            ..SpectrumSettings::default()
        };
        BandMapper::new(&settings, 44100)
    }

    #[test]
    fn low_frequency_lands_in_first_band() {
        let m = mapper();
        // 20~25Hz 应在 band 0
        assert!(m.band_edges_hz(0).0 <= 20.0 && m.band_edges_hz(0).1 > 20.0);
    }

    #[test]
    fn highest_band_reaches_upper_limit() {
        let m = mapper();
        let (lo, hi) = m.band_edges_hz(m.bar_count() - 1);
        assert!((hi - 20000.0).abs() < 5.0, "last band ends at {hi}");
        assert!(lo < hi);
    }

    #[test]
    fn band_edges_are_monotonically_increasing() {
        let m = mapper();
        for i in 1..m.bar_count() {
            let (prev_lo, prev_hi) = m.band_edges_hz(i - 1);
            let (lo, hi) = m.band_edges_hz(i);
            assert!(lo >= prev_lo && hi > prev_hi, "band {i} not monotonic");
        }
    }

    #[test]
    fn bins_never_out_of_bounds() {
        let m = mapper();
        for i in 0..m.bar_count() {
            let (s, e) = m.band_bins(i);
            assert!(s <= e, "band {i}: start {s} > end {e}");
            assert!(e < m.bin_count(), "band {i}: end {e} >= {}", m.bin_count());
        }
    }

    #[test]
    fn sine_peaks_in_band_containing_its_frequency() {
        let m = mapper();
        let mags = vec![0.0f32; m.bin_count()];
        let target = 1000.0f32;
        // 找到包含 1kHz 的 band，把该 band 覆盖的 bin 置为高值
        let mut band_index = 0;
        for i in 0..m.bar_count() {
            let (lo, hi) = m.band_edges_hz(i);
            if target >= lo && target < hi {
                band_index = i;
                break;
            }
        }
        let mut mags = mags;
        let (s, e) = m.band_bins(band_index);
        for mag in &mut mags[s..=e] {
            *mag = 1.0;
        }
        let values = m.values(&mags);
        let max_i = values
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .expect("non-empty");
        assert_eq!(max_i, band_index);
    }

    #[test]
    fn sub_bin_bands_interpolate_smoothly() {
        // 低频相邻 band 覆盖同一 bin 时，亚 bin 插值应产生不同取值
        let m = mapper();
        let mut mags = vec![0.0f32; m.bin_count()];
        mags[0] = 1.0;
        mags[1] = 0.5;
        let values = m.values(&mags);
        assert_eq!(values.len(), m.bar_count());
        // 前几个 band（20~30Hz 区间）取值应渐变而非全部相同
        let head: Vec<f32> = values.iter().take(4).copied().collect();
        assert!(head.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-4));
    }
}
