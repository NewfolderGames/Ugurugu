// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 NewfolderGames

//! Gradient operation (`Op::Gradient`) rasterizer shared between full layer
//! draws and incremental `Split` editing stamps.

use ugu_core::dither::{self, Dither};
use ugu_core::gradient::{self, Gradient, GradientShape, QuantizeMode, QuantizedRamp};
use ugu_core::ops::Rgba8;
use ugu_core::store::Mask;
use vello_cpu::Pixmap;

use crate::compose::{inside, premultiplied, src_over};
use crate::raster::PixelRect;
use crate::tile::TiledSurface;

#[inline]
fn div255(value: u32) -> u8 {
    ((value + 127) / 255) as u8
}

#[derive(Clone, Debug)]
pub struct GradientDraw {
    pub gradient: Gradient,
    pub shape: GradientShape,
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub dither: Dither,
    pub quantize: QuantizeMode,
    pub opacity: f32,
}

impl GradientDraw {
    /// Bounds of pixels affected in canvas space (`canvas_size`).
    pub fn bounds(&self, clip: Option<&Mask>, canvas_size: [u32; 2]) -> Option<PixelRect> {
        let [w, h] = canvas_size.map(|v| v as i32);
        let [l, t, r, b] = match clip {
            Some(mask) => {
                let [left, top, mw, mh] = mask.bounds;
                [
                    left.max(0),
                    top.max(0),
                    (left + mw).min(w),
                    (top + mh).min(h),
                ]
            }
            None => [0, 0, w, h],
        };
        if l >= r || t >= b {
            None
        } else {
            Some([l as u32, t as u32, r as u32, b as u32])
        }
    }

    /// Renders this gradient into a transparent premultiplied `Pixmap` covering
    /// `rect` in canvas coordinates (or scaled by `shrink` when `shrink > 1`).
    pub fn render_rect(&self, rect: PixelRect, clip: Option<&Mask>, shrink: u32) -> Pixmap {
        let shrink = shrink.max(1);
        let [left, top, right, bottom] = rect;
        let out_w = (right - left).div_ceil(shrink).max(1) as u16;
        let out_h = (bottom - top).div_ceil(shrink).max(1) as u16;
        let mut pixmap = Pixmap::new(out_w, out_h);
        let width = usize::from(out_w);
        let height = usize::from(out_h);
        let ramp = QuantizedRamp::new(&self.gradient, self.quantize);
        let opacity_u32 = u32::from((self.opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
        if opacity_u32 == 0 {
            return pixmap;
        }
        let scale = f64::from(shrink);
        let from_x = f64::from(self.from[0]);
        let from_y = f64::from(self.from[1]);
        let dx = f64::from(self.to[0] - self.from[0]);
        let dy = f64::from(self.to[1] - self.from[1]);
        let len2 = dx * dx + dy * dy;
        let len = len2.sqrt();

        let t_at = |px_idx: usize, py_idx: usize| -> Option<(i32, i32, u32)> {
            let doc_x = left as i32 + (px_idx as i32) * (shrink as i32);
            let doc_y = top as i32 + (py_idx as i32) * (shrink as i32);
            if !inside(clip, doc_x as usize, doc_y as usize) {
                return None;
            }
            let cx = f64::from(doc_x) + 0.5 * scale;
            let cy = f64::from(doc_y) + 0.5 * scale;
            let t = match self.shape {
                GradientShape::Linear => {
                    if len2 <= 0.0 {
                        1.0
                    } else {
                        ((cx - from_x) * dx + (cy - from_y) * dy) / len2
                    }
                }
                GradientShape::Radial => {
                    if len <= 0.0 {
                        1.0
                    } else {
                        (cx - from_x).hypot(cy - from_y) / len
                    }
                }
            };
            Some((doc_x, doc_y, gradient::to_q16(t as f32)))
        };

        let encode = |sample: Rgba8| -> [u8; 4] {
            let alpha = if opacity_u32 == 255 {
                sample.0[3]
            } else {
                div255(u32::from(sample.0[3]) * opacity_u32)
            };
            premultiplied([sample.0[0], sample.0[1], sample.0[2], alpha])
        };

        let pixels = pixmap.data_as_u8_slice_mut().as_chunks_mut::<4>().0;
        if self.dither.is_error_diffusion() {
            dither::diffuse_plane(
                &ramp,
                self.dither,
                width,
                height,
                |x, y| t_at(x, y).map(|(_, _, t)| t),
                |x, y, sample| {
                    pixels[y * width + x] = encode(sample);
                },
            );
        } else {
            for y in 0..height {
                for x in 0..width {
                    let Some((doc_x, doc_y, t_q16)) = t_at(x, y) else {
                        continue;
                    };
                    let gx = if shrink == 1 { doc_x } else { x as i32 };
                    let gy = if shrink == 1 { doc_y } else { y as i32 };
                    let sample = dither::sample_positioned(
                        &self.gradient,
                        &ramp,
                        self.dither,
                        t_q16,
                        gx,
                        gy,
                    );
                    pixels[y * width + x] = encode(sample);
                }
            }
        }
        pixmap
    }
}

/// Stamps `draw` `src_over` onto `surface` at `shrink == 1`. Returns the changed rect.
pub fn stamp_gradient(
    surface: &mut TiledSurface,
    draw: &GradientDraw,
    clip: Option<&Mask>,
) -> Option<PixelRect> {
    let rect = draw.bounds(clip, surface.size())?;
    let pixmap = draw.render_rect(rect, clip, 1);
    let [left, top, right, _] = rect;
    let width = (right - left) as usize;
    let src_pixels = pixmap.data_as_u8_slice().as_chunks::<4>().0;
    surface.ensure(rect);
    for (y, line) in surface.rows_mut(rect) {
        let src_row = (y - top) as usize * width;
        for (x, target) in line.iter_mut().enumerate() {
            let source = src_pixels[src_row + x];
            if source[3] != 0 {
                src_over(target, &source);
            }
        }
    }
    Some(rect)
}
