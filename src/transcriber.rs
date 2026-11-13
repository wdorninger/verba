//! Runs Whisper over speech chunks, tells speakers apart and appends the text to the transcript file.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local, TimeDelta};
use crossbeam_channel::{Receiver, RecvTimeoutError};

use crate::app::{Core, UiEvent};
use crate::segmenter::{Chunk, RATE, Source};
use crate::engine::Engine;
use crate::whisper::{Part, Whisper};

/// Phrases Whisper tends to invent on near-silent audio.
const HALLUCINATIONS: &[&str] = &[
    "thank you.",
    "thanks for watching!",
    "thank you for watching.",
    "you",
    "untertitel im auftrag des zdf, 2017",
    "untertitel der amara.org-community",
    "vielen dank.",
    "danke.",
    "bis zum nächsten mal.",
];

/// Whisper's non-speech annotations, invented on noise or silence: "[INAUDIBLE]", "(chattering)", "(Pausa)",
/// "*music*", "♪ ♪", or nothing but punctuation.
fn is_annotation(text: &str) -> bool {
    let t = text.trim();
    let enclosed = |open: char, close: char| t.starts_with(open) && t.ends_with(close) && !t[1..].contains(open);
    enclosed('[', ']')
        || enclosed('(', ')')
        || (t.len() > 1 && t.starts_with('*') && t.ends_with('*'))
        || !t.chars().any(|c| c.is_alphanumeric())
        || t.chars().all(|c| c == '♪' || c == '♫' || !c.is_alphanumeric())
}

pub struct TranscriptWriter {
    directory: PathBuf,
    inner: Mutex<(PathBuf, String)>, // current file, header written with its first line
}

impl TranscriptWriter {
    pub fn new(directory: PathBuf) -> TranscriptWriter {
        let _ = std::fs::create_dir_all(&directory);
        let w = TranscriptWriter { directory, inner: Mutex::new((PathBuf::new(), String::new())) };
        w.new_file("");
        w
    }

    pub fn directory(&self) -> &PathBuf {
        &self.directory
    }

    pub fn path(&self) -> PathBuf {
        self.inner.lock().unwrap().0.clone()
    }

    /// The current transcript's own folder (screenshots and the summary go there too), created on demand.
    pub fn folder(&self) -> std::io::Result<PathBuf> {
        let path = self.path();
        let folder = path.parent().map(PathBuf::from).unwrap_or_else(|| self.directory.clone());
        std::fs::create_dir_all(&folder)?;
        Ok(folder)
    }

    /// Starts a fresh transcript, e.g. one per call or from the "New transcript" button. Each transcript gets
    /// its own folder: `<directory>\<date_time>[_App]\<date_time>[_App].txt`.
    pub fn new_file(&self, suffix: &str) {
        let now = Local::now();
        let mut name = now.format("%Y-%m-%d_%H-%M-%S").to_string();
        if !suffix.is_empty() {
            let clean: String =
                suffix.chars().filter(|c| c.is_alphanumeric() || "-_ ".contains(*c)).collect::<String>();
            name.push('_');
            name.push_str(&clean.trim().replace(' ', "-"));
        }
        let path = self.directory.join(&name).join(format!("{name}.txt"));
        // Folder and file are only created with the first line, so starts without speech leave nothing behind.
        let header = format!(
            "# Transcript started {}{}\n",
            now.format("%Y-%m-%d %H:%M:%S"),
            if suffix.is_empty() { String::new() } else { format!(" ({suffix})") }
        );
        log::info!("Next transcript file: {}", path.display());
        *self.inner.lock().unwrap() = (path, header);
    }

    pub fn append(&self, line: &str) {
        let inner = self.inner.lock().unwrap();
        let new = !inner.0.exists();
        let result = inner
            .0
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(&inner.0))
            .and_then(|mut f| f.write_all(format!("{}{line}\n", if new { inner.1.as_str() } else { "" }).as_bytes()));
        if let Err(e) = result {
            log::error!("Could not write {}: {e}", inner.0.display());
        }
    }

    /// Updates lines already written in this session after a speaker was named.
    pub fn rename(&self, old_label: &str, new_label: &str) -> std::io::Result<()> {
        let inner = self.inner.lock().unwrap();
        if !inner.0.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(&inner.0)?;
        std::fs::write(&inner.0, text.replace(&format!("] [{old_label}] "), &format!("] [{new_label}] ")))
    }
}

struct Transcriber {
    core: Arc<Core>,
    model: Engine,
    fast_model: Option<Whisper>,
    catching_up: bool,
    recent_them: VecDeque<(DateTime<Local>, DateTime<Local>, String)>,
    finalized: HashMap<Source, u64>,
    language: HashMap<Source, String>,
    since_detect: HashMap<Source, i64>,
}

pub fn run(core: Arc<Core>, input: Receiver<Chunk>) {
    let (model_name, threads, beam) = core.whisper_settings();
    let backend = core.cfg.lock().unwrap().str("whisper", "backend");
    log::info!("Loading Whisper model '{model_name}' (first run downloads it)...");
    crate::models::set_progress(format!("Loading speech model '{model_name}'…"), None);
    let model = match Engine::load(&model_name, &backend, threads, beam) {
        Ok((m, info)) => {
            let fallback = info.fallback.clone();
            core.set_engine(info);
            if let Some(e) = fallback {
                core.ui.send(UiEvent::Note(format!(
                    "GPU speech recognition unavailable, using the CPU (slower). Reason: {e}"
                )));
            }
            m
        }
        Err(e) => {
            log::error!("Could not load the Whisper model: {e:#}");
            core.set_error(format!("Whisper model failed to load: {e}"));
            return;
        }
    };
    log::info!("Model loaded");
    if let Some(speakers) = &core.speakers {
        crate::models::set_progress("Loading voice recognition…", None);
        if let Err(e) = speakers.load_model() {
            log::error!("Speaker recognition unavailable: {e:#}");
        }
    }
    crate::models::clear_progress();
    core.on_ready();

    let mut t = Transcriber {
        core: core.clone(),
        model,
        fast_model: None,
        catching_up: false,
        recent_them: VecDeque::with_capacity(20),
        finalized: HashMap::new(),
        language: HashMap::new(),
        since_detect: HashMap::new(),
    };
    while !(core.stop.load(Ordering::Relaxed) && input.is_empty()) {
        let first = match input.recv_timeout(Duration::from_millis(100)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => {
                core.set_delay(0.0);
                // Idle: spend the time on a live preview of speech that is still going on.
                let partial = if t.catching_up { None } else { core.partials.as_ref().and_then(|p| p.pop()) };
                if let Some(p) = partial {
                    if p.chunk_id > t.finalized.get(&p.source).copied().unwrap_or(0) {
                        t.preview(&p);
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut batch = vec![first];
        batch.extend(input.try_iter());
        // Speaker chunks first, so an echoed copy picked up by the mic can be recognised and dropped.
        batch.sort_by_key(|c| (c.source != Source::Them, c.start));
        for chunk in batch {
            t.handle(chunk);
        }
    }
}

impl Transcriber {
    fn transcribe(&mut self, audio: &[f32], language: Option<&str>, fast: bool) -> (Vec<Part>, Option<String>) {
        let result = match self.fast_model.as_mut().filter(|_| fast) {
            Some(m) => m.transcribe(audio, language),
            None => self.model.transcribe(audio, language),
        };
        match result {
            Ok((mut parts, detected)) => {
                parts.retain(|p| !is_annotation(&p.2));
                let joined = parts.iter().map(|p| p.2.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
                if HALLUCINATIONS.contains(&joined.as_str()) { (Vec::new(), detected) } else { (parts, detected) }
            }
            Err(e) => {
                log::error!("Transcription failed: {e:#}");
                (Vec::new(), None)
            }
        }
    }

    /// Falls back to the fast 'base' model while transcription lags far behind the speech.
    fn pick_fast(&mut self, chunk: &Chunk) -> bool {
        let delay = (Local::now() - chunk.end()).as_seconds_f64();
        self.core.set_delay(delay);
        if self.model.is_gpu() {
            return false; // the GPU is already faster than CPU 'base', so falling back wouldn't help
        }
        let (catchup, model) = {
            let c = self.core.cfg.lock().unwrap();
            (c.f64("whisper", "catchup_seconds"), c.str("whisper", "model"))
        };
        if !self.catching_up && catchup > 0.0 && delay > catchup && model != "base" && model != "tiny" {
            log::info!("{delay:.0} s behind: switching to fast model until caught up");
            if self.fast_model.is_none() {
                let (_, threads, beam) = self.core.whisper_settings();
                match Whisper::load("base-q8_0", threads, beam) {
                    Ok(m) => self.fast_model = Some(m),
                    Err(e) => log::error!("Could not load the fast model: {e:#}"),
                }
            }
            self.catching_up = self.fast_model.is_some();
        } else if self.catching_up && delay < 5.0 {
            log::info!("Caught up: back to '{model}'");
            self.catching_up = false;
        }
        self.core.set_catching_up(self.catching_up);
        self.catching_up
    }

    fn preview(&mut self, chunk: &Chunk) {
        let language = self.configured_language().or_else(|| self.language.get(&chunk.source).cloned());
        let (parts, _) = self.transcribe(&chunk.audio, language.as_deref(), false);
        let text = parts.iter().map(|p| p.2.as_str()).collect::<Vec<_>>().join(" ");
        // The final version may have arrived while this preview was being computed.
        if !text.is_empty() && chunk.chunk_id > self.finalized.get(&chunk.source).copied().unwrap_or(0) {
            self.core.ui.send(UiEvent::Partial { label: chunk.source.label().into(), text });
        }
    }

    fn configured_language(&self) -> Option<String> {
        let l = self.core.cfg.lock().unwrap().str("whisper", "language");
        if l.is_empty() { None } else { Some(l) }
    }

    fn handle(&mut self, chunk: Chunk) {
        self.finalized.insert(chunk.source, chunk.chunk_id);
        if self.core.partials.is_some() {
            self.core.ui.send(UiEvent::Partial { label: chunk.source.label().into(), text: String::new() });
        }
        if chunk.audio.is_empty() {
            return;
        }
        let configured = self.configured_language();
        let redetect_every = self.core.cfg.lock().unwrap().i64("whisper", "redetect_every").max(1);
        let mut language = configured.clone();
        if language.is_none() && self.since_detect.get(&chunk.source).copied().unwrap_or(redetect_every) < redetect_every
        {
            // Language detection costs extra, so only re-check now and then.
            language = self.language.get(&chunk.source).cloned();
            *self.since_detect.get_mut(&chunk.source).unwrap() += 1;
        }
        let fast = self.pick_fast(&chunk);
        let (parts, detected) = self.transcribe(&chunk.audio, language.as_deref(), fast);
        if language.is_none() && configured.is_none() {
            if let Some(d) = detected {
                self.language.insert(chunk.source, d);
                self.since_detect.insert(chunk.source, 1); // counts the detecting chunk itself
            }
        }
        if parts.is_empty() {
            return;
        }
        let text = parts.iter().map(|p| p.2.as_str()).collect::<Vec<_>>().join(" ");
        let echo_dedupe = self.core.cfg.lock().unwrap().bool("output", "echo_dedupe");
        if chunk.source == Source::Them {
            if self.recent_them.len() == 20 {
                self.recent_them.pop_front();
            }
            self.recent_them.push_back((chunk.start, chunk.end(), text.clone()));
        } else if echo_dedupe && self.is_echo(&chunk, &text) {
            log::debug!("Dropped echo: {text}");
            return;
        }
        let core = self.core.clone();
        let speakers = match &core.speakers {
            Some(s) if core.speaker_sources.contains(&chunk.source) => s,
            _ => {
                core.write_line(chunk.start, chunk.source.id(), chunk.source.label(), &text, chunk.source);
                return;
            }
        };

        // A chunk can hold several voices when people talk without a pause, so identify each
        // sentence segment on its own and start a new line whenever the voice changes.
        let mut whos: Vec<Option<(String, String)>> = parts
            .iter()
            .map(|(start, end, _)| {
                let a = ((*start * RATE as f32) as usize).min(chunk.audio.len());
                let b = ((*end * RATE as f32) as usize).clamp(a, chunk.audio.len());
                speakers.identify(&chunk.audio[a..b], chunk.source).unwrap_or_else(|e| {
                    log::error!("Speaker identification failed: {e:#}");
                    None
                })
            })
            .collect();
        // An unclear short sentence ("Thanks.", "Great.") usually opens the next speaker's turn,
        // so take the following sentence's voice, else the previous one, else whoever spoke last.
        let fallback = (chunk.source.id().to_string(), chunk.source.label().to_string());
        for i in 0..whos.len() {
            if whos[i].is_none() {
                let after = whos[i + 1..].iter().flatten().next().cloned();
                let before = whos[..i].iter().rev().flatten().next().cloned();
                whos[i] = Some(
                    after.or(before).or_else(|| speakers.last_recent(chunk.source)).unwrap_or_else(|| fallback.clone()),
                );
            }
        }
        let mut turns: Vec<(f32, String, String, Vec<&str>)> = Vec::new();
        for ((start, _, seg_text), who) in parts.iter().zip(whos) {
            let (id, label) = who.unwrap();
            match turns.last_mut() {
                Some(t) if t.1 == id => t.3.push(seg_text),
                _ => turns.push((*start, id, label, vec![seg_text])),
            }
        }
        for (start, id, label, texts) in turns {
            let at = chunk.start + TimeDelta::milliseconds((start * 1000.0) as i64);
            core.write_line(at, &id, &label, &texts.join(" "), chunk.source);
        }
    }

    fn is_echo(&self, chunk: &Chunk, text: &str) -> bool {
        let slack = TimeDelta::seconds(2);
        let text = text.to_lowercase();
        self.recent_them.iter().any(|(start, end, them)| {
            chunk.start <= *end + slack
                && *start - slack <= chunk.end()
                && similar::TextDiff::from_chars(text.as_str(), them.to_lowercase().as_str()).ratio() > 0.6
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{TranscriptWriter, is_annotation};

    #[test]
    fn each_transcript_gets_its_own_folder() {
        let dir = std::env::temp_dir().join(format!("transcribe-writer-test-{}", std::process::id()));
        let w = TranscriptWriter::new(dir.clone());
        let first = w.path();
        let folder = first.parent().unwrap().to_path_buf();
        assert_eq!(folder.parent().unwrap(), dir);
        assert_eq!(folder.file_name().unwrap(), first.file_stem().unwrap());
        assert!(!folder.exists(), "nothing is created before the first line");
        w.append("[10:00:00] [Hanna] Hello");
        assert!(first.exists());
        assert_eq!(w.folder().unwrap(), folder);
        std::thread::sleep(std::time::Duration::from_millis(1100)); // names are per second
        w.new_file("Teams");
        assert_ne!(w.path().parent(), first.parent());
        assert!(w.path().to_string_lossy().ends_with("_Teams.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drops_non_speech_annotations() {
        for t in ["[INAUDIBLE]", "(chattering)", "(Pausa)", "[ Inaudible ]", "*music*", "♪ ♪", "...", " - "] {
            assert!(is_annotation(t), "{t}");
        }
        for t in ["Can you hear me?", "(laughs) That was funny.", "Yes [Peter] said so", "OK"] {
            assert!(!is_annotation(t), "{t}");
        }
    }
}
