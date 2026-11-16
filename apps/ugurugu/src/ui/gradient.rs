// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 NewfolderGames

//! Reusable multi-stop gradient, dithering and quantization editor shared by
//! the canvas Gradient tool and Gradient Map filter layers.

use egui::{Color32, CornerRadius, Mesh, Pos2, Rect, Sense, Stroke, Ui, epaint::Vertex};
use ugu_core::dither::Dither;
use ugu_core::gradient::{self, Gradient, GradientShape, GradientStop, PRESETS, QuantizeMode};
use ugu_core::ops::Rgba8;

use crate::i18n::tr;
use crate::icons::Glyph;
use crate::settings::SavedGradient;
use crate::theme;
use crate::widgets;

/// Per-instance transient state for [`editor`].
#[derive(Default)]
pub struct GradientEditorState {
    pub selected: usize,
    dragging: Option<usize>,
    saving_name: Option<String>,
    stop_picking: bool,
    stop_dirty: bool,
}

/// What an interactive [`editor`] frame produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GradientEditResult {
    /// Any value changed this frame (update live preview).
    pub changed: bool,
    /// A drag ended or discrete control changed (commit one undo step).
    pub committed: bool,
}

/// Optional controls shown only when editing [`Tool::Gradient`] settings.
pub struct ToolGradientControls<'a> {
    pub shape: &'a mut GradientShape,
    pub opacity: &'a mut f32,
    pub use_pen_color: &'a mut bool,
    pub pen_color: Rgba8,
}

/// Localized display name for a [`Dither`] mode.
pub fn dither_name(dither: Dither) -> &'static str {
    tr(match dither {
        Dither::None => "dither-none",
        Dither::Constant => "dither-constant",
        Dither::Checker => "dither-checker",
        Dither::HorizontalLines => "dither-horizontal",
        Dither::VerticalLines => "dither-vertical",
        Dither::Diagonal => "dither-diagonal",
        Dither::Crosshatch => "dither-crosshatch",
        Dither::Halftone => "dither-halftone",
        Dither::Bayer2 => "dither-bayer2",
        Dither::Bayer4 => "dither-bayer4",
        Dither::Bayer8 => "dither-bayer8",
        Dither::FloydSteinberg => "dither-floyd-steinberg",
        Dither::Atkinson => "dither-atkinson",
    })
}

fn preset_name(id: &str) -> &'static str {
    tr(match id {
        "black-to-white" => "gradient-preset-bw",
        "foreground-to-transparent" => "gradient-preset-fg-transparent",
        "sunset" => "gradient-preset-sunset",
        "ocean" => "gradient-preset-ocean",
        "heat" => "gradient-preset-heat",
        "sepia" => "gradient-preset-sepia",
        "gameboy-4" => "gradient-preset-gameboy",
        _ => "gradient-custom",
    })
}

fn active_preset_label<'a>(gradient: &Gradient, saved: &'a [SavedGradient]) -> &'a str {
    if let Some(preset) = PRESETS.iter().find(|p| p.stops == &*gradient.stops) {
        return preset_name(preset.id);
    }
    if let Some(user) = saved.iter().find(|s| &s.gradient == gradient) {
        return &user.name;
    }
    tr("gradient-custom")
}

/// Renders the multi-stop gradient, preset, dither and quantization editor.
#[allow(clippy::too_many_arguments)]
pub fn editor(
    ui: &mut Ui,
    id_salt: &str,
    state: &mut GradientEditorState,
    gradient: &mut Gradient,
    dither: &mut Dither,
    quantize: &mut QuantizeMode,
    saved: &mut Vec<SavedGradient>,
    saved_changed: &mut bool,
    mut tool: Option<ToolGradientControls<'_>>,
) -> GradientEditResult {
    let mut res = GradientEditResult::default();
    if gradient.stops.is_empty() {
        *gradient = Gradient::default();
    }
    state.selected = state.selected.min(gradient.stops.len() - 1);

    if let Some(tool) = &mut tool {
        widgets::segmented(
            ui,
            tool.shape,
            &[
                (GradientShape::Linear, tr("gradient-shape-linear")),
                (GradientShape::Radial, tr("gradient-shape-radial")),
            ],
            2,
        );
        ui.add_space(theme::space::XS);
    }

    presets_row(ui, id_salt, state, gradient, saved, saved_changed, &mut res);
    ui.add_space(theme::space::XS);

    let effective_stop0 = tool
        .as_ref()
        .filter(|t| *t.use_pen_color)
        .map(|t| t.pen_color);
    ramp_bar(ui, id_salt, state, gradient, effective_stop0, &mut res);
    ui.add_space(theme::space::XS);

    stop_controls(ui, id_salt, state, gradient, effective_stop0, &mut res);

    if let Some(tool) = tool {
        let before_use = *tool.use_pen_color;
        widgets::checkbox(ui, tool.use_pen_color, tr("gradient-use-pen-color"));
        if *tool.use_pen_color != before_use {
            res.changed = true;
            res.committed = true;
        }
        let mut pct = *tool.opacity * 100.0;
        let label = tr("opacity");
        let row = widgets::form_row(ui, label, |ui| {
            let field = widgets::number(ui, &mut pct, 1.0..=100.0, "%", 0, 56.0, label);
            let width = ui.available_width().max(40.0);
            widgets::slider(ui, &mut pct, 1.0..=100.0, label, width).union(field)
        });
        let next = (pct.round() / 100.0).clamp(0.01, 1.0);
        if (next - *tool.opacity).abs() > 1e-4 {
            *tool.opacity = next;
            res.changed = true;
        }
        if row.drag_stopped() || (row.changed() && !row.dragged()) {
            res.committed = true;
        }
    }

    ui.add_space(theme::space::XS);
    dither_and_quantize(ui, id_salt, dither, quantize, &mut res);
    res
}

fn presets_row(
    ui: &mut Ui,
    id_salt: &str,
    state: &mut GradientEditorState,
    gradient: &mut Gradient,
    saved: &mut Vec<SavedGradient>,
    saved_changed: &mut bool,
    res: &mut GradientEditResult,
) {
    let selected_label = active_preset_label(gradient, saved).to_owned();
    let user_idx = saved.iter().position(|s| &s.gradient == gradient);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let combo_w = (ui.available_width() - 80.0).max(72.0);
        let mut chosen_preset: Option<Gradient> = None;
        egui::ComboBox::from_id_salt((id_salt, "preset"))
            .width(combo_w)
            .selected_text(&selected_label)
            .show_ui(ui, |ui| {
                for preset in PRESETS {
                    let mut picked = preset.stops == &*gradient.stops;
                    if ui
                        .selectable_label(picked, preset_name(preset.id))
                        .clicked()
                    {
                        picked = true;
                    }
                    if picked && preset.stops != &*gradient.stops {
                        chosen_preset = Some(Gradient::from_preset(&preset));
                    }
                }
                if !saved.is_empty() {
                    ui.separator();
                    for item in saved.iter() {
                        let picked = &item.gradient == gradient;
                        if ui.selectable_label(picked, &item.name).clicked() && !picked {
                            chosen_preset = Some(item.gradient.clone());
                        }
                    }
                }
            })
            .response
            .widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, tr("gradient-presets"))
            });
        if let Some(next) = chosen_preset {
            *gradient = next;
            state.selected = 0;
            state.stop_picking = false;
            res.changed = true;
            res.committed = true;
        }

        if widgets::icon_button(
            ui,
            Glyph::MirrorHorizontal,
            14.0,
            tr("gradient-reverse"),
            true,
        )
        .clicked()
        {
            let count = gradient.stops.len();
            *gradient = gradient.reversed();
            state.selected = count.saturating_sub(1).saturating_sub(state.selected);
            res.changed = true;
            res.committed = true;
        }

        if widgets::icon_button(ui, Glyph::Add, 14.0, tr("gradient-save-preset"), true).clicked() {
            state.saving_name = Some(match user_idx {
                Some(idx) => saved[idx].name.clone(),
                None => tr("gradient-default-name").to_owned(),
            });
        }

        if let Some(idx) = user_idx
            && widgets::icon_button(ui, Glyph::Delete, 14.0, tr("gradient-delete-preset"), true)
                .clicked()
        {
            saved.remove(idx);
            *saved_changed = true;
        }
    });

    if let Some(name) = state.saving_name.as_mut() {
        let mut action = 0i8;
        ui.horizontal(|ui| {
            let field_w = (ui.available_width() - 56.0).max(60.0);
            let field = ui.add(
                egui::TextEdit::singleline(name)
                    .id_salt((id_salt, "save-name"))
                    .desired_width(field_w)
                    .char_limit(32),
            );
            field.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::TextEdit,
                    true,
                    tr("gradient-save-name"),
                )
            });
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                action = 1;
            }
            if widgets::icon_button(
                ui,
                Glyph::Confirm,
                14.0,
                tr("confirm"),
                !name.trim().is_empty(),
            )
            .clicked()
            {
                action = 1;
            }
            if widgets::icon_button(ui, Glyph::Cancel, 14.0, tr("cancel"), true).clicked() {
                action = -1;
            }
        });
        if action == 1 {
            if let Some(raw) = state.saving_name.take() {
                let clean = raw.trim().to_owned();
                if !clean.is_empty() {
                    if let Some(existing) = saved.iter_mut().find(|s| s.name == clean) {
                        existing.gradient = gradient.clone();
                    } else {
                        saved.push(SavedGradient {
                            name: clean,
                            gradient: gradient.clone(),
                        });
                    }
                    *saved_changed = true;
                }
            }
        } else if action == -1 {
            state.saving_name = None;
        }
    }
}

fn ramp_bar(
    ui: &mut Ui,
    id_salt: &str,
    state: &mut GradientEditorState,
    gradient: &mut Gradient,
    stop0_rgb: Option<Rgba8>,
    res: &mut GradientEditResult,
) {
    let width = ui.available_width().max(80.0);
    let total_height = 42.0;
    let bar_height = 24.0;
    let pad = 6.0;
    let (outer, response) =
        ui.allocate_exact_size(egui::vec2(width, total_height), Sense::click_and_drag());
    let response = response.on_hover_text(tr("gradient-ramp-tip"));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, tr("gradient-ramp"))
    });

    let bar = Rect::from_min_max(
        egui::pos2(outer.left() + pad, outer.top() + 2.0),
        egui::pos2(outer.right() - pad, outer.top() + 2.0 + bar_height),
    );
    let bar_w = bar.width().max(1.0);

    // Checkerboard under the gradient bar so stop alpha is visible.
    let painter = ui.painter();
    let cell = 6.0;
    let cols = (bar_w / cell).ceil() as usize;
    let rows = (bar.height() / cell).ceil() as usize;
    for r in 0..rows {
        for c in 0..cols {
            let x0 = bar.left() + c as f32 * cell;
            let y0 = bar.top() + r as f32 * cell;
            let sq = Rect::from_min_max(
                egui::pos2(x0, y0),
                egui::pos2((x0 + cell).min(bar.right()), (y0 + cell).min(bar.bottom())),
            );
            let tint = if (r + c).is_multiple_of(2) {
                Color32::from_gray(205)
            } else {
                Color32::from_gray(145)
            };
            painter.rect_filled(sq, 0.0, tint);
        }
    }

    // Preview gradient using a horizontal triangle strip.
    let preview = if let Some(rgb) = stop0_rgb {
        let mut s = gradient.stops.to_vec();
        s[0].color = Rgba8([rgb.0[0], rgb.0[1], rgb.0[2], s[0].color.0[3]]);
        Gradient::new(s)
    } else {
        gradient.clone()
    };
    let steps = (bar_w as usize / 2).clamp(16, 160);
    let mut mesh = Mesh::default();
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = bar.left() + bar_w * t;
        let Rgba8([r, g, b, a]) = preview.sample(t);
        let col = Color32::from_rgba_unmultiplied(r, g, b, a);
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push(Vertex {
            pos: Pos2::new(x, bar.top()),
            uv: Pos2::ZERO,
            color: col,
        });
        mesh.vertices.push(Vertex {
            pos: Pos2::new(x, bar.bottom()),
            uv: Pos2::ZERO,
            color: col,
        });
        if i > 0 {
            mesh.indices
                .extend_from_slice(&[base - 2, base - 1, base, base - 1, base + 1, base]);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.rect_stroke(
        bar,
        CornerRadius::same(theme::radius::SMALL),
        Stroke::new(1.0, theme::border()),
        egui::StrokeKind::Inside,
    );

    // Draw stop handles along the bottom edge.
    let mut handle_rects = Vec::with_capacity(gradient.stops.len());
    for (idx, stop) in gradient.stops.iter().enumerate() {
        let hx = bar.left() + bar_w * stop.position;
        let hy = bar.bottom() + 9.0;
        let selected = idx == state.selected;
        let Rgba8([r, g, b, _]) = if idx == 0 {
            stop0_rgb.unwrap_or(stop.color)
        } else {
            stop.color
        };
        let fill = Color32::from_rgb(r, g, b);
        let radius = if selected { 6.0 } else { 4.5 };
        let pts = vec![
            Pos2::new(hx, bar.bottom() + 1.0),
            Pos2::new(hx + radius, hy),
            Pos2::new(hx, hy + radius),
            Pos2::new(hx - radius, hy),
        ];
        painter.add(egui::Shape::convex_polygon(
            pts,
            fill,
            Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected {
                    theme::accent()
                } else {
                    theme::border()
                },
            ),
        ));
        handle_rects.push(Rect::from_center_size(
            Pos2::new(hx, hy),
            egui::vec2(14.0, 18.0),
        ));
    }

    if response.has_focus() {
        widgets::focus_ring(painter, outer, theme::radius::SMALL);
    }

    // Handle pointer press/drag/insert.
    if (response.drag_started() || (response.clicked() && state.dragging.is_none()))
        && let Some(pos) = response.interact_pointer_pos()
    {
        if let Some(hit) = handle_rects.iter().position(|r| r.contains(pos)) {
            state.selected = hit;
            state.dragging = response.drag_started().then_some(hit);
            response.request_focus();
        } else if response.clicked()
            && bar.expand2(egui::vec2(pad, 6.0)).contains(pos)
            && gradient.stops.len() < *gradient::limits::STOPS.end()
        {
            let t = ((pos.x - bar.left()) / bar_w).clamp(0.0, 1.0);
            let color = gradient.sample(t);
            let mut stops = gradient.stops.to_vec();
            stops.push(GradientStop { position: t, color });
            let last = stops.len() - 1;
            let new_idx = sort_stops_preserving(&mut stops, last);
            *gradient = Gradient::new(stops);
            state.selected = new_idx;
            response.request_focus();
            res.changed = true;
            res.committed = true;
        }
    }

    if response.dragged()
        && let Some(idx) = state.dragging
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        let t = ((pos.x - bar.left()) / bar_w).clamp(0.0, 1.0);
        let mut stops = gradient.stops.to_vec();
        if idx < stops.len() && (stops[idx].position - t).abs() > 1e-4 {
            stops[idx].position = t;
            let new_idx = sort_stops_preserving(&mut stops, idx);
            *gradient = Gradient::new(stops);
            state.selected = new_idx;
            state.dragging = Some(new_idx);
            res.changed = true;
        }
    }

    if response.drag_stopped() && state.dragging.take().is_some() {
        res.changed = true;
        res.committed = true;
    }

    // Keyboard stop nudge & delete when focused.
    let _ = id_salt;
    if response.has_focus() {
        let step = if ui.input(|i| i.modifiers.shift) {
            0.05
        } else {
            0.01
        };
        if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            nudge_stop(state, gradient, -step, res);
        } else if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            nudge_stop(state, gradient, step, res);
        } else if (ui.input(|i| i.key_pressed(egui::Key::Delete))
            || ui.input(|i| i.key_pressed(egui::Key::Backspace)))
            && gradient.stops.len() > *gradient::limits::STOPS.start()
        {
            remove_selected_stop(state, gradient, res);
        }
    }
}

fn sort_stops_preserving(stops: &mut [GradientStop], tracked: usize) -> usize {
    let mut tagged: Vec<(bool, GradientStop)> = stops
        .iter()
        .copied()
        .enumerate()
        .map(|(i, s)| (i == tracked, s))
        .collect();
    tagged.sort_by(|a, b| {
        a.1.position
            .partial_cmp(&b.1.position)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (dst, (_, src)) in stops.iter_mut().zip(&tagged) {
        *dst = *src;
    }
    tagged.iter().position(|(marked, _)| *marked).unwrap_or(0)
}

fn nudge_stop(
    state: &mut GradientEditorState,
    gradient: &mut Gradient,
    delta: f32,
    res: &mut GradientEditResult,
) {
    let mut stops = gradient.stops.to_vec();
    let idx = state.selected.min(stops.len() - 1);
    let next = (stops[idx].position + delta).clamp(0.0, 1.0);
    if (next - stops[idx].position).abs() > 1e-4 {
        stops[idx].position = next;
        let moved = sort_stops_preserving(&mut stops, idx);
        *gradient = Gradient::new(stops);
        state.selected = moved;
        res.changed = true;
        res.committed = true;
    }
}

fn remove_selected_stop(
    state: &mut GradientEditorState,
    gradient: &mut Gradient,
    res: &mut GradientEditResult,
) {
    if gradient.stops.len() <= *gradient::limits::STOPS.start() {
        return;
    }
    let mut stops = gradient.stops.to_vec();
    stops.remove(state.selected);
    *gradient = Gradient::new(stops);
    state.selected = state.selected.min(gradient.stops.len() - 1);
    res.changed = true;
    res.committed = true;
}

fn stop_controls(
    ui: &mut Ui,
    id_salt: &str,
    state: &mut GradientEditorState,
    gradient: &mut Gradient,
    stop0_rgb: Option<Rgba8>,
    res: &mut GradientEditResult,
) {
    let idx = state.selected.min(gradient.stops.len() - 1);
    let stop = gradient.stops[idx];
    let can_remove = gradient.stops.len() > *gradient::limits::STOPS.start();
    let display_color = if idx == 0 {
        stop0_rgb.map_or(stop.color, |rgb| {
            Rgba8([rgb.0[0], rgb.0[1], rgb.0[2], stop.color.0[3]])
        })
    } else {
        stop.color
    };
    let [r, g, b, a] = display_color.0;
    let mut col = Color32::from_rgba_unmultiplied(r, g, b, a);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let swatch = widgets::color_swatch(ui, col, tr("gradient-stop-color"), state.stop_picking)
            .on_hover_text(tr("gradient-stop-color-tip"));
        if swatch.clicked() {
            state.stop_picking = !state.stop_picking;
        }
        let mut pct = stop.position * 100.0;
        let pos_name = tr("gradient-stop-position");
        let num = widgets::number(ui, &mut pct, 0.0..=100.0, "%", 0, 56.0, pos_name);
        let new_pos = (pct / 100.0).clamp(0.0, 1.0);
        if (new_pos - stop.position).abs() > 1e-4 {
            let mut stops = gradient.stops.to_vec();
            stops[idx].position = new_pos;
            let moved = sort_stops_preserving(&mut stops, idx);
            *gradient = Gradient::new(stops);
            state.selected = moved;
            res.changed = true;
        }
        if num.drag_stopped() || (num.changed() && !num.dragged()) {
            res.committed = true;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::icon_button(
                ui,
                Glyph::Remove,
                14.0,
                tr("gradient-stop-remove"),
                can_remove,
            )
            .clicked()
            {
                remove_selected_stop(state, gradient, res);
            }
        });
    });

    if state.stop_picking {
        let mut picker_col = col;
        let picked = egui::color_picker::color_picker_color32(
            ui,
            &mut picker_col,
            egui::color_picker::Alpha::OnlyBlend,
        );
        if picked {
            let next = Rgba8(picker_col.to_srgba_unmultiplied());
            let mut stops = gradient.stops.to_vec();
            stops[idx].color = if idx == 0 && stop0_rgb.is_some() {
                Rgba8([
                    stops[idx].color.0[0],
                    stops[idx].color.0[1],
                    stops[idx].color.0[2],
                    next.0[3],
                ])
            } else {
                next
            };
            *gradient = Gradient::new(stops);
            col = picker_col;
            let _ = col;
            state.stop_dirty = true;
            res.changed = true;
        }
    }
    if state.stop_dirty && !ui.input(|i| i.pointer.any_down()) {
        state.stop_dirty = false;
        res.committed = true;
    }
    let _ = id_salt;
}

fn dither_and_quantize(
    ui: &mut Ui,
    id_salt: &str,
    dither: &mut Dither,
    quantize: &mut QuantizeMode,
    res: &mut GradientEditResult,
) {
    let mut chosen_dither = *dither;
    widgets::form_row(ui, tr("dither-mode"), |ui| {
        egui::ComboBox::from_id_salt((id_salt, "dither"))
            .width(ui.available_width())
            .selected_text(dither_name(*dither))
            .show_ui(ui, |ui| {
                for mode in Dither::ALL {
                    widgets::choice(ui, &mut chosen_dither, mode, dither_name(mode));
                }
            })
            .response
            .widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, tr("dither-mode"))
            });
    });
    if chosen_dither != *dither {
        *dither = chosen_dither;
        res.changed = true;
        res.committed = true;
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum QKind {
        Overall,
        PerSegment,
    }
    let mut kind = match *quantize {
        QuantizeMode::Overall(_) => QKind::Overall,
        QuantizeMode::PerSegment(_) => QKind::PerSegment,
    };
    let prev_kind = kind;
    widgets::segmented(
        ui,
        &mut kind,
        &[
            (QKind::Overall, tr("quantize-overall")),
            (QKind::PerSegment, tr("quantize-segment")),
        ],
        2,
    );
    if kind != prev_kind {
        *quantize = match kind {
            QKind::Overall => QuantizeMode::Overall(match *quantize {
                QuantizeMode::PerSegment(n) => (n + 1).clamp(
                    *gradient::limits::OVERALL_LEVELS.start(),
                    *gradient::limits::OVERALL_LEVELS.end(),
                ),
                QuantizeMode::Overall(n) => n,
            }),
            QKind::PerSegment => QuantizeMode::PerSegment(match *quantize {
                QuantizeMode::Overall(n) => n.saturating_sub(1).clamp(
                    *gradient::limits::SEGMENT_LEVELS.start(),
                    *gradient::limits::SEGMENT_LEVELS.end(),
                ),
                QuantizeMode::PerSegment(n) => n,
            }),
        };
        res.changed = true;
        res.committed = true;
    }

    let (range, mut levels) = match *quantize {
        QuantizeMode::Overall(n) => (
            *gradient::limits::OVERALL_LEVELS.start() as f32
                ..=*gradient::limits::OVERALL_LEVELS.end() as f32,
            n as f32,
        ),
        QuantizeMode::PerSegment(n) => (
            *gradient::limits::SEGMENT_LEVELS.start() as f32
                ..=*gradient::limits::SEGMENT_LEVELS.end() as f32,
            n as f32,
        ),
    };
    let label = tr("quantize-levels");
    let row = widgets::form_row(ui, label, |ui| {
        let num = widgets::number(ui, &mut levels, range.clone(), "", 0, 56.0, label);
        let width = ui.available_width().max(40.0);
        widgets::slider(ui, &mut levels, range, label, width).union(num)
    });
    let rounded = levels.round() as u8;
    let next = match *quantize {
        QuantizeMode::Overall(_) => QuantizeMode::Overall(rounded),
        QuantizeMode::PerSegment(_) => QuantizeMode::PerSegment(rounded),
    }
    .clamped();
    if next != *quantize {
        *quantize = next;
        res.changed = true;
    }
    if row.drag_stopped() || (row.changed() && !row.dragged()) {
        res.committed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_editor_exposes_controls_and_reverses() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut state = GradientEditorState::default();
        let mut gradient = Gradient::from_preset(&PRESETS[2]);
        let mut dither = Dither::Bayer4;
        let mut quantize = QuantizeMode::Overall(4);
        let mut saved = Vec::new();
        let mut saved_changed = false;
        let mut shape = GradientShape::Linear;
        let mut opacity = 1.0;
        let mut use_pen = false;

        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = editor(
                ui,
                "test-grad",
                &mut state,
                &mut gradient,
                &mut dither,
                &mut quantize,
                &mut saved,
                &mut saved_changed,
                Some(ToolGradientControls {
                    shape: &mut shape,
                    opacity: &mut opacity,
                    use_pen_color: &mut use_pen,
                    pen_color: Rgba8([10, 20, 30, 255]),
                }),
            );
        });
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.expect("accesskit");
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some(tr("gradient-ramp")))
        );
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some(tr("dither-mode")))
        );

        let original = gradient.clone();
        let mut res = GradientEditResult::default();
        nudge_stop(&mut state, &mut gradient, 0.25, &mut res);
        assert!(res.changed && res.committed);
        assert_ne!(gradient, original);
    }
}
