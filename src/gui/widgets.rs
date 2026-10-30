// OpenMemoryOptimizer - a simple memory optimizer for Windows
// Copyright (C) 2026 Lucian Caetano
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Custom-painted widgets: toggle switch, gradient ring gauge, cards and buttons.

use eframe::egui;
use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Frame, Margin, Pos2, Rect, Response,
    Sense, Shadow, Stroke, Ui, Vec2,
};

use crate::theme::{self, *};

pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(42.0, 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.1);
    let painter = ui.painter();
    let track = lerp_color(TRACK, ACCENT, t);
    let track = lerp_color(track, Color32::WHITE, hover * 0.06);
    painter.rect_filled(rect, CornerRadius::same(12), track);
    let r = rect.height() / 2.0;
    let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
    painter.circle_filled(
        pos2(x, rect.center().y + 1.0),
        r - 3.0,
        Color32::from_black_alpha(60),
    );
    painter.circle_filled(pos2(x, rect.center().y), r - 3.5, Color32::WHITE);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Circular gauge drawn as a gradient arc with a soft glow and round caps.
pub fn ring(ui: &Ui, center: Pos2, radius: f32, width: f32, frac: f32, c0: Color32, c1: Color32) {
    let painter = ui.painter();
    painter.circle_stroke(center, radius, Stroke::new(width, TRACK));
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.001 {
        return;
    }
    let start = -FRAC_PI_2;
    let sweep = frac * TAU;
    let n = ((frac * 160.0).ceil() as usize).max(2);
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + sweep * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    let color_at = |i: usize| lerp_color(c0, c1, i as f32 / n as f32);
    // glow
    for i in 0..n {
        painter.line_segment(
            [pts[i], pts[i + 1]],
            Stroke::new(width + 10.0, color_at(i).gamma_multiply(0.07)),
        );
    }
    for i in 0..n {
        painter.line_segment([pts[i], pts[i + 1]], Stroke::new(width, color_at(i)));
    }
    painter.circle_filled(pts[0], width / 2.0, c0);
    painter.circle_filled(pts[n], width / 2.0, color_at(n));
}

pub fn card_frame() -> Frame {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(18))
}

/// A card occupying exactly `size` (outer), with contents laid out top-down.
pub fn card_sized<R>(ui: &mut Ui, size: Vec2, add: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = card_frame();
    let margin = frame.total_margin().sum();
    ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
        frame
            .show(ui, |ui| {
                let inner = size - margin;
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                add(ui)
            })
            .inner
    })
    .inner
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_frame()
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn label(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) -> Response {
    ui.label(egui::RichText::new(text).font(font).color(color))
}

pub fn card_title(ui: &mut Ui, title: &str, subtitle: Option<&str>) {
    label(ui, title, semibold(15.0), TEXT);
    if let Some(s) = subtitle {
        ui.add_space(-6.0);
        label(ui, s, regular(12.5), MUTED);
    }
}

/// Filled pill button with glow; optional icon glyph and spinner.
pub fn primary_button(
    ui: &mut Ui,
    size: Vec2,
    text: &str,
    icon: Option<&str>,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), enabled && resp.hovered(), 0.12);
    let press = enabled && resp.is_pointer_button_down_on();
    let rect = if press { rect.shrink(1.0) } else { rect };
    let radius = CornerRadius::same((size.y / 2.0).min(255.0) as u8);
    let painter = ui.painter();
    if enabled {
        let glow = Shadow {
            offset: [0, 6],
            blur: 22,
            spread: 0,
            color: ACCENT.gamma_multiply(0.30 + 0.25 * hover),
        };
        painter.add(glow.as_shape(rect, radius));
    }
    let fill = if enabled {
        lerp_color(ACCENT, ACCENT_HI, hover)
    } else {
        CARD_HI
    };
    painter.rect_filled(rect, radius, fill);
    let fg = if enabled { Color32::WHITE } else { MUTED };
    let label = painter.layout_no_wrap(text.to_owned(), semibold(15.0), fg);
    let icon_g = icon.map(|i| painter.layout_no_wrap(i.to_owned(), theme::icon(15.0), fg));
    let gap = 9.0;
    let total = label.size().x + icon_g.as_ref().map_or(0.0, |g| g.size().x + gap);
    let mut x = rect.center().x - total / 2.0;
    if let Some(g) = icon_g {
        let y = rect.center().y - g.size().y / 2.0;
        let w = g.size().x;
        painter.galley(pos2(x, y), g, fg);
        x += w + gap;
    }
    painter.galley(pos2(x, rect.center().y - label.size().y / 2.0), label, fg);
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Outlined secondary button.
pub fn ghost_button(ui: &mut Ui, text: &str, icon: Option<&str>) -> Response {
    let font = regular(14.0);
    let painter = ui.painter();
    let label = painter.layout_no_wrap(text.to_owned(), font, TEXT);
    let icon_g = icon.map(|i| painter.layout_no_wrap(i.to_owned(), theme::icon(13.0), TEXT));
    let gap = 8.0;
    let content = label.size().x + icon_g.as_ref().map_or(0.0, |g| g.size().x + gap);
    let size = vec2(content + 32.0, 38.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(19),
        lerp_color(CARD, CARD_HI, hover),
        Stroke::new(1.0, lerp_color(STROKE, MUTED, hover * 0.6)),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.center().x - content / 2.0;
    if let Some(g) = icon_g {
        let w = g.size().x;
        painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, TEXT);
        x += w + gap;
    }
    painter.galley(pos2(x, rect.center().y - label.size().y / 2.0), label, TEXT);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Sidebar navigation item.
pub fn nav_item(ui: &mut Ui, icon: &str, text: &str, selected: bool) -> Response {
    let size = vec2(ui.available_width(), 40.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.1);
    let sel = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("s"), selected, 0.15);
    let painter = ui.painter();
    let bg = lerp_color(
        lerp_color(SIDEBAR, CARD_HI, hover * 0.8),
        ACCENT.gamma_multiply(0.22),
        sel,
    );
    painter.rect_filled(rect, CornerRadius::same(10), bg);
    if sel > 0.0 {
        let bar = Rect::from_center_size(
            pos2(rect.left() + 3.0, rect.center().y),
            vec2(3.0, 18.0 * sel),
        );
        painter.rect_filled(bar, CornerRadius::same(2), lerp_color(CYAN, VIOLET, 0.5));
    }
    let fg = lerp_color(MUTED, Color32::WHITE, sel.max(hover * 0.6));
    painter.text(
        pos2(rect.left() + 18.0, rect.center().y),
        Align2::LEFT_CENTER,
        icon,
        theme::icon(16.0),
        lerp_color(fg, CYAN, sel * 0.7),
    );
    painter.text(
        pos2(rect.left() + 46.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        if selected {
            semibold(14.0)
        } else {
            regular(14.0)
        },
        fg,
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Small square title-bar button (minimize / close).
pub fn titlebar_button(ui: &mut Ui, icon: &str, danger: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 34.0), Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.08);
    let painter = ui.painter();
    let hover_bg = if danger {
        Color32::from_rgb(196, 43, 28)
    } else {
        CARD_HI
    };
    painter.rect_filled(
        rect,
        CornerRadius::same(8),
        lerp_color(Color32::TRANSPARENT, hover_bg, hover),
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        theme::icon(10.0),
        lerp_color(MUTED, Color32::WHITE, hover),
    );
    resp
}

/// Rounded "chip" with a coloured dot or icon.
pub fn chip(ui: &mut Ui, icon: Option<&str>, text: &str, color: Color32) {
    let painter = ui.painter();
    let text_g = painter.layout_no_wrap(text.to_owned(), semibold(12.0), color);
    let icon_g = icon.map(|i| painter.layout_no_wrap(i.to_owned(), theme::icon(11.0), color));
    let lead = icon_g.as_ref().map_or(7.0, |g| g.size().x);
    let size = vec2(10.0 + lead + 6.0 + text_g.size().x + 12.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(13),
        color.gamma_multiply(0.14),
        Stroke::new(1.0, color.gamma_multiply(0.35)),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.left() + 10.0;
    match icon_g {
        Some(g) => {
            let w = g.size().x;
            painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
            x += w;
        }
        None => {
            painter.circle_filled(pos2(x + 3.5, rect.center().y), 3.5, color);
            x += 7.0;
        }
    }
    painter.galley(
        pos2(x + 6.0, rect.center().y - text_g.size().y / 2.0),
        text_g,
        color,
    );
}

/// Compact statistic card of exactly `size`: coloured dot + caption, big value, subtitle.
pub fn stat_card(ui: &mut Ui, size: Vec2, caption: &str, value: &str, sub: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        CARD,
        Stroke::new(1.0, STROKE),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(16.0, 14.0));
    // A detached child keeps the parent's cursor at exactly `size`.
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ui.set_clip_rect(rect);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, color);
        label(ui, caption, regular(12.5), MUTED);
    });
    label(&mut ui, value, semibold(21.0), TEXT);
    label(&mut ui, sub, regular(11.5), FAINT);
}

/// Row with title + description on the left and a toggle on the right.
pub fn setting_toggle(ui: &mut Ui, title: &str, desc: &str, value: &mut bool) -> bool {
    let full_w = ui.available_width();
    let text = ui.allocate_ui_with_layout(
        vec2(full_w - 64.0, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            label(ui, title, semibold(14.0), TEXT);
            ui.add(
                egui::Label::new(egui::RichText::new(desc).font(regular(12.5)).color(MUTED)).wrap(),
            );
        },
    );
    let block = text.response.rect;
    let toggle_rect = Rect::from_center_size(
        pos2(block.left() + full_w - 21.0, block.center().y),
        vec2(42.0, 24.0),
    );
    let mut changed = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(toggle_rect), |ui| {
        changed = toggle(ui, value).changed();
    });
    changed
}

/// Segmented control; returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    Frame::new()
        .fill(Color32::from_rgb(19, 22, 31))
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (opt, text) in options {
                    let selected = *value == *opt;
                    let galley =
                        ui.painter()
                            .layout_no_wrap(text.to_string(), semibold(13.0), TEXT);
                    let size = vec2(galley.size().x + 26.0, 30.0);
                    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                    let t = ui.ctx().animate_bool_with_time(resp.id, selected, 0.15);
                    let hover =
                        ui.ctx()
                            .animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.1);
                    let p = ui.painter();
                    p.rect_filled(
                        rect,
                        CornerRadius::same(8),
                        lerp_color(lerp_color(Color32::TRANSPARENT, CARD_HI, hover), ACCENT, t),
                    );
                    let fg = lerp_color(MUTED, Color32::WHITE, t.max(hover * 0.5));
                    p.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        *text,
                        semibold(13.0),
                        fg,
                    );
                    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() && !selected {
                        *value = *opt;
                        changed = true;
                    }
                }
            });
        });
    changed
}
