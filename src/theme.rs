//! Windows 11 look: colours, Segoe UI fonts, rounded widgets, avatars and icons.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Theme};
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke as SkStroke, Transform};

pub struct Palette {
    pub bg: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub note: Color32,
    pub partial: Color32,
    pub found: Color32,
    pub current: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub error: Color32,
    pub me: Color32,
    pub them: Color32,
    pub speakers: [Color32; 7],
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const LIGHT: Palette = Palette {
    bg: hex(0xfafafa),
    text: hex(0x1b1b1b),
    muted: hex(0x6b6b6b),
    note: hex(0x8a8a8a),
    partial: hex(0x9a9a9a),
    found: hex(0xfff0a8),
    current: hex(0xffc94d),
    accent: hex(0x005fb8),
    on_accent: Color32::WHITE,
    error: hex(0xe5484d),
    me: hex(0x1a7f37),
    them: hex(0x0b5cad),
    speakers: [hex(0x0b5cad), hex(0xa1349a), hex(0xb35900), hex(0x00838f), hex(0x6d4c41), hex(0x5e35b1), hex(0xc62828)],
};

pub const DARK: Palette = Palette {
    bg: hex(0x1c1c1c),
    text: hex(0xececec),
    muted: hex(0x9d9d9d),
    note: hex(0x8a8a8a),
    partial: hex(0x7d7d7d),
    found: hex(0x5c4d00),
    current: hex(0x8a6d00),
    accent: hex(0x60cdff),
    on_accent: hex(0x000000),
    error: hex(0xe5484d),
    me: hex(0x4ade80),
    them: hex(0x60a5fa),
    speakers: [hex(0x60a5fa), hex(0xe879f9), hex(0xfb923c), hex(0x22d3ee), hex(0xd6a77a), hex(0xa78bfa), hex(0xf87171)],
};

pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

pub fn status_color(state: crate::app::State, dark: bool) -> Color32 {
    use crate::app::State::*;
    match (state, dark) {
        (Recording, false) => hex(0x16a34a),
        (Recording, true) => hex(0x22c55e),
        (Waiting, false) => hex(0x2f6fd6),
        (Waiting, true) => hex(0x60a5fa),
        (Paused, false) => hex(0x8a8a8a),
        (Paused, true) => hex(0x9d9d9d),
        (Loading, _) => hex(0xd97706),
        (Error, _) => hex(0xe5484d),
    }
}

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}
pub fn italic() -> FontFamily {
    FontFamily::Name("italic".into())
}

/// Whether the Windows icon font loaded (Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets on Windows 10).
static HAS_ICONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub const ICON_SETTINGS: char = '\u{E713}';
pub const ICON_CAMERA: char = '\u{E722}';

/// An icon from the Windows icon font (the glyphs Settings and Explorer use), or `fallback` if it is missing.
pub fn icon(glyph: char, fallback: &str) -> egui::RichText {
    if HAS_ICONS.load(std::sync::atomic::Ordering::Relaxed) {
        egui::RichText::new(glyph.to_string()).family(FontFamily::Name("icons".into())).size(14.0)
    } else {
        egui::RichText::new(fallback)
    }
}

/// Segoe UI from the Windows fonts folder, with egui's built-in fonts as fallback for symbols and emoji.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let mut load = |name: &str, file: &str| -> bool {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let regular = load("segoe", "segoeui.ttf");
    let bold_ok = load("segoe-bold", "segoeuib.ttf");
    let italic_ok = load("segoe-italic", "segoeuii.ttf");
    let symbols = load("segoe-symbol", "seguisym.ttf");
    let icons = load("icons", "SegoeIcons.ttf") || load("icons", "segmdl2.ttf");
    HAS_ICONS.store(icons, std::sync::atomic::Ordering::Relaxed);
    if icons {
        fonts.families.insert(FontFamily::Name("icons".into()), vec!["icons".into()]);
    }
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |primary: Option<&str>| {
        let mut v: Vec<String> = primary.map(|p| vec![p.to_string()]).unwrap_or_default();
        if regular && primary != Some("segoe") {
            v.push("segoe".into());
        }
        v.extend(fallbacks.iter().cloned());
        if symbols {
            v.push("segoe-symbol".into());
        }
        v
    };
    fonts.families.insert(FontFamily::Proportional, family(regular.then_some("segoe")));
    fonts.families.insert(bold(), family(bold_ok.then_some("segoe-bold")));
    fonts.families.insert(italic(), family(italic_ok.then_some("segoe-italic")));
    ctx.set_fonts(fonts);
}

/// Rounded, flat widgets and Windows 11 surface colours for both themes.
pub fn install_style(ctx: &egui::Context) {
    for (theme, dark) in [(Theme::Light, false), (Theme::Dark, true)] {
        ctx.style_mut_of(theme, |style| {
            let p = palette(dark);
            let v = &mut style.visuals;
            v.panel_fill = if dark { hex(0x202020) } else { hex(0xf3f3f3) };
            v.window_fill = if dark { hex(0x2b2b2b) } else { hex(0xfbfbfb) };
            v.extreme_bg_color = if dark { hex(0x1c1c1c) } else { hex(0xffffff) };
            v.selection.bg_fill = p.accent.gamma_multiply(0.35);
            v.selection.stroke = Stroke::new(1.0, p.text);
            v.hyperlink_color = p.accent;
            let radius = CornerRadius::same(5);
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.corner_radius = radius;
            }
            v.widgets.noninteractive.corner_radius = radius;
            v.widgets.inactive.weak_bg_fill = if dark { hex(0x2d2d2d) } else { hex(0xfbfbfb) };
            v.widgets.inactive.bg_fill = v.widgets.inactive.weak_bg_fill;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, if dark { hex(0x3a3a3a) } else { hex(0xe0e0e0) });
            v.widgets.hovered.weak_bg_fill = if dark { hex(0x323232) } else { hex(0xf5f5f5) };
            v.widgets.hovered.bg_fill = v.widgets.hovered.weak_bg_fill;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, if dark { hex(0x444444) } else { hex(0xd5d5d5) });
            v.window_corner_radius = CornerRadius::same(8);
            v.menu_corner_radius = CornerRadius::same(8);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
            style.spacing.interact_size.y = 26.0;
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            // Like native controls: label and button text in the Windows text colour (egui's default is a
            // mid grey, which makes text look thinner) at 12 px (Segoe UI 9 pt; egui's default is 13 px).
            let w = &mut style.visuals.widgets;
            for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                s.fg_stroke.color = p.text;
            }
            for t in [egui::TextStyle::Body, egui::TextStyle::Button] {
                style.text_styles.insert(t, egui::FontId::new(12.0, FontFamily::Proportional));
            }
        });
    }
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| c.is_whitespace() || c == '-').filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [] => "?".into(),
        [one] => {
            let mut c = one.chars();
            let first: String = c.next().map(|f| f.to_uppercase().collect()).unwrap_or_default();
            first + &c.next().map(|s| s.to_lowercase().collect::<String>()).unwrap_or_default()
        }
        [first, .., last] => {
            let f: String = first.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            if last.chars().all(|c| c.is_ascii_digit()) {
                f + last
            } else {
                f + &last.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default()
            }
        }
    }
}

/// Round badge with a white microphone, as RGBA pixels.
pub fn mic_icon(size: u32, color: [u8; 3]) -> Vec<u8> {
    let mut pm = Pixmap::new(size, size).expect("icon size");
    let u = size as f32 / 64.0;
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(color[0], color[1], color[2], 255);
    if let Some(circle) = PathBuilder::from_circle(32.0 * u, 32.0 * u, 31.0 * u) {
        pm.fill_path(&circle, &paint, FillRule::Winding, Transform::identity(), None);
    }
    paint.set_color_rgba8(255, 255, 255, 255);
    // Mic capsule: a thick round-capped line.
    let stroke = |w: f32| SkStroke { width: w * u, line_cap: LineCap::Round, ..Default::default() };
    let mut capsule = PathBuilder::new();
    capsule.move_to(32.0 * u, 20.0 * u);
    capsule.line_to(32.0 * u, 31.0 * u);
    if let Some(p) = capsule.finish() {
        pm.stroke_path(&p, &paint, &stroke(14.0), Transform::identity(), None);
    }
    // Holder arc, stem and base.
    let mut holder = PathBuilder::new();
    holder.move_to(19.0 * u, 31.0 * u);
    holder.cubic_to(19.0 * u, 48.0 * u, 45.0 * u, 48.0 * u, 45.0 * u, 31.0 * u);
    holder.move_to(32.0 * u, 44.0 * u);
    holder.line_to(32.0 * u, 52.0 * u);
    holder.move_to(25.0 * u, 52.0 * u);
    holder.line_to(39.0 * u, 52.0 * u);
    if let Some(p) = holder.finish() {
        pm.stroke_path(&p, &paint, &stroke(3.5), Transform::identity(), None);
    }
    // tiny-skia stores premultiplied colours; icons want straight alpha.
    pm.pixels()
        .iter()
        .flat_map(|px| {
            let c = px.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn initials() {
        assert_eq!(super::initials("Speaker 3"), "S3");
        assert_eq!(super::initials("Hanna"), "Ha");
        assert_eq!(super::initials("Hanna Meier"), "HM");
        assert_eq!(super::initials(""), "?");
    }
}
