//! Tray app: transcribes the microphone ([Me]) and the speaker output ([Them]) in the background.
//! Start with `--console` to see the log live.
#![windows_subsystem = "windows"]

mod answers;
mod app;
mod callwatch;
mod capture;
mod claude;
mod config;
mod engine;
mod logger;
mod markdown;
mod models;
mod openvino;
mod screenshots;
mod segmenter;
mod speakers;
mod theme;
mod transcriber;
mod tray;
mod ui;
mod whisper;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::app::Core;
use crate::config::Config;

/// Opens a file or folder like a double click in Explorer.
pub fn shell_open(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
    let (verb, file) = (wide("open".as_ref()), wide(path.as_os_str()));
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            AllocConsole();
        }
    }
}

/// Only one instance at a time (same mutex name as the Python version, so they never run together).
fn already_running() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\transcribe-tray-app".encode_utf16().chain([0]).collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        let _ = handle; // never closed: held until the process exits
        GetLastError() == ERROR_ALREADY_EXISTS
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("spawn thread")
}

/// `--transcribe <file.wav>`: runs Whisper and voice recognition on a 16 kHz mono 16-bit WAV and prints the
/// lines, to check the installation without a call.
/// Samples of a 16 kHz mono 16-bit WAV file.
fn read_wav(path: &str) -> anyhow::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    let data = bytes.windows(4).position(|w| w == b"data").ok_or_else(|| anyhow::anyhow!("not a WAV file"))? + 8;
    Ok(bytes[data..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect())
}

fn transcribe_file(path: &str) -> anyhow::Result<()> {
    let audio = read_wav(path)?;
    let cfg = Config::load();
    let started = Instant::now();
    let mut model =
        engine::Engine::load(&cfg.str("whisper", "model"), &cfg.str("whisper", "backend"), cfg.i64("whisper", "cpu_threads") as i32, cfg.i64("whisper", "beam_size") as i32)?.0;
    let language = cfg.str("whisper", "language");
    println!("model loaded in {:.1} s", started.elapsed().as_secs_f32());
    let voices = speakers::Embedder::load()?;
    let mut parts = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let (p, language) = model.transcribe(&audio, (!language.is_empty()).then_some(language.as_str()))?;
        println!("transcribed {:.1} s of audio in {:.1} s, language {language:?}", audio.len() as f32 / 16000.0, started.elapsed().as_secs_f32());
        parts = p;
    }
    for (start, end, text) in parts {
        let a = (start * 16000.0) as usize;
        let b = ((end * 16000.0) as usize).min(audio.len());
        let voice = if b > a + 8000 { voices.embed(&audio[a..b]).map(|e| format!("{:.3}", e[0])).unwrap_or_default() } else { String::new() };
        println!("[{start:5.1}-{end:5.1}] {text}  (voiceprint[0]={voice})");
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let console = args.iter().any(|a| a == "--console" || a == "--transcribe");
    if console {
        attach_console();
    }
    logger::init(console);
    if let Some(i) = args.iter().position(|a| a == "--transcribe") {
        if let Err(e) = transcribe_file(args.get(i + 1).map(String::as_str).unwrap_or("")) {
            println!("error: {e:#}");
        }
        return;
    }
    if already_running() {
        log::warn!("Already running");
        return;
    }
    std::panic::set_hook(Box::new(|info| log::error!("Fatal error: {info}")));

    let cfg = Config::load();
    let show_on_start = cfg.bool("ui", "show_window_on_start");
    let sources = Core::capture_sources(&cfg);
    let (capture, capture_commands) = capture::channel();
    let (chunk_tx, chunk_rx) = crossbeam_channel::unbounded();
    let (core, ui_events) = Core::new(cfg, capture.clone(), chunk_rx.clone());
    let core = Arc::new(core);

    // Threads joined on shutdown, in this order, each with how long it may take to finish.
    let mut senders = HashMap::new();
    let mut segmenters = Vec::new();
    for source in sources {
        let (tx, rx) = crossbeam_channel::unbounded();
        senders.insert(source, tx);
        let (core, out) = (core.clone(), chunk_tx.clone());
        segmenters.push(spawn(&format!("segmenter-{}", source.id()), move || segmenter::run(core, source, rx, out)));
    }
    // `--feed a.wav b.wav …`: once the model is loaded, push WAV files (16 kHz mono) through the pipeline as if they
    // came from the speakers, to test transcription, voices and the window without a call.
    if let Some(i) = args.iter().position(|a| a == "--feed") {
        let files: Vec<String> = args[i + 1..].iter().take_while(|a| !a.starts_with("--")).cloned().collect();
        let (core, tx) = (core.clone(), chunk_tx.clone());
        spawn("feed", move || {
            while core.state() == app::State::Loading {
                std::thread::sleep(Duration::from_millis(200));
            }
            // Laid out back to back with a 1 s pause, ending now, like a real conversation.
            let clips: Vec<Vec<f32>> = files
                .iter()
                .filter_map(|f| read_wav(f).map_err(|e| log::error!("--feed {f}: {e:#}")).ok())
                .collect();
            let total_ms: i64 = clips.iter().map(|a| a.len() as i64 / 16 + 1000).sum();
            let mut start = chrono::Local::now() - chrono::TimeDelta::milliseconds(total_ms);
            for (n, audio) in clips.into_iter().enumerate() {
                let len_ms = audio.len() as i64 / 16;
                let _ = tx.send(segmenter::Chunk { source: segmenter::Source::Them, start, audio, chunk_id: 1_000_000 + n as u64 });
                start += chrono::TimeDelta::milliseconds(len_ms + 1000);
            }
        });
    }
    drop(chunk_tx);
    let transcriber = {
        let core = core.clone();
        spawn("transcriber", move || transcriber::run(core, chunk_rx))
    };
    let audio = {
        let core = core.clone();
        spawn("audio", move || capture::run(core, senders, capture_commands))
    };
    core.add_thread(audio, Duration::from_secs(2));
    for s in segmenters {
        core.add_thread(s, Duration::from_secs(2));
    }
    // Give the model a chance to finish what was already captured.
    core.add_thread(transcriber, Duration::from_secs(60));
    {
        let core = core.clone();
        spawn("call-watcher", move || callwatch::run(core));
    }
    {
        let core = core.clone();
        spawn("answers", move || answers::Answerer::run(core));
    }
    {
        let core = core.clone();
        spawn("screenshots", move || screenshots::run_auto(core));
    }

    let icon = egui::IconData { rgba: theme::mic_icon(64, [0x2f, 0x6f, 0xd6]), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Transcribe - live")
            .with_inner_size([780.0, 560.0])
            .with_min_inner_size([560.0, 360.0])
            .with_icon(Arc::new(icon))
            .with_visible(show_on_start),
        ..Default::default()
    };
    let ui_core = core.clone();
    let result = eframe::run_native(
        "Transcribe",
        options,
        Box::new(move |cc| Ok(Box::new(ui::LiveApp::new(cc, ui_core, ui_events)))),
    );
    if let Err(e) = result {
        log::error!("Window failed: {e}");
    }

    core.shutdown();
}
