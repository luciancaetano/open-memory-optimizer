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

//! Palette, typography and global egui style.

use eframe::egui;
use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
};

pub const BG: Color32 = Color32::from_rgb(13, 15, 21);
pub const SIDEBAR: Color32 = Color32::from_rgb(17, 20, 28);
pub const CARD: Color32 = Color32::from_rgb(23, 27, 37);
pub const CARD_HI: Color32 = Color32::from_rgb(30, 35, 48);
pub const STROKE: Color32 = Color32::from_rgb(38, 43, 57);
pub const TRACK: Color32 = Color32::from_rgb(40, 45, 60);

pub const TEXT: Color32 = Color32::from_rgb(231, 234, 242);
pub const MUTED: Color32 = Color32::from_rgb(140, 148, 168);
pub const FAINT: Color32 = Color32::from_rgb(92, 100, 120);

pub const CYAN: Color32 = Color32::from_rgb(34, 211, 238);
pub const VIOLET: Color32 = Color32::from_rgb(139, 92, 246);
pub const ACCENT: Color32 = Color32::from_rgb(109, 93, 252);
pub const ACCENT_HI: Color32 = Color32::from_rgb(129, 115, 255);
pub const GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const AMBER: Color32 = Color32::from_rgb(251, 191, 36);
pub const RED: Color32 = Color32::from_rgb(248, 113, 113);

pub const RADIUS: u8 = 14;

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// Status colour for a memory-load percentage.
pub fn level_color(used_pct: f32) -> Color32 {
    if used_pct < 60.0 {
        GREEN
    } else if used_pct < 85.0 {
        AMBER
    } else {
        RED
    }
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

/// Segoe Fluent Icons / MDL2 Assets code points (shared by both fonts).
pub mod icons {
    pub const DASHBOARD: &str = "\u{E9D2}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const CLOCK: &str = "\u{E823}";
    pub const INFO: &str = "\u{E946}";
    pub const MINIMIZE: &str = "\u{E921}";
    pub const CLOSE: &str = "\u{E8BB}";
    pub const BOLT: &str = "\u{E945}";
    pub const SHIELD: &str = "\u{EA18}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const CHECK: &str = "\u{E73E}";
    pub const DELETE: &str = "\u{E74D}";
    pub const ADD: &str = "\u{E710}";
}

fn load_system_font(fonts: &mut FontDefinitions, name: &str, files: &[&str]) -> bool {
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    for file in files {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts
                .font_data
                .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            return true;
        }
    }
    false
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let family = |fonts: &mut FontDefinitions, name: &str, primary: &[(&str, &[&str])]| {
        let mut list = Vec::new();
        for (font, files) in primary {
            if load_system_font(fonts, font, files) {
                list.push(font.to_string());
            }
        }
        list.extend(fallback.iter().cloned());
        let fam = if name == "proportional" {
            FontFamily::Proportional
        } else {
            FontFamily::Name(name.into())
        };
        fonts.families.insert(fam, list);
    };

    family(&mut fonts, "proportional", &[("segoeui", &["segoeui.ttf"])]);
    family(
        &mut fonts,
        "semibold",
        &[("seguisb", &["seguisb.ttf"]), ("segoeui", &["segoeui.ttf"])],
    );
    family(
        &mut fonts,
        "bold",
        &[
            ("segoeuib", &["segoeuib.ttf"]),
            ("segoeui", &["segoeui.ttf"]),
        ],
    );
    family(
        &mut fonts,
        "icons",
        &[("segoeicons", &["SegoeIcons.ttf", "segmdl2.ttf"])],
    );
    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(TEXT);
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = Color32::from_rgb(19, 22, 31);
        v.faint_bg_color = CARD_HI;
        v.window_stroke = Stroke::new(1.0, STROKE);
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.hyperlink_color = CYAN;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
        v.selection.stroke = Stroke::new(1.0, ACCENT_HI);

        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.noninteractive, CARD),
            (&mut w.inactive, CARD_HI),
            (&mut w.hovered, Color32::from_rgb(38, 44, 60)),
            (&mut w.active, Color32::from_rgb(46, 53, 72)),
            (&mut w.open, CARD_HI),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = CornerRadius::same(8);
            state.bg_stroke = Stroke::new(1.0, STROKE);
        }
        w.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.8));
        w.active.bg_stroke = Stroke::new(1.0, ACCENT_HI);
        w.hovered.expansion = 0.0;
        w.active.expansion = 0.0;

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(10.0, 10.0);
        s.button_padding = egui::vec2(14.0, 7.0);
        s.interact_size.y = 30.0;
        s.slider_width = 260.0;
        s.window_margin = Margin::same(16);

        style
            .text_styles
            .insert(egui::TextStyle::Body, regular(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, regular(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, regular(12.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, semibold(22.0));
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        );
    });
}

static ICON_PNG: &[u8] = include_bytes!("../../icon.png");

/// icon.png at full resolution. If the artwork has no transparency, its black corners
/// are made transparent: opaque inside the rounded square, fading by brightness outside.
fn icon_master() -> &'static image::RgbaImage {
    static IMG: std::sync::OnceLock<image::RgbaImage> = std::sync::OnceLock::new();
    IMG.get_or_init(|| {
        let mut img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
            .map(|i| i.to_rgba8())
            .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
        // Only needed for artwork without transparency (black corners).
        if img.pixels().any(|p| p[3] < 255) {
            return img;
        }
        let (w, h) = (img.width() as f32, img.height() as f32);
        let side = w.min(h);
        let inset = side * 0.035;
        let radius = side * 0.16;
        for (x, y, px) in img.enumerate_pixels_mut() {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = fx.clamp(inset + radius, w - inset - radius);
            let cy = fy.clamp(inset + radius, h - inset - radius);
            let outside = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() - radius;
            if outside > 0.0 {
                let brightness = px[0].max(px[1]).max(px[2]) as f32;
                px[3] = ((brightness / 48.0).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
        img
    })
}

/// The app icon (icon.png) as straight RGBA at `size` x `size`.
pub fn app_icon_rgba(size: u32) -> Vec<u8> {
    image::imageops::resize(
        icon_master(),
        size,
        size,
        image::imageops::FilterType::Lanczos3,
    )
    .into_raw()
}
