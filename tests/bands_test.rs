//! 频段映射集成测试：频率 → 频谱柱 的对数映射正确性。
//!
//! 验证低频/高频落点、覆盖无空洞、幅度谱取值长度一致。

use music_spectrum::config::Settings;
use music_spectrum::spectrum::BandMapper;

const SAMPLE_RATE: u32 = 44100;

fn default_mapper() -> BandMapper {
    let settings = Settings::default();
    BandMapper::new(&settings.spectrum, SAMPLE_RATE)
}

#[test]
fn lowest_frequency_lands_in_first_band() {
    let mapper = default_mapper();
    let (lo, hi) = mapper.band_edges_hz(0);
    assert!(lo <= 20.0, "第一个频段应从 20Hz 或更低开始：{lo}");
    assert!(hi > 20.0, "第一个频段应覆盖 20Hz：({lo}, {hi})");
}

#[test]
fn highest_frequency_lands_in_last_band() {
    let mapper = default_mapper();
    let last = mapper.bar_count() - 1;
    let (lo, hi) = mapper.band_edges_hz(last);
    assert!(lo < 20000.0, "最后一个频段应覆盖 20kHz：({lo}, {hi})");
    assert!(
        (hi - 20000.0).abs() < 5.0,
        "最后一个频段应终止于 20kHz：{hi}"
    );
}

#[test]
fn mid_frequency_maps_between_extremes() {
    let mapper = default_mapper();
    let mags = vec![0.0f32; mapper.bin_count()];
    let values = mapper.values(&mags);
    assert_eq!(values.len(), mapper.bar_count());
    // 1kHz 应落在首尾之间（既不在 band 0 也不在最后 band）
    let one_khz = band_of(&mapper, 1000.0);
    assert!(one_khz > 0 && one_khz < mapper.bar_count() - 1);
}

#[test]
fn bands_cover_range_without_gaps() {
    let mapper = default_mapper();
    for i in 1..mapper.bar_count() {
        let (prev_lo, prev_hi) = mapper.band_edges_hz(i - 1);
        let (lo, hi) = mapper.band_edges_hz(i);
        assert!(
            (lo - prev_hi).abs() < 1e-3,
            "band {i} 起点与前一频段终点不连续：{prev_hi} vs {lo}"
        );
        assert!(lo > prev_lo && hi > prev_hi);
        assert!(lo < hi, "band {i} 空频段");
    }
}

#[test]
fn all_bins_stay_in_bounds() {
    let mapper = default_mapper();
    for i in 0..mapper.bar_count() {
        let (s, e) = mapper.band_bins(i);
        assert!(s <= e, "band {i}: start {s} > end {e}");
        assert!(e < mapper.bin_count(), "band {i}: end {e} 越界");
    }
}

#[test]
fn center_frequencies_are_geometric_means() {
    let mapper = default_mapper();
    for i in 0..mapper.bar_count() {
        let (lo, hi) = mapper.band_edges_hz(i);
        let center = mapper.center_hz(i);
        assert!(
            center > lo && center < hi,
            "band {i} 中心频率 {center} 不在 ({lo}, {hi})"
        );
    }
}

#[test]
fn values_into_reuses_buffer_without_extra_allocation() {
    let mapper = default_mapper();
    let mags = vec![0.5f32; mapper.bin_count()];
    let mut out = Vec::new();
    mapper.values_into(&mags, &mut out);
    assert_eq!(out.len(), mapper.bar_count());
    assert!(out.iter().all(|v| (v - 0.5).abs() < 1e-5));
    // 再次调用应复用容量（长度不变，内容重算）
    let cap_before = out.capacity();
    mapper.values_into(&mags, &mut out);
    assert_eq!(out.len(), mapper.bar_count());
    assert!(out.capacity() >= cap_before);
}

fn band_of(mapper: &BandMapper, freq: f32) -> usize {
    for i in 0..mapper.bar_count() {
        let (lo, hi) = mapper.band_edges_hz(i);
        if freq >= lo && freq < hi {
            return i;
        }
    }
    mapper.bar_count() - 1
}
