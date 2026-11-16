// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 NewfolderGames

//! Pixel-art dithering, quantization and screentone kernels.
//!
//! Every kernel runs on deterministic integer arithmetic so outputs match byte
//! for byte across threads and CPU targets.

use crate::gradient::{Gradient, QuantizedRamp};
use crate::ops::Rgba8;

pub mod limits {
    pub const TONE_SIZE: std::ops::RangeInclusive<f32> = 2.0..=64.0;
    pub const TONE_ANGLE: std::ops::RangeInclusive<f32> = -90.0..=90.0;
    pub const VALID_ANGLE: std::ops::RangeInclusive<f32> = -180.0..=180.0;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dither {
    None,
    Constant,
    Checker,
    HorizontalLines,
    VerticalLines,
    Diagonal,
    Crosshatch,
    Halftone,
    Bayer2,
    #[default]
    Bayer4,
    Bayer8,
    FloydSteinberg,
    Atkinson,
}

impl Dither {
    pub const ALL: [Self; 13] = [
        Self::None,
        Self::Constant,
        Self::Checker,
        Self::HorizontalLines,
        Self::VerticalLines,
        Self::Diagonal,
        Self::Crosshatch,
        Self::Halftone,
        Self::Bayer2,
        Self::Bayer4,
        Self::Bayer8,
        Self::FloydSteinberg,
        Self::Atkinson,
    ];

    pub fn is_error_diffusion(self) -> bool {
        matches!(self, Self::FloydSteinberg | Self::Atkinson)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToneShape {
    #[default]
    Dot,
    Square,
    Line,
    Cross,
    Bayer,
}

impl ToneShape {
    pub const ALL: [Self; 5] = [
        Self::Dot,
        Self::Square,
        Self::Line,
        Self::Cross,
        Self::Bayer,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screentone {
    pub shape: ToneShape,
    pub size: f32,
    pub angle: f32,
    pub dot_color: Rgba8,
    /// When `None`, non-dot pixels leave the underlying image unchanged.
    pub background: Option<Rgba8>,
}

impl Default for Screentone {
    fn default() -> Self {
        Self {
            shape: ToneShape::Dot,
            size: 8.0,
            angle: 45.0,
            dot_color: Rgba8([0, 0, 0, 255]),
            background: Some(Rgba8([255, 255, 255, 255])),
        }
    }
}

impl Screentone {
    pub fn is_valid(&self) -> bool {
        limits::TONE_SIZE.contains(&self.size) && limits::VALID_ANGLE.contains(&self.angle)
    }
}

/// Rec.709 integer luma (`0..=65280`) from straight `[r, g, b]`.
#[inline]
pub fn luma_u16([r, g, b]: [u8; 3]) -> u16 {
    54 * u16::from(r) + 183 * u16::from(g) + 19 * u16::from(b)
}

/// Rec.709 luma scaled to `0..=65536`.
#[inline]
pub fn luma_q16(rgb: [u8; 3]) -> u32 {
    let l = u32::from(luma_u16(rgb));
    (l * 65536 + 32640) / 65280
}

const BAYER_2: [[u8; 2]; 2] = [[0, 2], [3, 1]];

const BAYER_4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

const fn make_bayer_8() -> [[u8; 8]; 8] {
    let mut table = [[0u8; 8]; 8];
    let mut y = 0;
    while y < 8 {
        let mut x = 0;
        while x < 8 {
            table[y][x] = BAYER_4[y & 3][x & 3] * 4 + BAYER_2[y >> 2][x >> 2];
            x += 1;
        }
        y += 1;
    }
    table
}

const BAYER_8: [[u8; 8]; 8] = make_bayer_8();

const CHECKER_2: [[u8; 2]; 2] = [[0, 1], [1, 0]];
const LINE_4: [u8; 4] = [0, 2, 1, 3];
const CROSSHATCH_4: [[u8; 4]; 4] = [[0, 3, 1, 5], [4, 12, 8, 13], [2, 9, 7, 10], [6, 14, 11, 15]];

// Clean unique permutation 0..=63 for 8×8 staggered halftone screen.
const HALFTONE_SCREEN_8: [[u8; 8]; 8] = make_halftone_8();

const fn make_halftone_8() -> [[u8; 8]; 8] {
    // Score every cell by its spot height around (1.5, 1.5) and (5.5, 5.5) vs
    // corner holes at (5.5, 1.5) and (1.5, 5.5), then assign ranks 0..=63.
    let mut scores = [0i64; 64];
    let mut y = 0;
    while y < 8 {
        let mut x = 0;
        while x < 8 {
            // Two staggered cell centers in Q4 coordinates (multiplied by 2).
            let dx0 = (x as i64 * 2 - 3).abs();
            let dy0 = (y as i64 * 2 - 3).abs();
            let dx1 = (x as i64 * 2 - 11).abs();
            let dy1 = (y as i64 * 2 - 11).abs();
            let d_dot = {
                let d0 = dx0 * dx0 + dy0 * dy0;
                let d1 = dx1 * dx1 + dy1 * dy1;
                if d0 < d1 { d0 } else { d1 }
            };
            let cx0 = (x as i64 * 2 - 11).abs();
            let cy0 = (y as i64 * 2 - 3).abs();
            let cx1 = (x as i64 * 2 - 3).abs();
            let cy1 = (y as i64 * 2 - 11).abs();
            let d_hole = {
                let h0 = cx0 * cx0 + cy0 * cy0;
                let h1 = cx1 * cx1 + cy1 * cy1;
                if h0 < h1 { h0 } else { h1 }
            };
            // Subtract hole distance so dots grow smoothly from centers to diamond to holes.
            scores[y * 8 + x] = (d_dot - d_hole) * 64 + (y * 8 + x) as i64;
            x += 1;
        }
        y += 1;
    }
    let mut out = [[0u8; 8]; 8];
    let mut i = 0;
    while i < 64 {
        let mut rank = 0u8;
        let mut j = 0;
        while j < 64 {
            if scores[j] < scores[i] {
                rank += 1;
            }
            j += 1;
        }
        out[i / 8][i % 8] = rank;
        i += 1;
    }
    out
}

/// Ordered dithering threshold in `1..65535` for positional `Dither` modes.
#[inline]
pub fn threshold_q16(dither: Dither, x: i32, y: i32) -> u32 {
    let scale = |rank: u8, count: u32| -> u32 { ((2 * u32::from(rank) + 1) * 65536) / (2 * count) };
    match dither {
        Dither::None | Dither::Constant | Dither::FloydSteinberg | Dither::Atkinson => 32768,
        Dither::Checker => {
            let ux = x.rem_euclid(2) as usize;
            let uy = y.rem_euclid(2) as usize;
            scale(CHECKER_2[uy][ux], 2)
        }
        Dither::HorizontalLines => scale(LINE_4[y.rem_euclid(4) as usize], 4),
        Dither::VerticalLines => scale(LINE_4[x.rem_euclid(4) as usize], 4),
        Dither::Diagonal => scale(LINE_4[(x + y).rem_euclid(4) as usize], 4),
        Dither::Crosshatch => {
            let ux = x.rem_euclid(4) as usize;
            let uy = y.rem_euclid(4) as usize;
            scale(CROSSHATCH_4[uy][ux], 16)
        }
        Dither::Halftone => {
            let ux = x.rem_euclid(8) as usize;
            let uy = y.rem_euclid(8) as usize;
            scale(HALFTONE_SCREEN_8[uy][ux], 64)
        }
        Dither::Bayer2 => {
            let ux = x.rem_euclid(2) as usize;
            let uy = y.rem_euclid(2) as usize;
            scale(BAYER_2[uy][ux], 4)
        }
        Dither::Bayer4 => {
            let ux = x.rem_euclid(4) as usize;
            let uy = y.rem_euclid(4) as usize;
            scale(BAYER_4[uy][ux], 16)
        }
        Dither::Bayer8 => {
            let ux = x.rem_euclid(8) as usize;
            let uy = y.rem_euclid(8) as usize;
            scale(BAYER_8[uy][ux], 64)
        }
    }
}

/// Evaluates `gradient` / `ramp` at fixed-point `t_q16` for any non-error-diffusion mode.
#[inline]
pub fn sample_positioned(
    gradient: &Gradient,
    ramp: &QuantizedRamp,
    dither: Dither,
    t_q16: u32,
    x: i32,
    y: i32,
) -> Rgba8 {
    match dither {
        Dither::None => gradient.sample_q16(t_q16),
        Dither::Constant => {
            let (low, high, frac) = ramp.ordered_pair(t_q16);
            if frac >= 32768 { high } else { low }
        }
        _ => {
            let (low, high, frac) = ramp.ordered_pair(t_q16);
            if frac == 0 {
                return low;
            }
            if frac > threshold_q16(dither, x, y) {
                high
            } else {
                low
            }
        }
    }
}

/// Runs 1D fixed-point error diffusion (`FloydSteinberg` or `Atkinson`) over a
/// `width × height` plane. Pixels where `t_at(x, y)` returns `None` emit no
/// output and do not diffuse error.
pub fn diffuse_plane(
    ramp: &QuantizedRamp,
    dither: Dither,
    width: usize,
    height: usize,
    mut t_at: impl FnMut(usize, usize) -> Option<u32>,
    mut put: impl FnMut(usize, usize, Rgba8),
) {
    if width == 0 || height == 0 {
        return;
    }
    let row_len = width + 4;
    let mut row0 = vec![0i32; row_len];
    let mut row1 = vec![0i32; row_len];
    let mut row2 = vec![0i32; row_len];
    let atkinson = dither == Dither::Atkinson;
    for y in 0..height {
        for x in 0..width {
            let err_in = row0[x + 2];
            let Some(t_q16) = t_at(x, y) else {
                continue;
            };
            let adjusted = (t_q16 as i32 + err_in).clamp(0, 65536);
            let (chosen_q16, color) = ramp.nearest(adjusted);
            put(x, y, color);
            let error = adjusted - chosen_q16 as i32;
            if error == 0 {
                continue;
            }
            if atkinson {
                let share = error >> 3;
                row0[x + 3] += share;
                row0[x + 4] += share;
                row1[x + 1] += share;
                row1[x + 2] += share;
                row1[x + 3] += share;
                row2[x + 2] += share;
            } else {
                row0[x + 3] += (error * 7) >> 4;
                row1[x + 1] += (error * 3) >> 4;
                row1[x + 2] += (error * 5) >> 4;
                row1[x + 3] += error >> 4;
            }
        }
        std::mem::swap(&mut row0, &mut row1);
        std::mem::swap(&mut row1, &mut row2);
        row2.fill(0);
    }
}

/// 257-entry `cos(2 * pi * i / 256)` table in `Q15` (`-32768..=32767`), built
/// with a `const` quarter-wave minimax polynomial for deterministic cross-CPU results.
const fn cos_q15_quarter(phase: u32) -> i32 {
    // `phase` in `0..=64` corresponds to `0..=pi/2`.
    // 4-term minimax Taylor polynomial for cos(x), x = phase * pi / 128 in Q15:
    // Error across the quarter wave is under 1 LSB of Q15.
    const PI_HALF_Q16: i64 = 102_944; // round(pi/2 * 65536)
    let x = (phase as i64 * PI_HALF_Q16) >> 6; // Q16 radians in [0, 102944]
    let x2 = (x * x) >> 16; // Q16
    let x4 = (x2 * x2) >> 16; // Q16
    let x6 = (x4 * x2) >> 16; // Q16
    let term2 = x2 >> 1; // x^2 / 2
    let term4 = (x4 * 2731) >> 16; // x^4 / 24
    let term6 = (x6 * 91) >> 16; // x^6 / 720
    let val = 65536 - term2 + term4 - term6;
    ((val + 1) >> 1) as i32 // Q15
}

const fn make_cos_lut() -> [i32; 257] {
    let mut lut = [0i32; 257];
    let mut i = 0;
    while i <= 256 {
        let q = i / 64;
        let r = i % 64;
        lut[i] = match q {
            0 => cos_q15_quarter(r as u32),
            1 => -cos_q15_quarter((64 - r) as u32),
            2 => -cos_q15_quarter(r as u32),
            3 => cos_q15_quarter((64 - r) as u32),
            _ => cos_q15_quarter(0),
        };
        i += 1;
    }
    lut
}

const COS_LUT: [i32; 257] = make_cos_lut();

#[inline]
fn cos_q15_from_q16(phase_q16: i32) -> i32 {
    let u = phase_q16.rem_euclid(65536) as usize;
    let idx = u >> 8;
    let frac = (u & 0xff) as i32;
    let a = COS_LUT[idx];
    let b = COS_LUT[idx + 1];
    a + (((b - a) * frac + 128) >> 8)
}

/// Precomputed rotated screen grid for fast row evaluation of [`Screentone`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreentoneGrid {
    shape: ToneShape,
    /// `cos(angle) / period` and `sin(angle) / period` in `Q16` cell units/px.
    cos_step: i64,
    sin_step: i64,
    /// Step per pixel for Bayer (`65536 / period`) when unrotated.
    bayer_step: i64,
}

impl ScreentoneGrid {
    pub fn new(tone: &Screentone, shrink: u32) -> Self {
        let period = (f64::from(tone.size) / f64::from(shrink.max(1))).max(1.0);
        let (sin, cos) = f64::from(tone.angle).to_radians().sin_cos();
        Self {
            shape: tone.shape,
            cos_step: ((cos / period) * 65536.0).round() as i64,
            sin_step: ((sin / period) * 65536.0).round() as i64,
            bayer_step: ((1.0 / period) * 65536.0).round().max(1.0) as i64,
        }
    }

    /// Returns `true` when document pixel `(x, y)` with straight RGB luma
    /// `luma_u16` (`0..=65280`) falls inside the halftone dot.
    #[inline]
    pub fn is_dot(&self, x: i32, y: i32, luma_u16: u16) -> bool {
        if luma_u16 >= 65280 {
            return false;
        }
        let darkness_q16 = 65536 - ((u32::from(luma_u16) * 65536 + 32640) / 65280);
        if luma_u16 == 0 {
            return true;
        }
        if self.shape == ToneShape::Bayer {
            let bx = (((i64::from(x) * 2 + 1) * self.bayer_step) >> 1) >> 13;
            let by = (((i64::from(y) * 2 + 1) * self.bayer_step) >> 1) >> 13;
            let th = threshold_q16(Dither::Bayer8, bx as i32, by as i32);
            return darkness_q16 > th;
        }
        // Evaluate at pixel centre `(x + 0.5, y + 0.5)`.
        let px2 = i64::from(x) * 2 + 1;
        let py2 = i64::from(y) * 2 + 1;
        let u_q16 = (px2 * self.cos_step + py2 * self.sin_step) >> 1;
        let v_q16 = (-px2 * self.sin_step + py2 * self.cos_step) >> 1;
        let u_cell = u_q16.rem_euclid(65536) as i32 - 32768;
        let v_cell = v_q16.rem_euclid(65536) as i32 - 32768;
        let threshold = match self.shape {
            ToneShape::Square => {
                let d = u_cell.unsigned_abs().max(v_cell.unsigned_abs());
                (d * d) >> 14
            }
            ToneShape::Line => v_cell.unsigned_abs() * 2,
            ToneShape::Cross => {
                let d = u_cell.unsigned_abs().min(v_cell.unsigned_abs());
                let inv = 32768 - d;
                65536 - ((inv * inv) >> 14)
            }
            ToneShape::Dot => {
                // `cos(2*pi*u) + cos(2*pi*v)` maps center (+65536) -> diamond (0) -> corners (-65536).
                // Flipping signs turns darkness threshold into `0..=65536`.
                let sum = cos_q15_from_q16(u_cell) + cos_q15_from_q16(v_cell);
                ((65536 - sum) / 2).clamp(0, 65536) as u32
            }
            ToneShape::Bayer => unreachable!(),
        };
        darkness_q16 > threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gradient::{GradientPreset, QuantizeMode};

    #[test]
    fn ordered_dithers_preserve_pure_endpoints() {
        let ramp_preset = GradientPreset {
            id: "bw",
            stops: &[
                crate::gradient::GradientStop {
                    position: 0.0,
                    color: Rgba8([0, 0, 0, 255]),
                },
                crate::gradient::GradientStop {
                    position: 1.0,
                    color: Rgba8([255, 255, 255, 255]),
                },
            ],
        };
        let g = Gradient::from_preset(&ramp_preset);
        let q = QuantizedRamp::new(&g, QuantizeMode::Overall(2));
        for dither in Dither::ALL {
            if dither.is_error_diffusion() {
                continue;
            }
            for y in -2..10 {
                for x in -2..10 {
                    assert_eq!(
                        sample_positioned(&g, &q, dither, 0, x, y),
                        Rgba8([0, 0, 0, 255]),
                        "{dither:?} at 0"
                    );
                    assert_eq!(
                        sample_positioned(&g, &q, dither, 65536, x, y),
                        Rgba8([255, 255, 255, 255]),
                        "{dither:?} at 1"
                    );
                }
            }
        }
    }

    #[test]
    fn halftone_matrix_is_a_permutation_of_0_through_63() {
        let mut seen = [false; 64];
        for row in HALFTONE_SCREEN_8 {
            for val in row {
                assert!(!seen[val as usize], "duplicate {val}");
                seen[val as usize] = true;
            }
        }
        assert!(seen.into_iter().all(|v| v));
    }

    #[test]
    fn error_diffusion_hits_expected_average_brightness() {
        let g = Gradient::default();
        let q = QuantizedRamp::new(&g, QuantizeMode::Overall(2));
        for dither in [Dither::FloydSteinberg, Dither::Atkinson] {
            let mut sum = 0u32;
            diffuse_plane(
                &q,
                dither,
                64,
                64,
                |_, _| Some(32768),
                |_, _, c| {
                    if c.0[0] > 128 {
                        sum += 1;
                    }
                },
            );
            let ratio = f32::from(sum as u16) / (64.0 * 64.0);
            assert!((ratio - 0.5).abs() < 0.05, "{dither:?} ratio={ratio}");
        }
    }

    #[test]
    fn screentone_grid_matches_extremes_and_fifty_percent_duty() {
        for shape in ToneShape::ALL {
            let tone = Screentone {
                shape,
                size: 8.0,
                angle: if shape == ToneShape::Bayer { 0.0 } else { 45.0 },
                ..Screentone::default()
            };
            let grid = ScreentoneGrid::new(&tone, 1);
            for y in 0..16 {
                for x in 0..16 {
                    assert!(grid.is_dot(x, y, 0), "{shape:?} black must dot");
                    assert!(!grid.is_dot(x, y, 65280), "{shape:?} white must not dot");
                }
            }
        }
    }
}
