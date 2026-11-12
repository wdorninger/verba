//! System tray icon and menu. Lives on the main (UI) thread; clicks arrive as `TrayAction`s.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{Core, State};

#[derive(Clone, Copy, Debug)]
pub enum TrayAction {
    Show,
    Pause,
    CallsOnly,
    Summarize,
    Screenshot,
    OpenTranscript,
    OpenFolder,
    RestartAudio,
    Quit,
}

thread_local! {
    /// The tray icon lives on the main thread; kept here so Quit can remove it before the process exits.
    static ICON: std::cell::RefCell<Option<TrayIcon>> = const { std::cell::RefCell::new(None) };
}

/// Quit: remove the tray icon, shut down cleanly and exit. Runs straight from the tray menu handler
/// (main thread, Windows message loop), so it works even while the window is hidden.
pub fn quit(core: &Core) -> ! {
    ICON.with(|i| drop(i.borrow_mut().take()));
    core.shutdown();
    std::process::exit(0);
}

pub struct Tray {
    status: MenuItem,
    pause: MenuItem,
    calls_only: CheckMenuItem,
    summarize: MenuItem,
    last: Option<(State, String, bool, bool, bool)>,
}

fn state_color(state: State) -> [u8; 3] {
    match state {
        State::Loading => [230, 160, 30],
        State::Recording => [40, 180, 70],
        State::Paused => [140, 140, 140],
        State::Waiting => [90, 130, 190],
        State::Error => [220, 60, 60],
    }
}

fn icon(state: State) -> Option<Icon> {
    Icon::from_rgba(crate::theme::mic_icon(32, state_color(state)), 32, 32).ok()
}

impl Tray {
    pub fn new(ctx: egui::Context, core: Arc<Core>) -> anyhow::Result<(Tray, Receiver<TrayAction>)> {
        let (tx, rx) = crossbeam_channel::unbounded::<TrayAction>();
        let status = MenuItem::new("Loading model…", false, None);
        let show = MenuItem::new("Show live window", true, None);
        let pause = MenuItem::new("Pause", true, None);
        let calls_only = CheckMenuItem::new("Only during calls", true, false, None);
        let summarize = MenuItem::new("Summarize transcript (Claude)", false, None);
        let shot = MenuItem::new("Take screenshot", true, None);
        let open = MenuItem::new("Open transcript", true, None);
        let folder = MenuItem::new("Open transcripts folder", true, None);
        let restart = MenuItem::new("Restart audio devices", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &show,
            &pause,
            &calls_only,
            &summarize,
            &shot,
            &open,
            &folder,
            &restart,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;
        let ids = [
            (show.id().clone(), TrayAction::Show),
            (pause.id().clone(), TrayAction::Pause),
            (calls_only.id().clone(), TrayAction::CallsOnly),
            (summarize.id().clone(), TrayAction::Summarize),
            (shot.id().clone(), TrayAction::Screenshot),
            (open.id().clone(), TrayAction::OpenTranscript),
            (folder.id().clone(), TrayAction::OpenFolder),
            (restart.id().clone(), TrayAction::RestartAudio),
            (quit.id().clone(), TrayAction::Quit),
        ];
        let send = |tx: &Sender<TrayAction>, ctx: &egui::Context, a| {
            let _ = tx.send(a);
            ctx.request_repaint();
        };
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                match ids.iter().find(|(id, _)| *id == e.id) {
                    Some((_, TrayAction::Quit)) => self::quit(&core),
                    Some((_, a)) => send(&tx, &ctx, *a),
                    None => {}
                }
            }));
        }
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = e {
                send(&tx, &ctx, TrayAction::Show);
            }
        }));
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Transcribe: loading model...")
            .with_icon(icon(State::Loading).expect("tray icon"))
            .build()?;
        ICON.with(|i| *i.borrow_mut() = Some(icon));
        Ok((Tray { status, pause, calls_only, summarize, last: None }, rx))
    }

    /// Mirrors the app state in the icon colour, tooltip and menu (cheap when nothing changed).
    pub fn update(&mut self, core: &Core) {
        let state = core.state();
        let status = match core.engine() {
            Some(e) => format!("{} · {}", core.status(), e.label),
            None => core.status(),
        };
        let paused = core.manual_pause.load(std::sync::atomic::Ordering::Relaxed);
        let calls_only = core.calls_only.load(std::sync::atomic::Ordering::Relaxed);
        let claude = core.cfg.lock().unwrap().bool("summary", "enabled");
        let now = (state, status.clone(), paused, calls_only, claude);
        if self.last.as_ref() == Some(&now) {
            return;
        }
        if self.last.as_ref().map(|l| l.0) != Some(state) {
            ICON.with(|i| i.borrow().as_ref().map(|t| t.set_icon(icon(state))));
        }
        let tip: String = format!("Transcribe: {status}").chars().take(127).collect();
        ICON.with(|i| i.borrow().as_ref().map(|t| t.set_tooltip(Some(tip))));
        self.status.set_text(&status);
        self.pause.set_text(if paused { "Resume" } else { "Pause" });
        self.calls_only.set_checked(calls_only);
        self.summarize.set_enabled(claude);
        self.last = Some(now);
    }
}
