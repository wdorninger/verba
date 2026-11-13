//! Shared application state and actions (the non-UI part of the tray app).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use crossbeam_channel::{Receiver, Sender};
use toml::Value;

use crate::answers::Answerer;
use crate::capture::CaptureHandle;
use crate::config::{Config, resolve, root, save_setting};
use crate::segmenter::{Chunk, LatestPartials, Source};
use crate::speakers::SpeakerRegistry;
use crate::transcriber::TranscriptWriter;

/// Everything the window shows comes through these events.
pub enum UiEvent {
    Line { time: String, speaker_id: String, label: String, text: String },
    Partial { label: String, text: String },
    Note(String),
    Screenshot { time: String, path: PathBuf },
    Summary { title: String, text: String, path: Option<PathBuf> },
    Summarizing(bool),
    AnswerPending { qid: u64, time: String, label: String, question: String },
    AnswerDone { qid: u64, text: String },
    AnswerSkipped(u64),
    AnswerError { qid: u64, message: String },
}

/// Sends events to the window and wakes it up (once the window exists).
pub struct UiSink {
    tx: Sender<UiEvent>,
    pub ctx: OnceLock<eframe::egui::Context>,
}

impl UiSink {
    pub fn send(&self, e: UiEvent) {
        let _ = self.tx.send(e);
        self.repaint();
    }
    pub fn repaint(&self) {
        if let Some(c) = self.ctx.get() {
            c.request_repaint();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Loading,
    Recording,
    Waiting,
    Paused,
    Error,
}

pub const SETTING_KEYS: &[&str] = &[
    "summary.enabled",
    "summary.model",
    "summary.auto_after_call",
    "answers.enabled",
    "answers.model",
    "answers.web_search",
    "answers.sources",
    "answers.context_lines",
    "whisper.language",
    "whisper.model",
    "whisper.backend",
    "capture.calls_only",
    "speakers.threshold",
    "screenshots.monitor",
    "screenshots.auto",
    "screenshots.change_percent",
    "ui.theme",
];

pub struct Core {
    pub cfg: Mutex<Config>,
    pub stop: AtomicBool,
    /// What capture obeys: manual pause, or no call in calls-only mode.
    pub paused: Arc<AtomicBool>,
    pub manual_pause: AtomicBool,
    pub calls_only: AtomicBool,
    pub call_app: Mutex<Option<String>>,
    loaded: AtomicBool,
    error: Mutex<Option<String>>,
    delay: AtomicU32,
    catching_up: AtomicBool,
    engine: Mutex<Option<crate::engine::EngineInfo>>,
    pub partials: Option<Arc<LatestPartials>>,
    pub speakers: Option<SpeakerRegistry>,
    pub speaker_sources: Vec<Source>,
    pub writer: TranscriptWriter,
    pub answerer: Answerer,
    pub ui: UiSink,
    pub capture: CaptureHandle,
    chunks: Receiver<Chunk>,
    threads: Mutex<Vec<(JoinHandle<()>, Duration)>>,
    shutdown_once: Once,
}

impl Core {
    pub fn new(mut cfg: Config, capture: CaptureHandle, chunks: Receiver<Chunk>) -> (Core, Receiver<UiEvent>) {
        // Claude (summaries and answers) always starts switched off; nothing is sent until the user turns it on.
        cfg.set("summary", "enabled", Value::Boolean(false));
        // Recording always starts in "All audio" mode; "Only calls" can be chosen per session.
        cfg.set("capture", "calls_only", Value::Boolean(false));
        let sources = Core::capture_sources(&cfg);
        let speaker_sources: Vec<Source> = if cfg.bool("speakers", "enabled") {
            cfg.str_list("speakers", "sources")
                .iter()
                .filter_map(|s| Source::from_id(s))
                .filter(|s| sources.contains(s))
                .collect()
        } else {
            Vec::new()
        };
        let speakers = (!speaker_sources.is_empty()).then(|| {
            SpeakerRegistry::new(
                root().join("speakers.json"),
                cfg.f64("speakers", "threshold"),
                cfg.f64("speakers", "min_seconds"),
            )
        });
        let (tx, rx) = crossbeam_channel::unbounded();
        let core = Core {
            stop: AtomicBool::new(false),
            paused: Arc::new(AtomicBool::new(true)),
            manual_pause: AtomicBool::new(false),
            calls_only: AtomicBool::new(cfg.bool("capture", "calls_only")),
            call_app: Mutex::new(None),
            loaded: AtomicBool::new(false),
            error: Mutex::new(None),
            delay: AtomicU32::new(0),
            catching_up: AtomicBool::new(false),
            engine: Mutex::new(None),
            partials: cfg.bool("ui", "live_preview").then(|| Arc::new(LatestPartials::default())),
            speakers,
            speaker_sources,
            writer: TranscriptWriter::new(resolve(&cfg.str("output", "directory"))),
            answerer: Answerer::default(),
            ui: UiSink { tx, ctx: OnceLock::new() },
            capture,
            chunks,
            threads: Mutex::new(Vec::new()),
            shutdown_once: Once::new(),
            cfg: Mutex::new(cfg),
        };
        core.apply_pause();
        (core, rx)
    }

    pub fn capture_sources(cfg: &Config) -> Vec<Source> {
        let mut s = Vec::new();
        if cfg.bool("capture", "mic") {
            s.push(Source::Me);
        }
        if cfg.bool("capture", "speakers") {
            s.push(Source::Them);
        }
        s
    }

    pub fn whisper_settings(&self) -> (String, i32, i32) {
        let c = self.cfg.lock().unwrap();
        (c.str("whisper", "model"), c.i64("whisper", "cpu_threads") as i32, c.i64("whisper", "beam_size") as i32)
    }

    // --- state ---
    pub fn state(&self) -> State {
        if self.error.lock().unwrap().is_some() {
            State::Error
        } else if !self.loaded.load(Ordering::Relaxed) {
            State::Loading
        } else if self.manual_pause.load(Ordering::Relaxed) {
            State::Paused
        } else if self.calls_only.load(Ordering::Relaxed) && self.call_app.lock().unwrap().is_none() {
            State::Waiting
        } else {
            State::Recording
        }
    }

    pub fn status(&self) -> String {
        if let Some(e) = self.error.lock().unwrap().as_ref() {
            return e.clone();
        }
        let mut parts: Vec<String> = Vec::new();
        match self.state() {
            State::Loading => {
                return match crate::models::progress() {
                    Some((text, Some(f))) => format!("{text} ({:.0}%)", f * 100.0),
                    Some((text, None)) => text,
                    None => "Loading model…".into(),
                };
            }
            State::Paused => parts.push("Paused".into()),
            State::Waiting => parts.push("Waiting for a call…".into()),
            _ => {
                parts.push("Recording".into());
                if let Some(app) = self.call_app.lock().unwrap().as_ref() {
                    parts.push(format!("in call ({app})"));
                }
            }
        }
        let delay = self.delay();
        if delay >= 3.0 {
            parts.push(format!("delay {delay:.0} s"));
        }
        if self.catching_up.load(Ordering::Relaxed) {
            parts.push("fast model (catching up)".into());
        }
        parts.join(" · ")
    }

    pub fn delay(&self) -> f32 {
        f32::from_bits(self.delay.load(Ordering::Relaxed))
    }
    pub fn set_delay(&self, d: f64) {
        self.delay.store((d.max(0.0) as f32).to_bits(), Ordering::Relaxed);
    }
    pub fn set_engine(&self, info: crate::engine::EngineInfo) {
        *self.engine.lock().unwrap() = Some(info);
        self.ui.repaint();
    }
    pub fn engine(&self) -> Option<crate::engine::EngineInfo> {
        self.engine.lock().unwrap().clone()
    }
    pub fn set_catching_up(&self, on: bool) {
        self.catching_up.store(on, Ordering::Relaxed);
    }
    pub fn set_error(&self, e: String) {
        *self.error.lock().unwrap() = Some(e);
        self.ui.repaint();
    }

    pub fn on_ready(&self) {
        self.loaded.store(true, Ordering::Relaxed);
        self.apply_pause();
    }

    fn apply_pause(&self) {
        let pause = self.manual_pause.load(Ordering::Relaxed)
            || (self.calls_only.load(Ordering::Relaxed) && self.call_app.lock().unwrap().is_none());
        self.paused.store(pause, Ordering::Relaxed);
        self.ui.repaint();
    }

    fn save(&self, section: &str, key: &str, value: &Value) {
        if let Err(e) = save_setting(section, key, value) {
            log::error!("Could not save {section}.{key} to config.toml: {e:#}");
        }
    }

    /// Writes one transcript line: file, window and the question spotter.
    pub fn write_line(&self, start: DateTime<Local>, speaker_id: &str, label: &str, text: &str, source: Source) {
        // Use the voice's current name: a line identified just before a rename or merge gets the new one.
        let current = self.speakers.as_ref().and_then(|s| s.current(speaker_id));
        let (speaker_id, label) = match &current {
            Some((id, name)) => (id.as_str(), name.as_str()),
            None => (speaker_id, label),
        };
        let time = start.format("%H:%M:%S").to_string();
        let line = format!("[{time}] [{label}] {text}");
        log::info!("{line}");
        self.writer.append(&line);
        self.ui.send(UiEvent::Line {
            time: time.clone(),
            speaker_id: speaker_id.into(),
            label: label.into(),
            text: text.into(),
        });
        self.answerer.on_line(self, source, &time, label, text);
    }

    // --- actions ---
    pub fn toggle_pause(&self) {
        self.manual_pause.fetch_xor(true, Ordering::Relaxed);
        self.apply_pause();
    }

    pub fn set_calls_only(&self, calls_only: bool) {
        if self.calls_only.swap(calls_only, Ordering::Relaxed) == calls_only {
            return;
        }
        log::info!("Record: {}", if calls_only { "only during calls" } else { "all audio" });
        // Only for this session: the app always starts with "All audio".
        self.cfg.lock().unwrap().set("capture", "calls_only", Value::Boolean(calls_only));
        self.apply_pause();
    }

    pub fn on_call(self: &Arc<Self>, app: Option<String>) {
        let previous = std::mem::replace(&mut *self.call_app.lock().unwrap(), app.clone());
        let stamp = Local::now().format("%H:%M");
        match (&previous, &app) {
            (None, Some(app)) => {
                if self.calls_only.load(Ordering::Relaxed) {
                    self.writer.new_file(app);
                }
                self.ui.send(UiEvent::Note(format!("Call started ({app}) {stamp}")));
            }
            (Some(prev), None) => {
                self.ui.send(UiEvent::Note(format!("Call ended ({prev}) {stamp}")));
                let auto = {
                    let c = self.cfg.lock().unwrap();
                    c.bool("summary", "enabled") && c.bool("summary", "auto_after_call")
                };
                if self.calls_only.load(Ordering::Relaxed) && auto {
                    self.summarize(Some(self.writer.path()), true);
                }
            }
            _ => {}
        }
        self.apply_pause();
    }

    pub fn get_settings(&self) -> Vec<(String, Value)> {
        let c = self.cfg.lock().unwrap();
        SETTING_KEYS
            .iter()
            .map(|key| {
                let (section, name) = key.split_once('.').unwrap();
                let v = if *key == "capture.calls_only" {
                    Value::Boolean(self.calls_only.load(Ordering::Relaxed))
                } else {
                    c.get(section, name).cloned().unwrap_or(Value::String(String::new()))
                };
                (key.to_string(), v)
            })
            .collect()
    }

    /// Applies settings from the window live (where possible) and saves them to config.toml.
    pub fn apply_settings(&self, changed: Vec<(String, Value)>) {
        for (key, value) in changed {
            let (section, name) = key.split_once('.').unwrap();
            if key == "capture.calls_only" {
                self.set_calls_only(value.as_bool().unwrap_or(true)); // saves itself
                continue;
            }
            if key == "summary.enabled" {
                // Only for this session: Claude always starts switched off.
                self.cfg.lock().unwrap().set(section, name, value.clone());
                log::info!("Claude {}", if value.as_bool() == Some(true) { "enabled" } else { "disabled" });
                continue;
            }
            if key == "speakers.threshold" {
                if let (Some(s), Some(t)) = (&self.speakers, value.as_float()) {
                    s.set_threshold(t);
                }
            }
            self.cfg.lock().unwrap().set(section, name, value.clone());
            self.save(section, name, &value);
            log::info!("Setting {key} = {value}");
        }
        self.apply_pause();
    }

    pub fn summarize(self: &Arc<Self>, path: Option<PathBuf>, wait_for_backlog: bool) {
        let model = {
            let c = self.cfg.lock().unwrap();
            if !c.bool("summary", "enabled") {
                return;
            }
            c.str("summary", "model")
        };
        let path = path.unwrap_or_else(|| self.writer.path());
        self.ui.send(UiEvent::Summarizing(true));
        let core = self.clone();
        std::thread::Builder::new()
            .name("summary".into())
            .spawn(move || {
                if wait_for_backlog {
                    // Let the last sentences of the call reach the file first.
                    let deadline = Instant::now() + Duration::from_secs(180);
                    while Instant::now() < deadline && (!core.chunks.is_empty() || core.delay() > 0.0) {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let event = match crate::claude::summarize(&path, &model) {
                    Ok(out) => UiEvent::Summary {
                        title: format!("Summary – {name}"),
                        text: std::fs::read_to_string(&out).unwrap_or_default(),
                        path: Some(out),
                    },
                    Err(e) => {
                        log::error!("Summary failed: {e:#}");
                        UiEvent::Summary { title: "Summary failed".into(), text: format!("{e:#}"), path: None }
                    }
                };
                core.ui.send(event);
            })
            .expect("spawn summary thread");
    }

    pub fn rename_speaker(&self, speaker_id: &str, name: &str) -> Option<(String, String, String)> {
        let result = self.speakers.as_ref()?.rename(speaker_id, name)?;
        if let Err(e) = self.writer.rename(&result.0, &result.2) {
            log::error!("Could not update transcript file: {e}");
        }
        self.answerer.rename(&result.0, &result.2);
        Some(result)
    }

    /// "New transcript": later lines go to a new file in a new folder; Claude no longer sees the old lines.
    pub fn new_transcript(&self) {
        let suffix = self.call_app.lock().unwrap().clone().unwrap_or_default();
        self.writer.new_file(&suffix);
        self.answerer.reset_context();
        log::info!("New transcript started");
        self.ui.send(UiEvent::Note(format!("New transcript started {}", Local::now().format("%H:%M"))));
    }

    pub fn ask(&self, time: &str, label: &str, text: &str) {
        self.answerer.ask(self, time, label, text);
    }

    pub fn open_transcript(&self) {
        // Nothing written yet in this session/call: show the folder instead.
        let p = self.writer.path();
        crate::shell_open(if p.exists() { &p } else { self.writer.directory() });
    }

    pub fn open_folder(&self) {
        crate::shell_open(self.writer.directory());
    }
}

impl Core {
    pub fn add_thread(&self, handle: JoinHandle<()>, timeout: Duration) {
        self.threads.lock().unwrap().push((handle, timeout));
    }

    /// Stops capturing, keeps named voices and lets the model finish what was already captured.
    /// Safe to call more than once (Quit from the tray, then the end of main).
    pub fn shutdown(&self) {
        self.shutdown_once.call_once(|| {
            log::info!("Stopping...");
            self.stop.store(true, Ordering::Relaxed);
            self.capture.stop();
            if let Some(s) = &self.speakers {
                s.save(); // keeps the voice adaptation of named speakers
            }
            let threads = std::mem::take(&mut *self.threads.lock().unwrap());
            for (handle, timeout) in threads {
                let deadline = Instant::now() + timeout;
                while !handle.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            log::info!("Stopped");
        });
    }
}
