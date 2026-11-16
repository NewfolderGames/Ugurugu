// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 NewfolderGames

//! Interactive canvas gradient tool (`Tool::Gradient`) and its live drag state.

use std::sync::Arc;

use ugu_core::command;
use ugu_core::dither::Dither;
use ugu_core::document::LayerId;
use ugu_core::edit::{EditError, Outcome};
use ugu_core::gradient::{Gradient, GradientShape, QuantizeMode};
use ugu_core::ops::{Affine, Rgba8};
use ugu_core::selection::Selection;
use ugu_core::store;

use crate::{Session, StrokeRefused, Tools, clamp_position, into, selecting};

/// Persistent tool settings for drawing gradients onto paint layers.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientSettings {
    pub gradient: Gradient,
    pub shape: GradientShape,
    pub dither: Dither,
    pub quantize: QuantizeMode,
    pub opacity: f32,
    pub use_pen_color: bool,
}

impl Default for GradientSettings {
    fn default() -> Self {
        Self {
            gradient: Gradient::default(),
            shape: GradientShape::Linear,
            dither: Dither::Bayer4,
            quantize: QuantizeMode::Overall(4),
            opacity: 1.0,
            use_pen_color: false,
        }
    }
}

impl GradientSettings {
    /// Returns the active gradient, replacing stop 0 RGB with `pen_color` RGB when
    /// `use_pen_color` is enabled.
    pub fn effective_gradient(&self, pen_color: Rgba8) -> Gradient {
        if !self.use_pen_color || self.gradient.stops.is_empty() {
            return self.gradient.clone();
        }
        let mut stops = self.gradient.stops.to_vec();
        stops[0].color = Rgba8([
            pen_color.0[0],
            pen_color.0[1],
            pen_color.0[2],
            stops[0].color.0[3],
        ]);
        Gradient::new(stops)
    }
}

/// Active canvas gradient drag in document pixels, transformed into layer space on commit.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveGradient {
    pub layer: LayerId,
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub gradient: Gradient,
    pub shape: GradientShape,
    pub dither: Dither,
    pub quantize: QuantizeMode,
    pub opacity: f32,
    pub clip: Option<Arc<Selection>>,
    pub to_layer: Option<Affine>,
}

impl LiveGradient {
    /// Endpoints in the target paint layer's own pixel coordinates.
    pub fn layer_endpoints(&self) -> ([f32; 2], [f32; 2]) {
        let map = |pt: [f64; 2]| {
            let p = clamp_position(into(self.to_layer, pt));
            [p[0] as f32, p[1] as f32]
        };
        (map(self.from), map(self.to))
    }
}

fn snap_angle(from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let len = dx.hypot(dy);
    if len <= 0.0 {
        return to;
    }
    let step = std::f64::consts::PI / 12.0; // 15 degrees
    let angle = (dy.atan2(dx) / step).round() * step;
    let (sin, cos) = angle.sin_cos();
    [from[0] + len * cos, from[1] + len * sin]
}

impl Session {
    pub fn live_gradient(&self) -> Option<&LiveGradient> {
        self.live_gradient.as_ref()
    }

    pub fn begin_gradient(
        &mut self,
        tools: &Tools,
        position: [f64; 2],
    ) -> Result<(), StrokeRefused> {
        self.drop_live();
        if self.folder() != Some(self.layer) {
            self.can_paint()?;
        }
        self.settle().map_err(|_| StrokeRefused::Pending)?;
        self.draw_on_cel().map_err(|_| StrokeRefused::NoLayer)?;
        let clip = self.stroke_clip();
        if clip.is_err() {
            self.drop_cel();
        }
        let clip = clip?;
        let to_layer = self
            .layer_transform()
            .and_then(|transform| transform.inverse());
        let gradient = tools.gradient.effective_gradient(tools.color);
        let limit = f64::from(store::limits::COORDINATE);
        let clamped = position.map(|v| v.clamp(-limit, limit));
        self.live_gradient = Some(LiveGradient {
            layer: self.layer,
            from: clamped,
            to: clamped,
            gradient,
            shape: tools.gradient.shape,
            dither: tools.gradient.dither,
            quantize: tools.gradient.quantize,
            opacity: tools.gradient.opacity.clamp(0.0, 1.0),
            clip,
            to_layer,
        });
        Ok(())
    }

    /// Updates the drag endpoint, optionally snapping to 15-degree increments.
    pub fn drag_gradient(&mut self, position: [f64; 2], snap: bool) -> bool {
        let Some(live) = self.live_gradient.as_mut() else {
            return false;
        };
        let limit = f64::from(store::limits::COORDINATE);
        let clamped = position.map(|v| v.clamp(-limit, limit));
        let next = if snap {
            snap_angle(live.from, clamped)
        } else {
            clamped
        };
        let changed = live.to != next;
        live.to = next;
        changed
    }

    pub fn end_gradient(&mut self, position: [f64; 2], snap: bool) -> Result<Outcome, EditError> {
        self.drag_gradient(position, snap);
        let Some(live) = self.live_gradient.take() else {
            return Ok(Outcome::NoChange);
        };
        if live.from == live.to {
            self.drop_cel();
            return Ok(Outcome::NoChange);
        }
        let (from, to) = live.layer_endpoints();
        self.with_cel(|session| {
            session.history_mut()?.group("Gradient", |group| {
                let clip = match &live.clip {
                    Some(selection) => Some(selecting::stored_mask(group, selection)?),
                    None => None,
                };
                group.apply(|document| {
                    command::gradient(
                        document,
                        live.layer,
                        live.gradient.clone(),
                        live.shape,
                        from,
                        to,
                        live.dither,
                        live.quantize,
                        live.opacity,
                        clip,
                    )
                })
            })
        })
    }

    pub fn cancel_gradient(&mut self) -> bool {
        if self.live_gradient.take().is_some() {
            self.drop_cel();
            return true;
        }
        false
    }
}
