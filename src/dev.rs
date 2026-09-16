//! Eyes and hands for research: capture the attached game's window, and send
//! it keys and mouse input, so a research session can look at the game, act
//! in it, and watch memory respond - all through the devtools channel.
//!
//! Compiled only with `--features devtools`. A released build never
//! synthesizes input or captures other windows.

use std::path::Path;

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetClientRect, GetForegroundWindow, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

/// The process's main window: its largest visible top-level window.
pub fn game_window(pid: u32) -> Option<HWND> {
    struct Search {
        pid: u32,
        best: Option<(HWND, i64)>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut owner));
        if owner == search.pid && IsWindowVisible(hwnd).as_bool() {
            let mut rc = RECT::default();
            if GetClientRect(hwnd, &mut rc).is_ok() {
                let area = (rc.right - rc.left) as i64 * (rc.bottom - rc.top) as i64;
                if search.best.map_or(true, |(_, a)| area > a) {
                    search.best = Some((hwnd, area));
                }
            }
        }
        BOOL(1)
    }
    let mut search = Search { pid, best: None };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize));
    }
    search.best.map(|(h, _)| h)
}

/// Bring the window to the front, so input reaches it. Windows only lets the
/// foreground app hand focus away, so this briefly joins the foreground
/// window's input queue to borrow that right.
pub fn focus(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if GetForegroundWindow() == hwnd {
            return true;
        }
        let front = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let me = GetCurrentThreadId();
        let joined = front != 0 && front != me && AttachThreadInput(me, front, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if joined {
            let _ = AttachThreadInput(me, front, false);
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
        GetForegroundWindow() == hwnd
    }
}

/// Save the window's client area as a PNG, scaled down to at most `max_width`
/// pixels wide. Asks the window to render itself first, which works for most
/// games in windowed or borderless mode; if that comes back black, copies the
/// window's spot on the screen instead, which needs it in front.
pub fn capture(hwnd: HWND, max_width: u32, path: &Path) -> Result<(u32, u32), String> {
    unsafe {
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc).map_err(|e| e.to_string())?;
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || h <= 0 {
            return Err("the game window has no size (minimized?)".into());
        }

        let grab = |from_screen: bool| -> Option<Vec<u8>> {
            let window_dc = GetDC(hwnd);
            let mem_dc = CreateCompatibleDC(window_dc);
            let bitmap = CreateCompatibleBitmap(window_dc, w, h);
            let old = SelectObject(mem_dc, bitmap);
            let drawn = if from_screen {
                let screen = GetDC(HWND::default());
                let mut origin = POINT::default();
                let _ = ClientToScreen(hwnd, &mut origin);
                let ok = BitBlt(mem_dc, 0, 0, w, h, screen, origin.x, origin.y, SRCCOPY).is_ok();
                ReleaseDC(HWND::default(), screen);
                ok
            } else {
                // PW_CLIENTONLY | PW_RENDERFULLCONTENT
                PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(1 | 2)).as_bool()
            };
            let mut info = BITMAPINFO::default();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut pixels = vec![0u8; w as usize * h as usize * 4];
            let lines = GetDIBits(
                mem_dc,
                bitmap,
                0,
                h as u32,
                Some(pixels.as_mut_ptr() as *mut _),
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(mem_dc, old);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, window_dc);
            (drawn && lines == h).then_some(pixels)
        };

        let black = |p: &Vec<u8>| p.chunks_exact(4).step_by(97).all(|px| px[0] | px[1] | px[2] == 0);
        let bgra = match grab(false) {
            Some(p) if !black(&p) => p,
            _ => grab(true).ok_or("could not copy the game window")?,
        };

        let rgb: Vec<u8> = bgra.chunks_exact(4).flat_map(|px| [px[2], px[1], px[0]]).collect();
        let mut img = image::RgbImage::from_raw(w as u32, h as u32, rgb).ok_or("bad capture size")?;
        if max_width > 0 && w as u32 > max_width {
            let nh = (h as f32 * max_width as f32 / w as f32).round().max(1.0) as u32;
            img = image::imageops::resize(&img, max_width, nh, image::imageops::FilterType::Triangle);
        }
        img.save(path).map_err(|e| e.to_string())?;
        Ok(img.dimensions())
    }
}

/// A key name as the game hears it: virtual key and whether it is an extended
/// key. Letters, digits, F1-F24, Num0-Num9, and the usual named keys.
fn key(name: &str) -> Option<(u16, bool)> {
    let n = name.trim().to_ascii_lowercase();
    let number = |prefix: &str| n.strip_prefix(prefix).and_then(|d| d.parse::<u16>().ok());
    if n.len() == 1 {
        let c = n.as_bytes()[0];
        if c.is_ascii_lowercase() {
            return Some((c.to_ascii_uppercase() as u16, false));
        }
        if c.is_ascii_digit() {
            return Some((c as u16, false));
        }
    }
    if let Some(d) = number("num").filter(|d| *d <= 9) {
        return Some((0x60 + d, false));
    }
    if let Some(d) = number("f").filter(|d| (1..=24).contains(d)) {
        return Some((0x6f + d, false));
    }
    Some(match n.as_str() {
        "esc" | "escape" => (0x1b, false),
        "tab" => (0x09, false),
        "space" => (0x20, false),
        "enter" | "return" => (0x0d, false),
        "backspace" => (0x08, false),
        "shift" => (0xa0, false),
        "ctrl" | "control" => (0xa2, false),
        "alt" => (0xa4, false),
        "capslock" => (0x14, false),
        "up" => (0x26, true),
        "down" => (0x28, true),
        "left" => (0x25, true),
        "right" => (0x27, true),
        "insert" => (0x2d, true),
        "delete" => (0x2e, true),
        "home" => (0x24, true),
        "end" => (0x23, true),
        "pageup" => (0x21, true),
        "pagedown" => (0x22, true),
        "tilde" | "`" => (0xc0, false),
        _ => return None,
    })
}

fn send(inputs: &[INPUT]) -> bool {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize == inputs.len() }
}

fn key_input(vk: u16, extended: bool, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: KEYBD_EVENT_FLAGS(flags.0),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Hold `keys` down together for `hold_ms`, then release them in reverse.
/// Sent as scan codes, which games reading raw input also see.
pub fn press(keys: &[String], hold_ms: u64) -> Result<(), String> {
    let parsed: Vec<(u16, bool)> = keys
        .iter()
        .map(|k| key(k).ok_or_else(|| format!("unknown key {k:?}")))
        .collect::<Result<_, _>>()?;
    let downs: Vec<INPUT> = parsed.iter().map(|&(vk, ext)| key_input(vk, ext, false)).collect();
    let ups: Vec<INPUT> = parsed.iter().rev().map(|&(vk, ext)| key_input(vk, ext, true)).collect();
    if !send(&downs) {
        return Err("input was blocked".into());
    }
    std::thread::sleep(std::time::Duration::from_millis(hold_ms));
    send(&ups).then_some(()).ok_or_else(|| "input was blocked".into())
}

/// Press `keys` down, or release them, without the other half - for holds
/// that need other input or a capture in between (aim, then fire).
pub fn hold(keys: &[String], down: bool) -> Result<(), String> {
    let parsed: Vec<(u16, bool)> = keys
        .iter()
        .map(|k| key(k).ok_or_else(|| format!("unknown key {k:?}")))
        .collect::<Result<_, _>>()?;
    let inputs: Vec<INPUT> = if down {
        parsed.iter().map(|&(vk, ext)| key_input(vk, ext, false)).collect()
    } else {
        parsed.iter().rev().map(|&(vk, ext)| key_input(vk, ext, true)).collect()
    };
    send(&inputs).then_some(()).ok_or_else(|| "input was blocked".into())
}

/// Move the mouse by a relative amount - how games read camera turns.
pub fn mouse_move(dx: i32, dy: i32) -> bool {
    send(&[mouse_input(dx, dy, MOUSEEVENTF_MOVE)])
}

/// Press and release a mouse button: `left`, `right` or `middle`.
pub fn click(button: &str, hold_ms: u64) -> Result<(), String> {
    let (down, up) = match button {
        "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        other => return Err(format!("unknown button {other:?}")),
    };
    if !send(&[mouse_input(0, 0, down)]) {
        return Err("input was blocked".into());
    }
    std::thread::sleep(std::time::Duration::from_millis(hold_ms));
    send(&[mouse_input(0, 0, up)]).then_some(()).ok_or_else(|| "input was blocked".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_virtual_keys() {
        assert_eq!(key("W"), Some((0x57, false)));
        assert_eq!(key("i"), Some((0x49, false)));
        assert_eq!(key("5"), Some((0x35, false)));
        assert_eq!(key("F4"), Some((0x73, false)));
        assert_eq!(key("Num3"), Some((0x63, false)));
        assert_eq!(key("Escape"), Some((0x1b, false)));
        assert_eq!(key("Up"), Some((0x26, true)));
        assert_eq!(key("Shift"), Some((0xa0, false)));
        assert_eq!(key("F25"), None);
        assert_eq!(key("Banana"), None);
    }
}
