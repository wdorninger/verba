//! Minimal Markdown display for Claude's output: #/## headings, - bullets, **bold**, [links](url) as text.

use eframe::egui::{self, Color32, FontFamily, FontId, RichText, text::LayoutJob};
use regex::Regex;

use crate::theme;

pub fn show(ui: &mut egui::Ui, text: &str, size: f32, color: Color32) {
    let link = Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap();
    let bullet = Regex::new(r"^[-*]\s+").unwrap();
    for line in text.lines() {
        let stripped = link.replace_all(line.trim(), "$1").into_owned();
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with('#') {
            let level = stripped.chars().take_while(|c| *c == '#').count();
            let heading = stripped.trim_start_matches('#').trim();
            ui.add_space(if level == 1 { 6.0 } else { 8.0 });
            ui.label(RichText::new(heading).font(FontId::new(size + if level == 1 { 6.0 } else { 3.0 }, theme::bold())).color(color));
            continue;
        }
        let (prefix, body) = if bullet.is_match(&stripped) {
            ("•  ", bullet.replace(&stripped, "").into_owned())
        } else {
            ("", stripped)
        };
        let mut job = LayoutJob::default();
        let normal = egui::TextFormat::simple(FontId::new(size, FontFamily::Proportional), color);
        let strong = egui::TextFormat::simple(FontId::new(size, theme::bold()), color);
        job.append(prefix, if prefix.is_empty() { 0.0 } else { 6.0 }, normal.clone());
        for (i, part) in body.split("**").enumerate() {
            job.append(part, 0.0, if i % 2 == 1 { strong.clone() } else { normal.clone() });
        }
        ui.add(egui::Label::new(job).wrap());
    }
}
