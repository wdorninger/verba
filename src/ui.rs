//! Live transcript window (egui). Closing it only hides it; the tray brings it back.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crossbeam_channel::Receiver;
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontFamily, FontId, Id, Key, Label, Layout, Margin, RichText, Sense,
    TextFormat, ThemePreference, ViewportBuilder, ViewportCommand, ViewportId, WindowLevel, text::LayoutJob,
};
use toml::Value;

use crate::app::{Core, State, UiEvent};
use crate::markdown;
use crate::theme::{self, Palette};
use crate::tray::{Tray, TrayAction};

const LANGUAGES: &[(&str, &str)] =
    &[("Auto-detect", ""), ("Deutsch", "de"), ("English", "en"), ("Français", "fr"), ("Español", "es"), ("Italiano", "it")];
const RECORD_CALLS: &str = "Only calls";
const RECORD_ALL: &str = "All audio";
const FIXED_IDS: &[&str] = &["me", "them"]; // not recognised voices, so not renamable
const MODELS: &[&str] = &["haiku", "sonnet", "opus"];
const WHISPER_MODELS: &[&str] = &["tiny", "base", "small", "medium", "large-v3", "large-v3-turbo", "large-v3-turbo-q5_0"];

enum Entry {
    Line { time: String, speaker_id: String, text: String },
    Note(String),
    Shot { time: String, path: PathBuf },
}

struct SpeakerUi {
    name: String,
    color_index: usize,
}

enum AnswerState {
    Thinking,
    Done(String),
    Error(String),
}

struct Card {
    qid: u64,
    time: String,
    label: String,
    question: String,
    state: AnswerState,
}

/// Inline rename of a speaker, edited in place of the name on one transcript entry.
struct Rename {
    entry: usize,
    speaker_id: String,
    name: String,
    focus: bool,
}

struct Summary {
    id: u64,
    title: String,
    text: String,
    path: Option<PathBuf>,
    open: bool,
}

/// Settings dialog values, keyed like config.toml ("section.key").
struct SettingsDialog {
    values: BTreeMap<String, Value>,
    original: BTreeMap<String, Value>,
}

pub struct LiveApp {
    core: Arc<Core>,
    events: Receiver<UiEvent>,
    tray: Option<(Tray, Receiver<TrayAction>)>,
    entries: Vec<Entry>,
    heights: Vec<f32>,
    layout_key: (u32, u32),
    speakers: HashMap<String, SpeakerUi>,
    partials: BTreeMap<String, String>,
    cards: Vec<Card>,
    answers_visible: bool,
    claude_on: bool,
    answers_on: bool,
    summarizing: bool,
    language: String,
    search: String,
    current_match: Option<(usize, usize)>,
    scroll_to: Option<usize>,
    font_size: f32,
    on_top: bool,
    theme_mode: String,
    rename: Option<Rename>,
    settings: Option<SettingsDialog>,
    summaries: Vec<Summary>,
    next_summary: u64,
    quitting: bool,
}

/// Case-insensitive matches as byte ranges.
fn find_all(hay: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let hay_l: Vec<(usize, char)> = hay.char_indices().flat_map(|(i, c)| c.to_lowercase().map(move |l| (i, l))).collect();
    let needle_l: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle_l.len() <= hay_l.len() {
        if hay_l[i..i + needle_l.len()].iter().map(|(_, c)| *c).eq(needle_l.iter().copied()) {
            let start = hay_l[i].0;
            let last = hay_l[i + needle_l.len() - 1].0;
            let end = last + hay[last..].chars().next().map_or(1, char::len_utf8);
            out.push((start, end));
            i += needle_l.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Windows 11 style toggle switch with a label.
fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str, p: &Palette) -> egui::Response {
    ui.horizontal(|ui| {
        let size = egui::vec2(36.0, 18.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        let text = ui.add(Label::new(label).sense(Sense::click()));
        if resp.clicked() || text.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        let enabled = ui.is_enabled();
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let radius = rect.height() / 2.0;
        let (fill, knob, stroke) = if *on {
            (p.accent, p.on_accent, egui::Stroke::NONE)
        } else {
            (Color32::TRANSPARENT, p.muted, egui::Stroke::new(1.0, p.muted))
        };
        let fill = if enabled { fill } else { fill.gamma_multiply(0.4) };
        ui.painter().rect(rect, radius, fill, stroke, egui::StrokeKind::Inside);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), radius * 0.55, knob);
        resp | text
    })
    .inner
}

fn accent_button(ui: &mut egui::Ui, text: &str, p: &Palette, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).color(p.on_accent)).fill(p.accent))
}

impl LiveApp {
    pub fn new(cc: &eframe::CreationContext<'_>, core: Arc<Core>, events: Receiver<UiEvent>) -> LiveApp {
        theme::install_fonts(&cc.egui_ctx);
        theme::install_style(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let _ = core.ui.ctx.set(cc.egui_ctx.clone());
        let tray = match Tray::new(cc.egui_ctx.clone(), core.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                log::error!("No tray icon: {e:#}");
                None
            }
        };
        let (claude_on, answers_on, language, font_size, on_top, theme_mode) = {
            let c = core.cfg.lock().unwrap();
            (
                c.bool("summary", "enabled"),
                c.bool("answers", "enabled"),
                c.str("whisper", "language"),
                c.f64("ui", "font_size") as f32,
                c.bool("ui", "always_on_top"),
                c.str("ui", "theme"),
            )
        };
        let app = LiveApp {
            core,
            events,
            tray,
            entries: Vec::new(),
            heights: Vec::new(),
            layout_key: (0, 0),
            speakers: HashMap::new(),
            partials: BTreeMap::new(),
            cards: Vec::new(),
            answers_visible: answers_on,
            claude_on,
            answers_on,
            summarizing: false,
            language,
            search: String::new(),
            current_match: None,
            scroll_to: None,
            font_size: font_size.clamp(7.0, 32.0),
            on_top,
            theme_mode,
            rename: None,
            settings: None,
            summaries: Vec::new(),
            next_summary: 1,
            quitting: false,
        };
        app.apply_theme(&cc.egui_ctx);
        if on_top {
            cc.egui_ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
        }
        app
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        ctx.set_theme(match self.theme_mode.as_str() {
            "dark" => ThemePreference::Dark,
            "light" => ThemePreference::Light,
            _ => ThemePreference::System,
        });
    }

    fn save(&self, section: &str, key: &str, value: Value) {
        self.core.apply_settings(vec![(format!("{section}.{key}"), value)]);
    }

    /// Text size for the transcript: config points as in Tk, egui sizes are pixels.
    fn px(&self) -> f32 {
        self.font_size * 4.0 / 3.0
    }

    fn speaker_color(&self, id: &str, p: &Palette) -> Color32 {
        match id {
            "me" => p.me,
            "them" => p.them,
            _ => p.speakers[self.speakers.get(id).map_or(0, |s| s.color_index) % p.speakers.len()],
        }
    }

    fn show_window(ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    // ------------------------------------------------------------------ events
    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(e) = self.events.try_recv() {
            match e {
                UiEvent::Line { time, speaker_id, label, text } => {
                    let count = self.speakers.keys().filter(|k| !FIXED_IDS.contains(&k.as_str())).count();
                    let sp = self.speakers.entry(speaker_id.clone()).or_insert(SpeakerUi { name: label.clone(), color_index: count });
                    sp.name = label; // renamed elsewhere (or merged)
                    self.entries.push(Entry::Line { time, speaker_id, text });
                }
                UiEvent::Partial { label, text } => {
                    self.partials.insert(label, text);
                }
                UiEvent::Note(n) => self.entries.push(Entry::Note(n)),
                UiEvent::Screenshot { time, path } => self.entries.push(Entry::Shot { time, path }),
                UiEvent::Summarizing(b) => self.summarizing = b,
                UiEvent::Summary { title, text, path } => {
                    self.summarizing = false;
                    self.summaries.push(Summary { id: self.next_summary, title, text, path, open: true });
                    self.next_summary += 1;
                    Self::show_window(ctx);
                }
                UiEvent::AnswerPending { qid, time, label, question } => {
                    self.cards.push(Card { qid, time, label, question, state: AnswerState::Thinking })
                }
                UiEvent::AnswerDone { qid, text } => {
                    if let Some(c) = self.cards.iter_mut().find(|c| c.qid == qid) {
                        c.state = AnswerState::Done(text);
                    }
                }
                UiEvent::AnswerError { qid, message } => {
                    if let Some(c) = self.cards.iter_mut().find(|c| c.qid == qid) {
                        c.state = AnswerState::Error(message);
                    }
                }
                UiEvent::AnswerSkipped(qid) => self.cards.retain(|c| c.qid != qid),
            }
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.tray else { return };
        let actions: Vec<TrayAction> = rx.try_iter().collect();
        for a in actions {
            match a {
                TrayAction::Show => Self::show_window(ctx),
                TrayAction::Pause => self.core.toggle_pause(),
                TrayAction::CallsOnly => self.core.set_calls_only(!self.core.calls_only.load(Ordering::Relaxed)),
                TrayAction::Summarize => self.core.summarize(None, false),
                TrayAction::Screenshot => crate::screenshots::capture_now(self.core.clone()),
                TrayAction::NewTranscript => self.new_transcript(),
                TrayAction::OpenTranscript => self.core.open_transcript(),
                TrayAction::OpenFolder => self.core.open_folder(),
                TrayAction::RestartAudio => self.core.capture.restart(),
                TrayAction::Quit => crate::tray::quit(&self.core),
            }
        }
    }

    // ------------------------------------------------------------------ actions
    fn clear(&mut self) {
        self.entries.clear();
        self.heights.clear();
        self.partials.clear();
        self.current_match = None;
    }

    fn new_transcript(&mut self) {
        self.clear();
        self.core.new_transcript();
    }

    fn copy_all(&self, ctx: &egui::Context) {
        let mut out = String::new();
        for e in &self.entries {
            match e {
                Entry::Line { time, speaker_id, text } => {
                    let name = self.speakers.get(speaker_id).map_or("?", |s| s.name.as_str());
                    out.push_str(&format!("[{time}] [{name}] {text}\n"));
                }
                Entry::Note(n) => out.push_str(&format!("{n}\n")),
                Entry::Shot { time, path } => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    out.push_str(&format!("[{time}] [Screenshot] {name}\n"));
                }
            }
        }
        ctx.copy_text(out);
    }

    fn zoom(&mut self, step: f32) {
        let size = (self.font_size + step).clamp(7.0, 32.0);
        if size != self.font_size {
            self.font_size = size;
            self.save("ui", "font_size", Value::Integer(size as i64));
        }
    }

    fn find_next(&mut self) {
        let matches: Vec<(usize, usize)> = self
            .entries
            .iter()
            .enumerate()
            .flat_map(|(i, e)| match e {
                Entry::Line { text, .. } => (0..find_all(text, &self.search).len()).map(|k| (i, k)).collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        if matches.is_empty() {
            self.current_match = None;
            return;
        }
        let next = match self.current_match {
            Some(cur) => matches.iter().find(|m| **m > cur).copied().unwrap_or(matches[0]),
            None => matches[0],
        };
        self.current_match = Some(next);
        self.scroll_to = Some(next.0);
    }

    fn set_claude(&mut self, on: bool) {
        self.claude_on = on;
        self.save("summary", "enabled", Value::Boolean(on));
    }

    fn set_answers(&mut self, on: bool) {
        self.answers_on = on;
        self.answers_visible = on;
        self.save("answers", "enabled", Value::Boolean(on));
    }

    fn set_language(&mut self, code: &str) {
        self.language = code.to_string();
        log::info!("Language set to {}", if code.is_empty() { "auto-detect" } else { code });
        self.save("whisper", "language", Value::String(code.into()));
    }

    // ------------------------------------------------------------------ layout
    fn header(&mut self, ui: &mut egui::Ui, p: &Palette, dark: bool) {
        ui.horizontal(|ui| {
            let state = self.core.state();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, theme::status_color(state, dark));
            if state == State::Loading {
                // Start-up progress: a bar while downloading, a spinner otherwise.
                let progress = crate::models::progress();
                match progress.as_ref().and_then(|(_, f)| *f) {
                    Some(f) => {
                        ui.add(egui::ProgressBar::new(f).desired_width(120.0).show_percentage());
                    }
                    None => {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                }
                let text = progress.map_or_else(|| "Loading model…".to_string(), |(t, _)| t);
                ui.label(RichText::new(text).font(FontId::new(14.0, theme::bold())));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            } else {
                ui.label(RichText::new(self.core.status()).font(FontId::new(14.0, theme::bold())));
            }
            self.engine_badge(ui, dark);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("⋯", |ui| {
                    if ui.checkbox(&mut self.on_top, "Always on top").changed() {
                        let level = if self.on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal };
                        ui.ctx().send_viewport_cmd(ViewportCommand::WindowLevel(level));
                        self.save("ui", "always_on_top", Value::Boolean(self.on_top));
                    }
                    ui.separator();
                    if ui.button("Open transcript").clicked() {
                        self.core.open_transcript();
                    }
                    if ui.button("Open transcripts folder").clicked() {
                        self.core.open_folder();
                    }
                    ui.separator();
                    if ui.button("Copy all").clicked() {
                        self.copy_all(ui.ctx());
                    }
                    if ui.button("Clear window").clicked() {
                        self.clear();
                    }
                });
                if ui.button("⚙").on_hover_text("Settings").clicked() {
                    let values: BTreeMap<String, Value> = self.core.get_settings().into_iter().collect();
                    self.settings = Some(SettingsDialog { original: values.clone(), values });
                }
                let paused = self.core.manual_pause.load(Ordering::Relaxed);
                if accent_button(ui, if paused { "Resume" } else { "Pause" }, p, state != State::Loading).clicked() {
                    self.core.toggle_pause();
                }
            });
        });
    }

    /// Shows what speech recognition runs on, so a slow CPU fallback is obvious.
    fn engine_badge(&self, ui: &mut egui::Ui, dark: bool) {
        let Some(info) = self.core.engine() else { return };
        let color = if info.gpu {
            theme::status_color(State::Recording, dark)
        } else {
            Color32::from_rgb(0xe0, 0x7b, 0x00) // orange: works, but slower
        };
        let text = if info.fallback.is_some() { format!("{} (fallback)", info.label) } else { info.label.clone() };
        let mut tip = info.detail.clone();
        if let Some(reason) = &info.fallback {
            tip.push_str(&format!("

The GPU backend could not start, so transcription is slower.
Reason: {reason}"));
        } else if !info.gpu {
            tip.push_str("

Set [whisper] backend = \"gpu\" (or ⚙ Settings → Run on) for faster transcription.");
        }
        ui.add_space(6.0);
        egui::Frame::NONE
            .fill(color.gamma_multiply(0.16))
            .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6)))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(8, 2))
            .show(ui, |ui| ui.label(RichText::new(text).size(12.0).color(color)))
            .response
            .on_hover_text(tip);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let current = LANGUAGES.iter().find(|(_, c)| *c == self.language).map_or(self.language.as_str(), |(n, _)| n);
            let mut chosen = None;
            egui::ComboBox::from_id_salt("language").selected_text(current).width(120.0).show_ui(ui, |ui| {
                for (name, code) in LANGUAGES {
                    if ui.selectable_label(self.language == *code, *name).clicked() {
                        chosen = Some(*code);
                    }
                }
            });
            if let Some(code) = chosen {
                self.set_language(code);
            }
            let calls_only = self.core.calls_only.load(Ordering::Relaxed);
            let mut mode = calls_only;
            egui::ComboBox::from_id_salt("record")
                .selected_text(if mode { RECORD_CALLS } else { RECORD_ALL })
                .width(100.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut mode, true, RECORD_CALLS);
                    ui.selectable_value(&mut mode, false, RECORD_ALL);
                });
            if mode != calls_only {
                self.core.set_calls_only(mode);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("A+").clicked() {
                    self.zoom(1.0);
                }
                if ui.button("A−").clicked() {
                    self.zoom(-1.0);
                }
                if ui.button("📷").on_hover_text("Take a screenshot (screen and auto mode: ⚙ Settings)").clicked() {
                    crate::screenshots::capture_now(self.core.clone());
                }
                ui.add_space(8.0);
                let resp = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("🔍 Search").desired_width(180.0));
                if resp.changed() {
                    self.current_match = None;
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.find_next();
                    resp.request_focus();
                }
                if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.search.clear();
                    self.current_match = None;
                }
            });
        });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            let mut claude = self.claude_on;
            if switch(ui, &mut claude, "Claude", p).changed() {
                self.set_claude(claude);
            }
            ui.add_space(10.0);
            let mut answers = self.answers_on;
            let changed = ui.add_enabled_ui(self.claude_on, |ui| switch(ui, &mut answers, "Answer questions", p).changed()).inner;
            if changed {
                self.set_answers(answers);
            }
            ui.add_space(10.0);
            let label = if self.summarizing { "Summarizing…" } else { "Summarize" };
            if accent_button(ui, label, p, self.claude_on && !self.summarizing).clicked() {
                self.summarizing = true;
                self.core.summarize(None, false);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.clear();
                }
                if ui.button("Copy all").clicked() {
                    self.copy_all(ui.ctx());
                }
                if ui.button("➕ New transcript").on_hover_text("Start a new transcript file and folder; clears the window").clicked() {
                    self.new_transcript();
                }
            });
        });
    }

    fn body_job(&self, text: &str, index: usize, p: &Palette) -> LayoutJob {
        let fmt = TextFormat::simple(FontId::new(self.px(), FontFamily::Proportional), p.text);
        let mut job = LayoutJob::default();
        let mut pos = 0;
        for (k, (s, e)) in find_all(text, &self.search).into_iter().enumerate() {
            job.append(&text[pos..s], 0.0, fmt.clone());
            let bg = if self.current_match == Some((index, k)) { p.current } else { p.found };
            job.append(&text[s..e], 0.0, TextFormat { background: bg, ..fmt.clone() });
            pos = e;
        }
        job.append(&text[pos..], 0.0, fmt);
        job
    }

    /// One transcript entry; returns a rename request or an "ask Claude" request.
    fn entry_ui(&self, ui: &mut egui::Ui, index: usize, p: &Palette, actions: &mut Vec<EntryAction>, rename: &mut Option<Rename>) {
        match &self.entries[index] {
            Entry::Note(n) => {
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(n).font(FontId::new(self.px(), theme::italic())).color(p.note));
                });
            }
            Entry::Shot { time, path } => {
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let link = ui.add(
                        Label::new(RichText::new(format!("📷  Screenshot {time}")).font(FontId::new(self.px(), theme::italic())).color(p.accent))
                            .sense(Sense::click()),
                    );
                    if link.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                    if link.on_hover_text(name).clicked() {
                        crate::shell_open(path);
                    }
                });
            }
            Entry::Line { time, speaker_id, text } => {
                let name = self.speakers.get(speaker_id).map_or("?", |s| s.name.as_str()).to_string();
                let renamable = !FIXED_IDS.contains(&speaker_id.as_str());
                let avatar = (self.px() * 1.75).round();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(avatar, avatar), if renamable { Sense::click() } else { Sense::hover() });
                    // Avatars keep the saturated light-theme colours so white initials stay readable.
                    let fill = self.speaker_color(speaker_id, &theme::LIGHT);
                    ui.painter().circle_filled(rect.center(), avatar / 2.0, fill);
                    let initials = theme::initials(&name);
                    let size = avatar * if initials.chars().count() <= 2 { 0.42 } else { 0.34 };
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, initials, FontId::new(size, theme::bold()), Color32::WHITE);
                    ui.add_space(4.0);
                    let font = FontId::new(self.px(), theme::bold());
                    match rename.as_mut().filter(|r| r.entry == index) {
                        Some(r) => {
                            // Editing this name in place: Enter or clicking elsewhere saves, Esc cancels.
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut r.name)
                                    .font(font)
                                    .desired_width(220.0)
                                    .hint_text("Name (an existing name merges)"),
                            );
                            if r.focus {
                                edit.request_focus();
                                r.focus = false;
                            }
                            if ui.input(|i| i.key_pressed(Key::Escape)) {
                                actions.push(EntryAction::CancelRename);
                            } else if edit.lost_focus() {
                                actions.push(EntryAction::CommitRename);
                            }
                        }
                        None => {
                            let label = ui.add(
                                Label::new(RichText::new(&name).font(font).color(self.speaker_color(speaker_id, p)))
                                    .sense(if renamable { Sense::click() } else { Sense::hover() })
                                    .selectable(false),
                            );
                            if renamable {
                                if label.hovered() || resp.hovered() {
                                    ui.ctx().set_cursor_icon(CursorIcon::Text);
                                }
                                let clicked = resp.clicked() || label.on_hover_text("Click to rename").clicked();
                                if clicked {
                                    actions.push(EntryAction::Rename(index, speaker_id.clone(), name.clone()));
                                }
                            }
                        }
                    }
                    ui.label(RichText::new(time).size((self.px() - 2.0).max(9.0)).color(p.muted));
                });
                let indent = avatar + 12.0;
                egui::Frame::NONE.inner_margin(Margin { left: indent as i8, ..Default::default() }).show(ui, |ui| {
                    let resp = ui.add(Label::new(self.body_job(text, index, p)).wrap().sense(Sense::click()));
                    resp.context_menu(|ui| {
                        if ui.add_enabled(self.claude_on, egui::Button::new("✦ Ask Claude about this")).clicked() {
                            actions.push(EntryAction::Ask(time.clone(), name.clone(), text.clone()));
                        }
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                        }
                    });
                });
            }
        }
    }

    fn transcript(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Ctrl + mouse wheel zooms the text.
        if ui.ui_contains_pointer() {
            let zoom = ui.input(|i| i.zoom_delta());
            if zoom > 1.0 {
                self.zoom(1.0);
            } else if zoom < 1.0 {
                self.zoom(-1.0);
            }
        }
        let key = (ui.available_width().round() as u32, self.font_size as u32);
        if key != self.layout_key {
            self.layout_key = key;
            self.heights.clear();
        }
        self.heights.resize(self.entries.len(), f32::NAN);
        let mut actions = Vec::new();
        let mut rename = self.rename.take();
        egui::Frame::NONE.fill(p.bg).inner_margin(Margin::symmetric(18, 8)).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
                let origin = ui.min_rect().top();
                for i in 0..self.entries.len() {
                    let top = ui.cursor().top() - origin;
                    let h = self.heights[i];
                    let target = self.scroll_to == Some(i);
                    let editing = rename.as_ref().is_some_and(|r| r.entry == i);
                    // Off-screen entries with a known height only reserve their space.
                    if !h.is_nan() && !target && !editing && (top + h < viewport.min.y - 300.0 || top > viewport.max.y + 300.0) {
                        ui.allocate_space(egui::vec2(ui.available_width(), h));
                        continue;
                    }
                    let resp = ui.scope(|ui| self.entry_ui(ui, i, p, &mut actions, &mut rename)).response;
                    self.heights[i] = resp.rect.height();
                    if target {
                        ui.scroll_to_rect(resp.rect, Some(Align::Center));
                        self.scroll_to = None;
                    }
                }
                for (label, text) in &self.partials {
                    if !text.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new(format!("{label}:  {text}")).font(FontId::new(self.px(), theme::italic())).color(p.partial));
                    }
                }
                if self.entries.is_empty() && self.partials.values().all(String::is_empty) {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("Transcript appears here.").color(p.note)));
                }
            });
        });
        self.rename = rename;
        for a in actions {
            match a {
                EntryAction::Rename(entry, id, name) => self.rename = Some(Rename { entry, speaker_id: id, name, focus: true }),
                EntryAction::CancelRename => self.rename = None,
                EntryAction::CommitRename => {
                    if let Some(r) = self.rename.take() {
                        self.apply_rename(&r.speaker_id, &r.name);
                    }
                }
                EntryAction::Ask(time, label, text) => {
                    self.answers_visible = true;
                    self.core.ask(&time, &label, &text);
                }
            }
        }
    }

    fn answers_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("✦  Claude answers").font(FontId::new(14.0, theme::bold())));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.cards.clear();
                }
            });
        });
        egui::Frame::NONE.fill(p.bg).inner_margin(Margin::symmetric(14, 6)).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                let size = self.px();
                for c in &self.cards {
                    ui.add_space(14.0);
                    ui.label(RichText::new(format!("{}  ·  {}", c.time, c.label)).size((size - 2.0).max(9.0)).color(p.muted));
                    ui.add(Label::new(RichText::new(format!("“{}”", c.question)).font(FontId::new(size, theme::italic())).color(p.muted)).wrap());
                    match &c.state {
                        AnswerState::Thinking => {
                            ui.label(RichText::new("Thinking…").font(FontId::new(size, theme::italic())).color(p.partial));
                        }
                        AnswerState::Done(text) => markdown::show(ui, text, size, p.text),
                        AnswerState::Error(e) => {
                            ui.add(Label::new(RichText::new(format!("Claude: {e}")).size(size).color(p.error)).wrap());
                        }
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------------ dialogs
    /// Names a voice (an existing name merges both) and updates every line already shown.
    fn apply_rename(&mut self, speaker_id: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() || self.speakers.get(speaker_id).is_some_and(|s| s.name == name) {
            return;
        }
        let Some((_, surviving, final_name)) = self.core.rename_speaker(speaker_id, name) else { return };
        let survivor_color = {
            let count = self.speakers.keys().filter(|k| !FIXED_IDS.contains(&k.as_str())).count();
            let s = self.speakers.entry(surviving.clone()).or_insert(SpeakerUi { name: final_name.clone(), color_index: count });
            s.name = final_name.clone();
            s.color_index
        };
        let old_id = speaker_id;
        if surviving != speaker_id {
            // Merged: earlier lines of this voice now belong to the survivor.
            for e in &mut self.entries {
                if let Entry::Line { speaker_id, .. } = e {
                    if speaker_id.as_str() == old_id {
                        *speaker_id = surviving.clone();
                    }
                }
            }
            if let Some(old) = self.speakers.get_mut(old_id) {
                old.name = final_name;
                old.color_index = survivor_color;
            }
        }
        self.heights.clear();
    }

    fn settings_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(d) = &mut self.settings else { return };
        let mut close = false;
        let mut save = false;
        egui::Modal::new(Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(440.0);
            let v = &mut d.values;
            let section = |ui: &mut egui::Ui, t: &str| {
                ui.add_space(8.0);
                ui.label(RichText::new(t).font(FontId::new(15.0, theme::bold())));
            };
            let toggle = |ui: &mut egui::Ui, v: &mut BTreeMap<String, Value>, key: &str, text: &str| {
                let mut on = v.get(key).and_then(Value::as_bool).unwrap_or(false);
                if switch(ui, &mut on, text, p).changed() {
                    v.insert(key.into(), Value::Boolean(on));
                }
            };
            let choice = |ui: &mut egui::Ui, v: &mut BTreeMap<String, Value>, key: &str, text: &str, options: &[(&str, &str)], editable: bool| {
                ui.horizontal(|ui| {
                    ui.add_sized([210.0, 22.0], Label::new(text));
                    let mut current = v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if editable {
                        ui.add(egui::TextEdit::singleline(&mut current).desired_width(130.0));
                        ui.menu_button("▾", |ui| {
                            for (value, label) in options {
                                if ui.button(*label).clicked() {
                                    current = value.to_string();
                                }
                            }
                        });
                    } else {
                        let shown = options.iter().find(|(val, _)| *val == current).map_or(current.clone(), |(_, l)| l.to_string());
                        egui::ComboBox::from_id_salt(key).selected_text(shown).width(160.0).show_ui(ui, |ui| {
                            for (value, label) in options {
                                ui.selectable_value(&mut current, value.to_string(), *label);
                            }
                        });
                    }
                    v.insert(key.into(), Value::String(current));
                });
            };
            let models: Vec<(&str, &str)> = MODELS.iter().map(|m| (*m, *m)).collect();
            section(ui, "Claude");
            toggle(ui, v, "summary.enabled", "Allow Claude usage (summaries and answers)");
            toggle(ui, v, "answers.enabled", "Answer questions automatically");
            choice(ui, v, "answers.model", "Answer model", &models, true);
            toggle(ui, v, "answers.web_search", "Allow web search for answers (slower)");
            choice(ui, v, "answers.sources", "Answer questions from", &[("all", "Everyone"), ("them", "Only others (speakers)"), ("me", "Only me (mic)")], false);
            ui.horizontal(|ui| {
                ui.add_sized([210.0, 22.0], Label::new("Context lines sent with a question"));
                let mut n = v.get("answers.context_lines").and_then(Value::as_integer).unwrap_or(15);
                if ui.add(egui::DragValue::new(&mut n).range(5..=50)).changed() {
                    v.insert("answers.context_lines".into(), Value::Integer(n));
                }
            });
            choice(ui, v, "summary.model", "Summary model", &models, true);
            toggle(ui, v, "summary.auto_after_call", "Summarize automatically after each call");

            section(ui, "Transcription");
            let langs: Vec<(&str, &str)> = LANGUAGES.iter().map(|(n, c)| (*c, *n)).collect();
            choice(ui, v, "whisper.language", "Language", &langs, false);
            ui.horizontal(|ui| {
                ui.add_sized([210.0, 22.0], Label::new("Record"));
                let mut calls = v.get("capture.calls_only").and_then(Value::as_bool).unwrap_or(true);
                egui::ComboBox::from_id_salt("rec").selected_text(if calls { RECORD_CALLS } else { RECORD_ALL }).width(160.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut calls, true, RECORD_CALLS);
                    ui.selectable_value(&mut calls, false, RECORD_ALL);
                });
                v.insert("capture.calls_only".into(), Value::Boolean(calls));
            });
            let wm: Vec<(&str, &str)> = WHISPER_MODELS.iter().map(|m| (*m, *m)).collect();
            choice(ui, v, "whisper.model", "Whisper model", &wm, true);
            choice(ui, v, "whisper.backend", "Run on", &[("gpu", "Intel GPU (OpenVINO)"), ("cpu", "CPU (whisper.cpp)")], false);
            ui.horizontal(|ui| {
                ui.add_space(218.0);
                ui.label(RichText::new("takes effect after restart").size(11.0).color(p.muted));
            });
            ui.horizontal(|ui| {
                ui.add_sized([210.0, 22.0], Label::new("Speaker match threshold"));
                let mut t = v.get("speakers.threshold").and_then(|x| x.as_float().or(x.as_integer().map(|i| i as f64))).unwrap_or(0.75);
                if ui.add(egui::DragValue::new(&mut t).range(0.5..=0.95).speed(0.01).fixed_decimals(2)).changed() {
                    v.insert("speakers.threshold".into(), Value::Float((t * 100.0).round() / 100.0));
                }
            });

            section(ui, "Screenshots");
            let screens = crate::screenshots::monitor_choices();
            let screens: Vec<(&str, &str)> = screens.iter().map(|(k, l)| (k.as_str(), l.as_str())).collect();
            if let Some(Value::Integer(n)) = v.get("screenshots.monitor").cloned() {
                v.insert("screenshots.monitor".into(), Value::String(n.to_string())); // a number written without quotes
            }
            choice(ui, v, "screenshots.monitor", "Screen to capture", &screens, false);
            toggle(ui, v, "screenshots.auto", "Auto screenshot while recording when the screen changes");
            ui.horizontal(|ui| {
                ui.add_sized([210.0, 22.0], Label::new("Auto: screen change needed (%)"));
                let mut pct = v.get("screenshots.change_percent").and_then(|x| x.as_integer().or(x.as_float().map(|f| f as i64))).unwrap_or(10);
                if ui.add(egui::DragValue::new(&mut pct).range(1..=100).suffix(" %")).changed() {
                    v.insert("screenshots.change_percent".into(), Value::Integer(pct));
                }
            });

            section(ui, "Appearance");
            choice(ui, v, "ui.theme", "Theme", &[("system", "Follow Windows"), ("light", "Light"), ("dark", "Dark")], false);

            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if accent_button(ui, "Save", p, true).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    close = true;
                }
            });
        });
        if !(save || close) {
            return;
        }
        let d = self.settings.take().unwrap();
        if !save {
            return;
        }
        let changed: Vec<(String, Value)> =
            d.values.into_iter().filter(|(k, val)| d.original.get(k) != Some(val)).collect();
        if changed.is_empty() {
            return;
        }
        // Mirror the saved settings in the window's own controls.
        for (k, val) in &changed {
            match k.as_str() {
                "summary.enabled" => self.claude_on = val.as_bool().unwrap_or(false),
                "answers.enabled" => {
                    self.answers_on = val.as_bool().unwrap_or(false);
                    self.answers_visible = self.answers_on;
                }
                "whisper.language" => self.language = val.as_str().unwrap_or("").into(),
                "ui.theme" => {
                    self.theme_mode = val.as_str().unwrap_or("system").into();
                    self.apply_theme(ctx);
                }
                _ => {}
            }
        }
        self.core.apply_settings(changed);
    }

    fn summary_windows(&mut self, ctx: &egui::Context, p: &Palette) {
        let size = self.px();
        for s in &mut self.summaries {
            let builder = ViewportBuilder::default().with_title(&s.title).with_inner_size([680.0, 560.0]);
            ctx.show_viewport_immediate(ViewportId::from_hash_of(("summary", s.id)), builder, |ui, _| {
                if ui.input(|i| i.viewport().close_requested()) {
                    s.open = false;
                }
                egui::Panel::bottom("buttons").show(ui, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(s.text.clone());
                        }
                        if let Some(path) = &s.path {
                            if ui.button("Open file").clicked() {
                                crate::shell_open(path);
                            }
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if accent_button(ui, "Close", p, true).clicked() {
                                s.open = false;
                            }
                        });
                    });
                    ui.add_space(4.0);
                });
                egui::CentralPanel::default().frame(egui::Frame::NONE.fill(p.bg).inner_margin(Margin::symmetric(22, 14))).show(ui, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| markdown::show(ui, &s.text, size, p.text));
                });
            });
        }
        self.summaries.retain(|s| s.open);
    }
}

enum EntryAction {
    Rename(usize, String, String),
    CommitRename,
    CancelRename,
    Ask(String, String, String),
}

impl eframe::App for LiveApp {
    /// Runs even while the window is hidden, so tray clicks and new lines are always handled.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_tray(ctx);
        self.handle_events(ctx);
        if let Some((tray, _)) = &mut self.tray {
            tray.update(&self.core);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500)); // status line, tray state
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.core.cfg.lock().unwrap().bool("ui", "close_to_tray") {
                // Closing just hides the window; Quit is in the tray menu. Say so, or it looks like it quit.
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                crate::tray::notify(
                    "Transcribe is still running",
                    "It keeps running in the system tray. Double-click the tray icon to open the window, or right-click it and choose Quit.",
                );
            } else {
                self.quitting = true;
                crate::tray::quit(&self.core);
            }
        }
        let dark = ui.style().visuals.dark_mode;
        let p = theme::palette(dark);
        egui::Panel::top("header").frame(egui::Frame::side_top_panel(ui.style()).inner_margin(Margin { left: 14, right: 10, top: 10, bottom: 8 })).show(ui, |ui| {
            self.header(ui, p, dark);
        });
        egui::Panel::top("toolbar").frame(egui::Frame::side_top_panel(ui.style()).inner_margin(Margin::symmetric(12, 8))).show(ui, |ui| {
            self.toolbar(ui);
        });
        egui::Panel::bottom("bottom").frame(egui::Frame::side_top_panel(ui.style()).inner_margin(Margin { left: 14, right: 12, top: 8, bottom: 10 })).show(ui, |ui| {
            self.bottom_bar(ui, p);
        });
        if self.answers_visible {
            egui::Panel::right("answers").resizable(true).default_size(320.0).min_size(220.0).show(ui, |ui| {
                self.answers_panel(ui, p);
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            self.transcript(ui, p);
        });
        self.settings_dialog(&ctx, p);
        self.summary_windows(&ctx, p);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn find_all_is_case_insensitive() {
        assert_eq!(super::find_all("Hallo hallo", "HALLO"), vec![(0, 5), (6, 11)]);
        assert_eq!(super::find_all("Straße", "ße"), vec![(4, 7)]);
        assert!(super::find_all("abc", "").is_empty());
    }
}
