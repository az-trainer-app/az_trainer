//! Global shortcuts for options, e.g. `Alt+F1`.
//!
//! The game has keyboard focus while you play, so these are system-wide
//! hotkeys (`RegisterHotKey`), held only while a game is attached. Windows
//! posts them to the thread that registered them, so this thread owns every
//! registration, pumps its own message queue, and owns the toast window that
//! reports what a shortcut changed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, MSG, PM_REMOVE, WM_HOTKEY,
};

use crate::engine::{self, Shared};
use crate::keys;
use crate::toast::Toast;

/// Holding a key down fires once, instead of toggling on every repeat.
const MOD_NOREPEAT: u32 = 0x4000;

/// Keep the registered hotkeys in step with the attached config's `keys`.
pub fn start(shared: Arc<Mutex<Shared>>) {
    std::thread::spawn(move || run(shared));
}

fn run(shared: Arc<Mutex<Shared>>) {
    let mut toast = Toast::new();
    if toast.is_none() {
        println!("[hotkey] toast window unavailable; shortcuts still work");
    }
    let mut bound: Vec<Vec<String>> = Vec::new();
    // hotkey id - 1 -> (option index, 1-based level); None if it failed to register
    let mut ids: Vec<Option<(usize, usize)>> = Vec::new();

    loop {
        let wanted = shared.lock().map(|s| s.keys.clone()).unwrap_or_default();
        if wanted != bound {
            ids = rebind(&ids, &wanted);
            bound = wanted;
        }

        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_HOTKEY {
                if let Some(Some((opt, level))) = ids.get(msg.wParam.0.wrapping_sub(1)) {
                    engine::press(&shared, *opt, *level);
                    if let (Some(t), Some(text)) = (toast.as_mut(), engine::describe(&shared, *opt)) {
                        t.show(&text);
                    }
                }
            }
            unsafe {
                DispatchMessageW(&msg);
            }
        }
        if let Some(t) = toast.as_mut() {
            t.tick();
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Drop every registration and register `wanted` afresh.
fn rebind(old: &[Option<(usize, usize)>], wanted: &[Vec<String>]) -> Vec<Option<(usize, usize)>> {
    for id in 1..=old.len() {
        unsafe {
            let _ = UnregisterHotKey(None, id as i32);
        }
    }
    let mut ids = Vec::new();
    for (opt, option_keys) in wanted.iter().enumerate() {
        for (k, text) in option_keys.iter().enumerate() {
            let id = ids.len() as i32 + 1;
            let registered = keys::parse(text).is_some_and(|hk| unsafe {
                RegisterHotKey(None, id, HOT_KEY_MODIFIERS(hk.mods | MOD_NOREPEAT), hk.vk).is_ok()
            });
            if !registered {
                println!("[hotkey] {text}: invalid, or already taken by another program");
            }
            ids.push(registered.then_some((opt, k + 1)));
        }
    }
    ids
}
