// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 NewfolderGames

//! Multi-stop gradients, quantization modes and built-in gradient presets.
//!
//! Colour interpolation runs in premultiplied sRGB when stop alphas differ so
//! fading to transparent (`#00000000`) never introduces a grey fringe, and in
//! integer fixed-point (`1/65536`) so all CPUs agree to the byte.

use std::sync::Arc;

use crate::ops::Rgba8;

pub mod limits {
    pub const STOPS: std::ops::RangeInclusive<usize> = 2..=32;
    pub const OVERALL_LEVELS: std::ops::RangeInclusive<u8> = 2..=64;
    pub const SEGMENT_LEVELS: std::ops::RangeInclusive<u8> = 1..=32;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// Position along the gradient in `0.0..=1.0`, non-decreasing across stops.
    pub position: f32,
    pub color: Rgba8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub stops: Arc<[GradientStop]>,
}

impl Default for Gradient {
    fn default() -> Self {
        Self::from_preset(&PRESETS[0])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradientShape {
    Linear,
    Radial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuantizeMode {
    /// `levels` (`2..=64`) evenly spaced samples across `[0.0, 1.0]`.
    Overall(u8),
    /// `levels` (`1..=32`) subdivisions within every stop pair (`1` = stop
    /// colours only).
    PerSegment(u8),
}

impl Default for QuantizeMode {
    fn default() -> Self {
        Self::Overall(4)
    }
}

impl QuantizeMode {
    pub fn is_valid(self) -> bool {
        match self {
            Self::Overall(levels) => limits::OVERALL_LEVELS.contains(&levels),
            Self::PerSegment(levels) => limits::SEGMENT_LEVELS.contains(&levels),
        }
    }

    pub fn clamped(self) -> Self {
        match self {
            Self::Overall(levels) => Self::Overall(levels.clamp(
                *limits::OVERALL_LEVELS.start(),
                *limits::OVERALL_LEVELS.end(),
            )),
            Self::PerSegment(levels) => Self::PerSegment(levels.clamp(
                *limits::SEGMENT_LEVELS.start(),
                *limits::SEGMENT_LEVELS.end(),
            )),
        }
    }
}

impl Gradient {
    pub fn new(stops: Vec<GradientStop>) -> Self {
        Self {
            stops: Arc::from(stops),
        }
    }

    pub fn from_preset(preset: &GradientPreset) -> Self {
        Self {
            stops: Arc::from(preset.stops),
        }
    }

    pub fn is_valid(&self) -> bool {
        limits::STOPS.contains(&self.stops.len())
            && self
                .stops
                .iter()
                .all(|stop| (0.0..=1.0).contains(&stop.position))
            && self
                .stops
                .windows(2)
                .all(|pair| pair[0].position <= pair[1].position)
    }

    pub fn reversed(&self) -> Self {
        let mut stops: Vec<GradientStop> = self
            .stops
            .iter()
            .rev()
            .map(|stop| GradientStop {
                position: (1.0 - stop.position).clamp(0.0, 1.0),
                color: stop.color,
            })
            .collect();
        // Floating-point subtraction can invert adjacent identical positions by 1 ulp.
        for index in 1..stops.len() {
            if stops[index].position < stops[index - 1].position {
                stops[index].position = stops[index - 1].position;
            }
        }
        Self::new(stops)
    }

    /// Samples at normalized position `t` (`0.0..=1.0`).
    pub fn sample(&self, t: f32) -> Rgba8 {
        self.sample_q16(to_q16(t))
    }

    /// Samples at fixed-point position `t_q16` in `0..=65536`.
    pub fn sample_q16(&self, t_q16: u32) -> Rgba8 {
        if self.stops.is_empty() {
            return Rgba8([0, 0, 0, 0]);
        }
        if self.stops.len() == 1 {
            return self.stops[0].color;
        }
        let t = t_q16.min(65536);
        let first = to_q16(self.stops[0].position);
        if t <= first {
            return self.stops[0].color;
        }
        for pair in self.stops.windows(2) {
            let p0 = to_q16(pair[0].position);
            let p1 = to_q16(pair[1].position);
            if t <= p1 {
                if p1 <= p0 {
                    return pair[1].color;
                }
                let frac = ((u64::from(t - p0) << 16) / u64::from(p1 - p0)) as u32;
                return lerp_rgba8(pair[0].color, pair[1].color, frac);
            }
        }
        self.stops[self.stops.len() - 1].color
    }
}

#[inline]
pub fn to_q16(t: f32) -> u32 {
    if !t.is_finite() {
        return 0;
    }
    (f64::from(t.clamp(0.0, 1.0)) * 65536.0 + 0.5) as u32
}

/// Linearly interpolates straight sRGB `a` to `b` by `frac_q16` (`0..=65536`),
/// interpolating in premultiplied space when alphas differ.
#[inline]
pub fn lerp_rgba8(a: Rgba8, b: Rgba8, frac_q16: u32) -> Rgba8 {
    let f = frac_q16.min(65536);
    if f == 0 {
        return a;
    }
    if f == 65536 {
        return b;
    }
    let inv = 65536 - f;
    let mix = |va: u32, vb: u32| ((va * inv + vb * f + 32768) >> 16) as u8;
    if a.0[3] == b.0[3] {
        return Rgba8([
            mix(u32::from(a.0[0]), u32::from(b.0[0])),
            mix(u32::from(a.0[1]), u32::from(b.0[1])),
            mix(u32::from(a.0[2]), u32::from(b.0[2])),
            a.0[3],
        ]);
    }
    let premul = |c: Rgba8| -> [u32; 4] {
        let alpha = u32::from(c.0[3]);
        [
            (u32::from(c.0[0]) * alpha + 127) / 255,
            (u32::from(c.0[1]) * alpha + 127) / 255,
            (u32::from(c.0[2]) * alpha + 127) / 255,
            alpha,
        ]
    };
    let pa = premul(a);
    let pb = premul(b);
    let alpha = mix(pa[3], pb[3]);
    if alpha == 0 {
        return Rgba8([0, 0, 0, 0]);
    }
    let alpha_u32 = u32::from(alpha);
    let unpremul = |c0: u32, c1: u32| -> u8 {
        let pm = (c0 * inv + c1 * f + 32768) >> 16;
        ((pm * 255 + alpha_u32 / 2) / alpha_u32).min(255) as u8
    };
    Rgba8([
        unpremul(pa[0], pb[0]),
        unpremul(pa[1], pb[1]),
        unpremul(pa[2], pb[2]),
        alpha,
    ])
}

/// Precomputed quantized levels for a `(Gradient, QuantizeMode)` pair.
#[derive(Clone, Debug, PartialEq)]
pub struct QuantizedRamp {
    /// Non-decreasing positions in `0..=65536`.
    pub positions: Vec<u32>,
    /// One colour per entry in `positions`.
    pub colors: Vec<Rgba8>,
}

impl QuantizedRamp {
    pub fn new(gradient: &Gradient, mode: QuantizeMode) -> Self {
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        match mode.clamped() {
            QuantizeMode::Overall(levels) => {
                let count = u32::from(levels);
                for index in 0..count {
                    let pos = (index * 65536 + (count - 1) / 2) / (count - 1);
                    positions.push(pos);
                    colors.push(gradient.sample_q16(pos));
                }
            }
            QuantizeMode::PerSegment(levels) => {
                if gradient.stops.is_empty() {
                    return Self {
                        positions: vec![0, 65536],
                        colors: vec![Rgba8([0; 4]), Rgba8([0; 4])],
                    };
                }
                if gradient.stops.len() == 1 {
                    let c = gradient.stops[0].color;
                    return Self {
                        positions: vec![0],
                        colors: vec![c],
                    };
                }
                let sub = u32::from(levels);
                for pair in gradient.stops.windows(2) {
                    let p0 = to_q16(pair[0].position);
                    let p1 = to_q16(pair[1].position);
                    if p1 <= p0 {
                        if positions.last().copied() != Some(p0)
                            || colors.last().copied() != Some(pair[0].color)
                        {
                            positions.push(p0);
                            colors.push(pair[0].color);
                        }
                        if positions.last().copied() != Some(p1)
                            || colors.last().copied() != Some(pair[1].color)
                        {
                            positions.push(p1);
                            colors.push(pair[1].color);
                        }
                        continue;
                    }
                    for step in 0..=sub {
                        let frac = ((step << 16) + sub / 2) / sub;
                        let pos = p0
                            + ((u64::from(p1 - p0) * u64::from(step) + u64::from(sub / 2))
                                / u64::from(sub)) as u32;
                        let color = lerp_rgba8(pair[0].color, pair[1].color, frac);
                        if positions.last().copied() == Some(pos)
                            && colors.last().copied() == Some(color)
                        {
                            continue;
                        }
                        positions.push(pos);
                        colors.push(color);
                    }
                }
            }
        }
        Self { positions, colors }
    }

    /// Returns `(left, right, local_frac_q16)` bracketing `t_q16` for ordered
    /// dithering; `local_frac_q16 == 0` when clamped outside the ramp or at a
    /// hard stop step.
    #[inline]
    pub fn ordered_pair(&self, t_q16: u32) -> (Rgba8, Rgba8, u32) {
        let last = self.positions.len() - 1;
        if t_q16 <= self.positions[0] {
            return (self.colors[0], self.colors[0], 0);
        }
        if t_q16 >= self.positions[last] {
            return (self.colors[last], self.colors[last], 0);
        }
        let mut idx = self.positions.partition_point(|&p| p <= t_q16);
        // `partition_point` returns the first element strictly greater than `t_q16`.
        let low = idx.saturating_sub(1);
        idx = idx.min(last);
        let p0 = self.positions[low];
        let p1 = self.positions[idx];
        if p1 <= p0 {
            return (self.colors[low], self.colors[low], 0);
        }
        let frac = ((u64::from(t_q16 - p0) << 16) / u64::from(p1 - p0)) as u32;
        (self.colors[low], self.colors[idx], frac)
    }

    /// Returns the `(position_q16, color)` of the nearest quantized level to `t_q16`.
    #[inline]
    pub fn nearest(&self, t_q16: i32) -> (u32, Rgba8) {
        let t = t_q16.clamp(0, 65536) as u32;
        let idx = self.positions.partition_point(|&p| p < t);
        if idx == 0 {
            return (self.positions[0], self.colors[0]);
        }
        if idx >= self.positions.len() {
            let last = self.positions.len() - 1;
            return (self.positions[last], self.colors[last]);
        }
        let prev = idx - 1;
        let d_prev = t - self.positions[prev];
        let d_next = self.positions[idx] - t;
        let pick = if d_next < d_prev { idx } else { prev };
        (self.positions[pick], self.colors[pick])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientPreset {
    pub id: &'static str,
    pub stops: &'static [GradientStop],
}

const fn stop(position: f32, r: u8, g: u8, b: u8, a: u8) -> GradientStop {
    GradientStop {
        position,
        color: Rgba8([r, g, b, a]),
    }
}

pub const PRESETS: [GradientPreset; 7] = [
    GradientPreset {
        id: "black-white",
        stops: &[stop(0.0, 0, 0, 0, 255), stop(1.0, 255, 255, 255, 255)],
    },
    GradientPreset {
        id: "ink-transparent",
        stops: &[stop(0.0, 0, 0, 0, 255), stop(1.0, 0, 0, 0, 0)],
    },
    GradientPreset {
        id: "gameboy",
        stops: &[
            stop(0.0, 15, 56, 15, 255),
            stop(0.33, 48, 98, 48, 255),
            stop(0.67, 139, 172, 15, 255),
            stop(1.0, 155, 188, 15, 255),
        ],
    },
    GradientPreset {
        id: "warm-sepia",
        stops: &[
            stop(0.0, 36, 20, 12, 255),
            stop(0.45, 142, 88, 52, 255),
            stop(1.0, 246, 228, 198, 255),
        ],
    },
    GradientPreset {
        id: "sunset",
        stops: &[
            stop(0.0, 34, 24, 72, 255),
            stop(0.42, 198, 58, 92, 255),
            stop(0.76, 246, 148, 62, 255),
            stop(1.0, 255, 232, 146, 255),
        ],
    },
    GradientPreset {
        id: "cyber-neon",
        stops: &[
            stop(0.0, 12, 8, 36, 255),
            stop(0.5, 232, 42, 148, 255),
            stop(1.0, 64, 242, 248, 255),
        ],
    },
    GradientPreset {
        id: "heatmap",
        stops: &[
            stop(0.0, 10, 12, 42, 255),
            stop(0.35, 138, 28, 128, 255),
            stop(0.7, 238, 92, 32, 255),
            stop(1.0, 252, 244, 154, 255),
        ],
    },
];

pub fn find_preset(id: &str) -> Option<&'static GradientPreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_valid_and_named_once() {
        let mut ids: Vec<_> = PRESETS.iter().map(|preset| preset.id).collect();
        for preset in PRESETS {
            let gradient = Gradient::from_preset(&preset);
            assert!(gradient.is_valid(), "{}", preset.id);
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PRESETS.len());
    }

    #[test]
    fn gradient_interpolates_multi_stop_and_transparent_without_dark_fringe() {
        let ramp = Gradient::new(vec![
            stop(0.0, 255, 0, 0, 255),
            stop(0.5, 0, 255, 0, 255),
            stop(1.0, 0, 0, 0, 0),
        ]);
        assert_eq!(ramp.sample(0.0), Rgba8([255, 0, 0, 255]));
        assert_eq!(ramp.sample(0.5), Rgba8([0, 255, 0, 255]));
        assert_eq!(ramp.sample(1.0), Rgba8([0, 0, 0, 0]));
        // Midpoint of green -> transparent keeps pure green hue at half alpha.
        let fade = ramp.sample(0.75);
        assert_eq!(fade.0[0], 0);
        assert!(fade.0[1] >= 254, "green stayed bright: {fade:?}");
        assert!((i32::from(fade.0[3]) - 128).abs() <= 1);
    }

    #[test]
    fn per_segment_one_uses_only_stop_colors_while_overall_uses_even_steps() {
        let gameboy = Gradient::from_preset(&PRESETS[2]);
        let stops_only = QuantizedRamp::new(&gameboy, QuantizeMode::PerSegment(1));
        assert_eq!(stops_only.colors.len(), gameboy.stops.len());
        let overall = QuantizedRamp::new(&gameboy, QuantizeMode::Overall(5));
        assert_eq!(overall.positions, vec![0, 16384, 32768, 49152, 65536]);
    }

    #[test]
    fn reversing_flips_stop_order_and_positions() {
        let g = Gradient::new(vec![
            stop(0.0, 10, 20, 30, 255),
            stop(0.25, 40, 50, 60, 255),
            stop(1.0, 70, 80, 90, 255),
        ]);
        let rev = g.reversed();
        assert!(rev.is_valid());
        assert_eq!(rev.stops[0].color, Rgba8([70, 80, 90, 255]));
        assert!((rev.stops[1].position - 0.75).abs() < 1e-6);
        assert_eq!(rev.stops[2].color, Rgba8([10, 20, 30, 255]));
    }
}
