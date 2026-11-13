//! The app's own notification card: a small window above the tray, bottom right, in the Windows 11 look
//! (rounded corners, Segoe UI, the app icon). Windows' own notifications can be switched off (Do Not
//! Disturb, notification settings), and egui can't open a window while the main window is hidden, so this
//! is plain Win32. Everything runs on the main (UI) thread.

use std::cell::RefCell;
use std::rc::Rc;

use eframe::egui::Color32;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DT_CENTER,
    DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FillRect, HFONT,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIcon, CreateWindowExW, DI_NORMAL, HWND_TOPMOST, SWP_NOACTIVATE, SetWindowPos, DefWindowProcW, DestroyWindow, DrawIconEx, HICON, IDC_HAND, KillTimer,
    LoadCursorW, MA_NOACTIVATE, RegisterClassW, SPI_GETWORKAREA, SW_SHOWNOACTIVATE, SetTimer, ShowWindow,
    SystemParametersInfoW, WM_DESTROY, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::theme::Palette;

const CLASS: &str = "VerbaToast";
const SHOW_MS: u32 = 6000;
// Layout at 96 dpi; scaled to the screen's dpi.
const WIDTH: i32 = 372;
const HEIGHT: i32 = 108;
const MARGIN: i32 = 12; // from the work area's corner

struct Card {
    hwnd: HWND,
    title: Vec<u16>,
    text: Vec<u16>,
    bg: u32,
    fg: u32,
    muted: u32,
    dpi: i32,
    on_click: Rc<dyn Fn()>,
}

thread_local! {
    static CARD: RefCell<Option<Card>> = const { RefCell::new(None) };
    static ICON: RefCell<HICON> = const { RefCell::new(std::ptr::null_mut()) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// COLORREF is 0x00BBGGRR.
fn colorref(c: Color32) -> u32 {
    c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16
}

/// Shows the card (replacing one already shown). `on_click` runs when the card body is clicked.
pub fn show(title: &str, text: &str, p: &Palette, on_click: impl Fn() + 'static) {
    close();
    // SAFETY: plain Win32 calls on the UI thread with valid, NUL-terminated strings.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS);
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
                    lpszClassName: class.as_ptr(),
                    hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
                    ..std::mem::zeroed()
                };
                RegisterClassW(&wc);
                *r.borrow_mut() = true;
            }
        });
        // The UI thread may run DPI-unaware (scaled, blurry, in scaled coordinates). Create the card as a
        // per-monitor-aware window instead: real pixels, sharp text, placed exactly above the taskbar.
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mut work = RECT { left: 0, top: 0, right: 1920, bottom: 1040 };
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut RECT as _, 0);
        let name = wide("Verba notification");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW, // on top, no taskbar button
            class.as_ptr(),
            name.as_ptr(),
            WS_POPUP,
            work.right - 1,
            work.bottom - 1,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            SetThreadDpiAwarenessContext(previous);
            log::warn!("Could not show the notification card");
            return;
        }
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let s = |v: i32| v * dpi / 96;
        let (w, h) = (s(WIDTH), s(HEIGHT));
        SetWindowPos(hwnd, HWND_TOPMOST, work.right - w - s(MARGIN), work.bottom - h - s(MARGIN), w, h, SWP_NOACTIVATE);
        SetThreadDpiAwarenessContext(previous);
        let pref = DWMWCP_ROUND; // Windows 11: rounded corners with the native border and shadow
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &pref as *const _ as _, 4);
        CARD.with(|c| {
            *c.borrow_mut() = Some(Card {
                hwnd,
                title: wide(title),
                text: wide(text),
                bg: colorref(p.bg),
                fg: colorref(p.text),
                muted: colorref(p.muted),
                dpi,
                on_click: Rc::new(on_click),
            })
        });
        SetTimer(hwnd, 1, SHOW_MS, None);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE); // don't take the focus from what the user is doing
    }
}

/// Closes the card if one is shown.
pub fn close() {
    let hwnd = CARD.with(|c| c.borrow().as_ref().map(|c| c.hwnd));
    if let Some(hwnd) = hwnd {
        // SAFETY: our own window; WM_DESTROY clears CARD.
        unsafe { DestroyWindow(hwnd) };
    }
}

/// The app icon as a Windows icon, made once.
fn icon() -> HICON {
    ICON.with(|i| {
        let mut i = i.borrow_mut();
        if i.is_null() {
            const N: i32 = 64;
            let rgba = crate::theme::mic_icon(N as u32, [0x2f, 0x6f, 0xd6]);
            let bgra: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
            let mask = vec![0u8; (N * N / 8) as usize]; // the alpha channel decides transparency
            // SAFETY: 64×64, 1 plane, 32 bpp colour data and a 1 bpp mask of the right sizes.
            *i = unsafe { CreateIcon(std::ptr::null_mut(), N, N, 1, 32, mask.as_ptr(), bgra.as_ptr()) };
        }
        *i
    })
}

unsafe fn font(height: i32, weight: i32, face: &str) -> HFONT {
    let face = wide(face);
    // SAFETY: valid face name; the caller deletes the font.
    unsafe {
        CreateFontW(
            -height, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET as _, OUT_DEFAULT_PRECIS as _, CLIP_DEFAULT_PRECIS as _, CLEARTYPE_QUALITY as _, 0,
            face.as_ptr(),
        )
    }
}

/// Close button area, in client pixels.
fn close_rect(w: i32, dpi: i32) -> RECT {
    let s = |v: i32| v * dpi / 96;
    RECT { left: w - s(36), top: s(6), right: w - s(6), bottom: s(36) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: called by Windows on the UI thread for our window.
    unsafe {
        match msg {
            WM_PAINT => {
                CARD.with(|c| {
                    let c = c.borrow();
                    let Some(card) = c.as_ref() else { return };
                    paint(hwnd, card);
                });
                0
            }
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_LBUTTONUP => {
                let (x, y) = ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32);
                let (on_close, click) = CARD.with(|c| {
                    let c = c.borrow();
                    let Some(card) = c.as_ref() else { return (true, None) };
                    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
                    let r = close_rect(client.right, card.dpi);
                    let on_close = x >= r.left && x < r.right && y >= r.top && y < r.bottom;
                    (on_close, Some(card.on_click.clone()))
                });
                DestroyWindow(hwnd);
                if let (false, Some(f)) = (on_close, click) {
                    f();
                }
                0
            }
            WM_TIMER => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, 1);
                CARD.with(|c| {
                    let mut c = c.borrow_mut();
                    if c.as_ref().is_some_and(|card| card.hwnd == hwnd) {
                        *c = None;
                    }
                });
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn paint(hwnd: HWND, card: &Card) {
    let s = |v: i32| v * card.dpi / 96;
    // SAFETY: standard WM_PAINT sequence; every GDI object created here is deleted here.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
        let brush = CreateSolidBrush(card.bg);
        FillRect(hdc, &client, brush);
        DeleteObject(brush as _);
        DrawIconEx(hdc, s(18), s(20), icon(), s(40), s(40), 0, std::ptr::null_mut(), DI_NORMAL);
        SetBkMode(hdc, TRANSPARENT as _);
        let left = s(72);
        let right = client.right - s(40);
        // Title
        let title_font = font(s(15), 600, "Segoe UI");
        let old = SelectObject(hdc, title_font as _);
        SetTextColor(hdc, card.fg);
        let mut r = RECT { left, top: s(16), right, bottom: s(38) };
        DrawTextW(hdc, card.title.as_ptr() as *mut u16, -1, &mut r, DT_SINGLELINE | DT_END_ELLIPSIS);
        // Message
        let text_font = font(s(13), 400, "Segoe UI");
        SelectObject(hdc, text_font as _);
        SetTextColor(hdc, card.muted);
        let mut r = RECT { left, top: s(40), right: client.right - s(18), bottom: client.bottom - s(10) };
        DrawTextW(hdc, card.text.as_ptr() as *mut u16, -1, &mut r, DT_WORDBREAK | DT_END_ELLIPSIS);
        // Close button (Segoe MDL2 Assets "ChromeClose")
        let glyph_font = font(s(10), 400, "Segoe MDL2 Assets");
        SelectObject(hdc, glyph_font as _);
        let mut r = close_rect(client.right, card.dpi);
        let glyph = wide("\u{E8BB}");
        DrawTextW(hdc, glyph.as_ptr() as *mut u16, -1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(hdc, old);
        DeleteObject(title_font as _);
        DeleteObject(text_font as _);
        DeleteObject(glyph_font as _);
        EndPaint(hwnd, &ps);
    }
}
