//! 调色板：把配置中的五色渐变（粉→浅粉→天蓝→浅紫→青）
//! 映射为逐柱颜色。
//!
//! 颜色插值在 sRGB 空间按柱的水平位置 `t = i / (n-1)` 线性采样；
//! 端点颜色超出条带时被夹紧（首尾柱恰好取端点色）。
//! 配置解析失败时回退到内置默认调色板，保证渲染永不中断。

use crate::config::{DEFAULT_COLORS, parse_hex_color};

/// 频谱柱渐变调色板。
pub struct Palette {
    /// 渐变停靠点（RGB，0~1 浮点）。
    stops: Vec<[f32; 3]>,
}

impl Palette {
    /// 从 `#RRGGBB` 列表构建调色板；不足两个有效色时回退默认。
    pub fn from_hex(colors: &[String]) -> Self {
        let stops: Vec<[f32; 3]> = colors
            .iter()
            .filter_map(|c| parse_hex_color(c))
            .map(|[r, g, b]| [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
            .collect();
        if stops.len() >= 2 {
            Self { stops }
        } else {
            let fallback: Vec<[f32; 3]> = DEFAULT_COLORS
                .iter()
                .filter_map(|c| parse_hex_color(c))
                .map(|[r, g, b]| [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
                .collect();
            Self { stops: fallback }
        }
    }

    /// 停靠点数量。
    pub fn stop_count(&self) -> usize {
        self.stops.len()
    }

    /// 在渐变带位置 `t ∈ [0, 1]` 取色（线性插值，sRGB 空间）。
    pub fn color_at(&self, t: f32) -> [f32; 3] {
        let last = self.stops.len() - 1;
        let t = t.clamp(0.0, 1.0);
        let pos = t * last as f32;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(last);
        let frac = pos - i0 as f32;
        let a = self.stops[i0];
        let b = self.stops[i1];
        [
            a[0] + (b[0] - a[0]) * frac,
            a[1] + (b[1] - a[1]) * frac,
            a[2] + (b[2] - a[2]) * frac,
        ]
    }

    /// 生成 `bar_count` 根柱的逐柱颜色（首尾柱恰为端点色）。
    pub fn bar_colors(&self, bar_count: usize) -> Vec<[f32; 3]> {
        let n = bar_count.max(1);
        (0..n)
            .map(|i| {
                let t = if n == 1 {
                    0.0
                } else {
                    i as f32 / (n - 1) as f32
                };
                self.color_at(t)
            })
            .collect()
    }
    /// 生成 `bar_count` 根柱的峰点颜色（柱色向白色亮化后的更亮色）。
    pub fn peak_colors(&self, bar_count: usize, lighten_amount: f32) -> Vec<[f32; 3]> {
        self.bar_colors(bar_count)
            .into_iter()
            .map(|rgb| lighten(rgb, lighten_amount))
            .collect()
    }
}

/// 把 RGB 向白色亮化：`t = 0` 原色，`t = 1` 纯白。
pub fn lighten(rgb: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        rgb[0] + (1.0 - rgb[0]) * t,
        rgb[1] + (1.0 - rgb[1]) * t,
        rgb[2] + (1.0 - rgb[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_palette() -> Palette {
        Palette::from_hex(
            &DEFAULT_COLORS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn endpoints_match_palette_ends() {
        let p = default_palette();
        assert_eq!(p.stop_count(), 5);
        let first = p.color_at(0.0);
        let last = p.color_at(1.0);
        // 首端 = 粉 #F8AFDB，尾端 = 青 #00FFFF
        assert!((first[0] - 0xF8 as f32 / 255.0).abs() < 1e-4);
        assert!((first[2] - 0xDB as f32 / 255.0).abs() < 1e-4);
        assert!((last[1] - 1.0).abs() < 1e-4, "青色绿色分量应为 1.0");
        assert!((last[2] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn midpoint_blends_between_stops() {
        let p = default_palette();
        let mid = p.color_at(0.5);
        // 中点 = 第 3 个停靠点 #87CEEB（恰好落在停靠点上）
        assert!((mid[0] - 0x87 as f32 / 255.0).abs() < 1e-4);
        assert!((mid[1] - 0xCE as f32 / 255.0).abs() < 1e-4);
        assert!((mid[2] - 0xEB as f32 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn out_of_range_t_is_clamped() {
        let p = default_palette();
        let lo = p.color_at(-1.0);
        let hi = p.color_at(2.0);
        assert!((lo[0] - p.color_at(0.0)[0]).abs() < 1e-6);
        assert!((hi[0] - p.color_at(1.0)[0]).abs() < 1e-6);
    }

    #[test]
    fn bar_colors_cover_all_bars() {
        let p = default_palette();
        let colors = p.bar_colors(88);
        assert_eq!(colors.len(), 88);
        // 首尾柱取端点色，中间柱渐进变化
        assert_eq!(colors[0], p.color_at(0.0));
        assert_eq!(colors[87], p.color_at(1.0));
        assert_ne!(colors[0][0], colors[87][0], "首尾颜色应不同");
    }

    #[test]
    fn invalid_config_falls_back_to_defaults() {
        let p = Palette::from_hex(&["bad".to_string(), "#12".to_string()]);
        assert_eq!(p.stop_count(), 5, "无效配置应回退默认调色板");
    }

    #[test]
    fn lighten_blends_toward_white() {
        let original = [0.2, 0.4, 0.6];
        assert_eq!(lighten(original, 0.0), original);
        let white = lighten(original, 1.0);
        assert!((white[0] - 1.0).abs() < 1e-6);
        let half = lighten(original, 0.5);
        assert!((half[0] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn peak_colors_are_lighter_than_bar_colors() {
        let p = default_palette();
        let bars = p.bar_colors(10);
        let peaks = p.peak_colors(10, 0.35);
        for (b, pk) in bars.iter().zip(&peaks) {
            assert!(pk[0] >= b[0] && pk[1] >= b[1] && pk[2] >= b[2]);
        }
    }
}
