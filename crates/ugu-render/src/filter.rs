// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 NewfolderGames

//! Filter layer pass evaluation (`GradientMap` and `Screentone`) in premultiplied RGBA8.

use std::sync::Arc;

use ugu_core::dither::{self, Dither, ScreentoneGrid};
use ugu_core::gradient::{Gradient, QuantizedRamp};
use ugu_core::ops::{Blend, FilterKind, FilterLayer, Rgba8};
use vello_cpu::Pixmap;

use crate::compose::premultiplied;
use crate::composite::blend;
use crate::plan::Composite;
use crate::tile::Pixel;

#[inline]
fn mul(a: u8, b: u8) -> u8 {
    ((u16::from(a) * u16::from(b) + 255) >> 8) as u8
}

#[inline]
fn div255(value: u32) -> u8 {
    ((value + 127) / 255) as u8
}

#[inline]
fn mix_u8(target: u8, filtered: u8, w: u8) -> u8 {
    if w == 0 {
        return target;
    }
    if w == 255 {
        return filtered;
    }
    let inv = 255 - u16::from(w);
    let w16 = u16::from(w);
    ((u16::from(target) * inv + u16::from(filtered) * w16 + 127) / 255) as u8
}

/// Scales straight `Rgba8` by backdrop alpha `ba` into premultiplied RGBA8.
#[inline]
fn scaled_by_alpha(straight: Rgba8, ba: u8) -> Pixel {
    let [r, g, b, a] = straight.0;
    if ba == 0 || a == 0 {
        return [0; 4];
    }
    if a == 255 && ba == 255 {
        return [r, g, b, 255];
    }
    let out_a = if a == 255 {
        ba
    } else if ba == 255 {
        a
    } else {
        div255(u32::from(a) * u32::from(ba))
    };
    premultiplied([r, g, b, out_a])
}

/// Render-plan specification for a filter layer step.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterSpec {
    pub composite: Composite,
    pub kind: FilterKind,
    pub shrink: u32,
}

impl FilterSpec {
    pub fn new(composite: Composite, kind: FilterKind) -> Self {
        Self {
            composite,
            kind,
            shrink: 1,
        }
    }

    pub fn scaled(&self, shrink: u32) -> Self {
        Self {
            shrink: shrink.max(1),
            ..self.clone()
        }
    }

    pub fn pass(&self) -> Arc<FilterPass> {
        Arc::new(FilterPass::new(self.composite, &self.kind, self.shrink))
    }
}

/// Prepared filter evaluator attached to `composite::Put::Filter`.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterPass {
    pub composite: Composite,
    opacity: u8,
    kind: PreparedFilter,
}

#[derive(Clone, Debug, PartialEq)]
enum PreparedFilter {
    GradientMap {
        gradient: Gradient,
        ramp: QuantizedRamp,
        dither: Dither,
    },
    Screentone {
        grid: ScreentoneGrid,
        dot_straight: Rgba8,
        dot_opaque: Pixel,
        bg_straight: Option<Rgba8>,
        bg_opaque: Option<Pixel>,
    },
}

impl FilterPass {
    pub fn new(composite: Composite, kind: &FilterKind, shrink: u32) -> Self {
        let opacity = (composite.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        let prepared = match kind {
            FilterKind::GradientMap {
                gradient,
                dither,
                quantize,
            } => PreparedFilter::GradientMap {
                gradient: gradient.clone(),
                ramp: QuantizedRamp::new(gradient, *quantize),
                dither: *dither,
            },
            FilterKind::Screentone(tone) => PreparedFilter::Screentone {
                grid: ScreentoneGrid::new(tone, shrink.max(1)),
                dot_straight: tone.dot_color,
                dot_opaque: premultiplied(tone.dot_color.0),
                bg_straight: tone.background,
                bg_opaque: tone.background.map(|bg| premultiplied(bg.0)),
            },
        };
        Self {
            composite,
            opacity,
            kind: prepared,
        }
    }

    pub fn from_layer(layer: &FilterLayer, shrink: u32) -> Self {
        let composite = Composite {
            blend: layer.blend,
            opacity: layer.opacity,
            clipped: layer.clip_to_below,
            moved: None,
        };
        Self::new(composite, &layer.kind, shrink)
    }

    pub fn is_whole_plane(&self) -> bool {
        matches!(
            self.kind,
            PreparedFilter::GradientMap { dither, .. } if dither.is_error_diffusion()
        )
    }

    /// Evaluates positional filters (`Screentone` or non-error-diffusion
    /// `GradientMap`) over one span row in `row[0..span]`, starting at document
    /// x `left` and y `y`.
    pub fn apply_row(
        &self,
        y: usize,
        left: usize,
        row: &mut [Pixel],
        base: Option<(&[Pixel], u8)>,
    ) {
        if self.opacity == 0 {
            return;
        }
        if self.composite.clipped && base.is_none() {
            return;
        }
        let yi = y as i32;
        for (offset, target) in row.iter_mut().enumerate() {
            let ba = target[3];
            if ba == 0 {
                continue;
            }
            let xi = (left + offset) as i32;
            let straight = Rgba8::from_premultiplied(*target);
            let filtered = match &self.kind {
                PreparedFilter::GradientMap {
                    gradient,
                    ramp,
                    dither,
                } => {
                    let t = dither::luma_q16([straight.0[0], straight.0[1], straight.0[2]]);
                    let mapped = dither::sample_positioned(gradient, ramp, *dither, t, xi, yi);
                    scaled_by_alpha(mapped, ba)
                }
                PreparedFilter::Screentone {
                    grid,
                    dot_straight,
                    dot_opaque,
                    bg_straight,
                    bg_opaque,
                } => {
                    let luma = dither::luma_u16([straight.0[0], straight.0[1], straight.0[2]]);
                    if grid.is_dot(xi, yi, luma) {
                        if ba == 255 {
                            *dot_opaque
                        } else {
                            scaled_by_alpha(*dot_straight, ba)
                        }
                    } else if ba == 255 {
                        bg_opaque.unwrap_or(*target)
                    } else {
                        bg_straight.map_or(*target, |bg| scaled_by_alpha(bg, ba))
                    }
                }
            };
            self.composite_pixel(target, filtered, base, offset);
        }
    }

    /// Evaluates whole-plane error-diffusion (`FloydSteinberg` / `Atkinson`)
    /// filters over `target`.
    pub fn apply_plane(&self, target: &mut Pixmap, base: Option<(&[Pixel], u8)>) {
        if !self.is_whole_plane() {
            let width = usize::from(target.width());
            let height = usize::from(target.height());
            let pixels = target.data_as_u8_slice_mut().as_chunks_mut::<4>().0;
            for y in 0..height {
                let row = &mut pixels[y * width..(y + 1) * width];
                let base_row = base.map(|(b, op)| (&b[y * width..(y + 1) * width], op));
                self.apply_row(y, 0, row, base_row);
            }
            return;
        }
        if self.opacity == 0 || (self.composite.clipped && base.is_none()) {
            return;
        }
        let PreparedFilter::GradientMap { ramp, dither, .. } = &self.kind else {
            return;
        };
        let width = usize::from(target.width());
        let height = usize::from(target.height());
        let pixels = target.data_as_u8_slice_mut().as_chunks_mut::<4>().0;
        let mut mapped_plane = vec![[0u8; 4]; width * height];
        dither::diffuse_plane(
            ramp,
            *dither,
            width,
            height,
            |x, y| {
                let p = pixels[y * width + x];
                if p[3] == 0 {
                    return None;
                }
                let straight = Rgba8::from_premultiplied(p);
                Some(dither::luma_q16([
                    straight.0[0],
                    straight.0[1],
                    straight.0[2],
                ]))
            },
            |x, y, mapped| {
                let idx = y * width + x;
                let ba = pixels[idx][3];
                mapped_plane[idx] = scaled_by_alpha(mapped, ba);
            },
        );
        for (idx, (pixel, &filtered)) in pixels.iter_mut().zip(&mapped_plane).enumerate() {
            if pixel[3] == 0 {
                continue;
            }
            self.composite_pixel(pixel, filtered, base, idx);
        }
    }

    #[inline]
    fn composite_pixel(
        &self,
        target: &mut Pixel,
        filtered: Pixel,
        base: Option<(&[Pixel], u8)>,
        index: usize,
    ) {
        let w = if let Some((base_pixels, base_opacity)) = base {
            let cover = mul(base_pixels[index][3], base_opacity);
            if cover == 0 {
                return;
            }
            mul(self.opacity, cover)
        } else {
            self.opacity
        };
        if self.composite.blend == Blend::Normal {
            for c in 0..4 {
                target[c] = mix_u8(target[c], filtered[c], w);
            }
        } else {
            let src = if w == 255 {
                filtered
            } else {
                filtered.map(|c| mul(c, w))
            };
            blend(self.composite.blend, target, src);
        }
    }

    /// Generates a standalone `size[0] × size[1]` thumbnail preview for UI layer rows.
    pub fn thumbnail(layer: &FilterLayer, size: [u32; 2]) -> Pixmap {
        let width = size[0].clamp(1, 256) as u16;
        let height = size[1].clamp(1, 256) as u16;
        let mut pixmap = Pixmap::new(width, height);
        let max_x = u32::from(width.saturating_sub(1)).max(1);
        let pixels = pixmap.data_as_u8_slice_mut().as_chunks_mut::<4>().0;
        for y in 0..usize::from(height) {
            for x in 0..usize::from(width) {
                let luma = ((x as u32 * 255) / max_x) as u8;
                pixels[y * usize::from(width) + x] = [luma, luma, luma, 255];
            }
        }
        let pass = Self::from_layer(layer, 1);
        pass.apply_plane(&mut pixmap, None);
        pixmap
    }
}
