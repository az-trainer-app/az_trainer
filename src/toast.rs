//! A brief notice at the bottom-left of the game's monitor - "Speed: 4x" -
//! when a shortcut changes an option while the trainer is in the background.
//!
//! A plain always-on-top window that is click-through, never takes focus and
//! stays out of the taskbar. It shows over windowed and borderless games; an
//! exclusive-fullscreen game draws over it.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, GetDC, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, SetWindowRgn, DT_CALCRECT, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, HFONT, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyIcon, DrawIconEx, GetClientRect, GetForegroundWindow,
    GetWindowThreadProcessId, LoadImageW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos,
    ShowWindow, DI_NORMAL, HICON, HWND_TOPMOST, IMAGE_ICON, LR_DEFAULTCOLOR,
    LWA_ALPHA, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WM_ERASEBKGND, WM_NCHITTEST, WM_PAINT,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

/// How long a toast stays up.
const SHOW_FOR: Duration = Duration::from_millis(1500);

// the trainer window's palette, as COLORREF (0x00BBGGRR)
const PANEL: COLORREF = COLORREF(0x00231b18);
const TEXT: COLORREF = COLORREF(0x00e3dad6);
const ACCENT: COLORREF = COLORREF(0x003b3bc2);

/// Size of the app icon drawn before the text, in 96-dpi pixels.
const ICON: f32 = 20.0;

/// What WM_PAINT draws; set by `show` on the same thread.
struct Paint {
    text: Vec<u16>,
    font: HFONT,
    /// the exe's own icon (resource 1) at the current monitor's scale
    icon: HICON,
    scale: f32,
}

thread_local! {
    static PAINT: RefCell<Paint> = RefCell::new(Paint {
        text: Vec::new(),
        font: HFONT::default(),
        icon: HICON::default(),
        scale: 1.0,
    });
}

pub struct Toast {
    hwnd: HWND,
    dpi: u32,
    until: Option<Instant>,
}

impl Toast {
    /// Create the (hidden) window. Must stay on the thread that pumps its messages.
    pub fn new() -> Option<Toast> {
        unsafe {
            let hinstance = GetModuleHandleW(None).ok()?;
            let class = w!("AzTrainerToast");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                hinstance,
                None,
            )
            .ok()?;
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA).ok()?;
            Some(Toast { hwnd, dpi: 0, until: None })
        }
    }

    /// Show `text` for `SHOW_FOR` - unless the trainer itself is in front,
    /// where the change is already visible in its own window.
    pub fn show(&mut self, text: &str) {
        unsafe {
            let front = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(front, Some(&mut pid));
            if pid == GetCurrentProcessId() {
                return;
            }

            // the monitor the game is on, at that monitor's scale
            let monitor = MonitorFromWindow(front, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return;
            }
            let (mut dpi, mut _y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
            let scale = dpi as f32 / 96.0;
            let px = |v: f32| (v * scale).round() as i32;

            let mut wide: Vec<u16> = text.encode_utf16().collect();
            let font = PAINT.with(|p| {
                let mut p = p.borrow_mut();
                if dpi != self.dpi || p.font.is_invalid() {
                    if !p.font.is_invalid() {
                        let _ = DeleteObject(p.font);
                    }
                    if !p.icon.is_invalid() {
                        let _ = DestroyIcon(p.icon);
                    }
                    p.font = CreateFontW(-px(15.0), 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, w!("Segoe UI"));
                    p.icon = GetModuleHandleW(None)
                        .ok()
                        .and_then(|module| {
                            let size = px(ICON);
                            LoadImageW(module, PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR).ok()
                        })
                        .map(|h| HICON(h.0))
                        .unwrap_or_default();
                    self.dpi = dpi;
                }
                p.text = wide.clone();
                p.scale = scale;
                p.font
            });

            // size to the text
            let hdc = GetDC(self.hwnd);
            let old = SelectObject(hdc, font);
            let mut measured = RECT::default();
            DrawTextW(hdc, &mut wide, &mut measured, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
            SelectObject(hdc, old);
            ReleaseDC(self.hwnd, hdc);

            // stripe, padding, icon, gap, text, padding
            let width = px(4.0) + px(14.0) + px(ICON) + px(10.0) + (measured.right - measured.left) + px(16.0);
            let height = (measured.bottom - measured.top).max(px(ICON)) + px(10.0) * 2;
            let margin = px(32.0);
            let x = info.rcMonitor.left + margin;
            let y = info.rcMonitor.bottom - margin - height;

            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, px(10.0), px(10.0));
            SetWindowRgn(self.hwnd, region, true); // the window owns the region now
            let _ = InvalidateRect(self.hwnd, None, false);
        }
        self.until = Some(Instant::now() + SHOW_FOR);
    }

    /// Hide once the toast has been up long enough.
    pub fn tick(&mut self) {
        if self.until.is_some_and(|t| Instant::now() >= t) {
            self.until = None;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // painted in full by WM_PAINT: no flicker
        WM_NCHITTEST => LRESULT(-1), // HTTRANSPARENT: clicks go to the game
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint(hwnd: HWND) {
    let (mut text, font, icon, scale) = PAINT.with(|p| {
        let p = p.borrow();
        (p.text.clone(), p.font, p.icon, p.scale)
    });
    let px = |v: f32| (v * scale).round() as i32;

    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);

    let panel = CreateSolidBrush(PANEL);
    FillRect(hdc, &rc, panel);
    let _ = DeleteObject(panel);
    let accent = CreateSolidBrush(ACCENT);
    let stripe = RECT { right: rc.left + px(4.0), ..rc };
    FillRect(hdc, &stripe, accent);
    let _ = DeleteObject(accent);

    let icon_left = stripe.right + px(14.0);
    if !icon.is_invalid() {
        let size = px(ICON);
        let top = rc.top + (rc.bottom - rc.top - size) / 2;
        let _ = DrawIconEx(hdc, icon_left, top, icon, size, size, 0, None, DI_NORMAL);
    }

    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, TEXT);
    let old = SelectObject(hdc, font);
    let mut area = RECT { left: icon_left + px(ICON) + px(10.0), ..rc };
    DrawTextW(hdc, &mut text, &mut area, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
    SelectObject(hdc, old);
    let _ = EndPaint(hwnd, &ps);
}
