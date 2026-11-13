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
    NewTranscript,
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

thread_local! {
    /// The bubble currently shown (a tooltip window) and the timer that hides it, as raw values.
    static BUBBLE: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Shows the app's own balloon bubble pointing at the tray icon for a few seconds. It is a plain Windows
/// balloon tooltip, so it appears whatever the Windows notification settings (Do Not Disturb etc.) are.
/// Call from the main thread, where the tray icon and the message loop live.
pub fn notify(title: &str, text: &str) {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::Controls::{
        ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TOOLTIPS_CLASSW, TTF_ABSOLUTE, TTF_TRACK,
        TTI_INFO, TTM_ADDTOOLW, TTM_SETMAXTIPWIDTH, TTM_SETTITLEW, TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTS_ALWAYSTIP,
        TTS_BALLOON, TTS_CLOSE, TTS_NOPREFIX, TTTOOLINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, SPI_GETWORKAREA, SendMessageW, SetTimer, SystemParametersInfoW, WS_EX_TOPMOST,
        WS_POPUP,
    };
    let Some((owner, rect)) = ICON.with(|i| i.borrow().as_ref().map(|t| (t.window_handle(), t.rect()))) else { return };
    // Point at the middle of the tray icon's top edge; if it is hidden in the overflow, at the screen corner.
    let (x, y) = match rect {
        Some(r) if r.size.width > 0 => ((r.position.x + r.size.width as f64 / 2.0) as i32, r.position.y as i32 + 4),
        _ => {
            let mut work = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut RECT as _, 0) };
            (work.right - 40, work.bottom - 2)
        }
    };
    hide_bubble();
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (mut text_w, title_w) = (wide(text), wide(title));
    unsafe {
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
        InitCommonControlsEx(&icc);
        let style = WS_POPUP | TTS_BALLOON | TTS_NOPREFIX | TTS_ALWAYSTIP | TTS_CLOSE;
        let tip = CreateWindowExW(
            WS_EX_TOPMOST, TOOLTIPS_CLASSW, std::ptr::null(), style,
            CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT,
            owner as _, null_mut(), null_mut(), std::ptr::null(),
        );
        if tip.is_null() {
            log::warn!("Could not show the tray bubble");
            return;
        }
        // The control copies the text and title. Without a manifest the exe gets common controls v5, which
        // rejects the full v6 struct size: use the older size without the trailing `lpReserved`.
        let tool = TTTOOLINFOW {
            cbSize: (std::mem::size_of::<TTTOOLINFOW>() - std::mem::size_of::<*mut core::ffi::c_void>()) as u32,
            uFlags: TTF_TRACK | TTF_ABSOLUTE,
            hwnd: owner as _,
            uId: 1,
            lpszText: text_w.as_mut_ptr(),
            ..Default::default()
        };
        let tool_ptr = &tool as *const TTTOOLINFOW as isize;
        if SendMessageW(tip, TTM_ADDTOOLW, 0, tool_ptr) == 0 {
            log::warn!("Could not show the tray bubble (tooltip refused)");
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(tip);
            return;
        }
        SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, 320);
        SendMessageW(tip, TTM_SETTITLEW, TTI_INFO as usize, title_w.as_ptr() as isize);
        SendMessageW(tip, TTM_TRACKPOSITION, 0, (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize);
        SendMessageW(tip, TTM_TRACKACTIVATE, 1, tool_ptr);
        let timer = SetTimer(null_mut(), 0, 6000, Some(bubble_timer));
        BUBBLE.with(|b| b.set((tip as usize, timer)));
    }
}

unsafe extern "system" fn bubble_timer(_: windows_sys::Win32::Foundation::HWND, _: u32, _: usize, _: u32) {
    hide_bubble();
}

fn hide_bubble() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyWindow, KillTimer};
    let (tip, timer) = BUBBLE.with(|b| b.replace((0, 0)));
    unsafe {
        if timer != 0 {
            KillTimer(std::ptr::null_mut(), timer);
        }
        if tip != 0 {
            DestroyWindow(tip as _);
        }
    }
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
        let new_transcript = MenuItem::new("New transcript", true, None);
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
            &new_transcript,
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
            (new_transcript.id().clone(), TrayAction::NewTranscript),
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
