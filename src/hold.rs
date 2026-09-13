//! High-frequency value holding.
//!
//! Some values are rewritten by the game every frame -- Dawnwalker's
//! CustomTimeDilation is, because that is how its Haste works. Writing such a
//! value from the 10 Hz tick loop loses the race and produces a visible
//! stutter as the value flickers between ours and the game's.
//!
//! This keeps a small set of (address, value) pairs and rewrites them from a
//! dedicated thread at ~1 kHz, which wins reliably. Measured effect on
//! Dawnwalker: 4.10x travel speed where the 10 Hz version did nothing useful.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::HANDLE;

/// What to do with a held address each pass.
#[derive(Clone, Copy)]
pub enum Mode {
    /// Force a constant.
    Set(f32),
    /// Multiply whatever the GAME most recently wrote.
    ///
    /// Needed when a value has several natural tiers - Dawnwalker's
    /// CustomTimeDilation is 1.0 walking/sprinting and 3.25 while Haste is
    /// active. Forcing a constant would override Haste and make it SLOWER.
    /// Scaling keeps every tier in proportion: 2x gives 2.0 and 6.5.
    Scale(f32),
}

#[derive(Clone, Copy)]
struct Item {
    addr: u64,
    mode: Mode,
    /// the last value we wrote, so our own write is not scaled again
    written: f32,
}

struct State {
    handle: isize,
    items: Vec<Item>,
    running: bool,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State { handle: 0, items: Vec::new(), running: false })
    })
}

/// Point the writer at a process, starting its thread on first use.
pub fn attach(handle: HANDLE) {
    let mut s = state().lock().unwrap();
    s.handle = handle.0 as isize;
    s.items.clear();
    if !s.running {
        s.running = true;
        std::thread::spawn(writer);
    }
}

/// Stop writing (the thread stays parked, costing nothing).
pub fn detach() {
    let mut s = state().lock().unwrap();
    s.handle = 0;
    s.items.clear();
}

/// Replace the held set. Called every tick, so the address can follow an
/// object that moves when the game reloads. Existing entries keep their
/// `written` memory so scaling does not restart on every tick.
pub fn set(items: Vec<(u64, Mode)>) {
    let mut s = state().lock().unwrap();
    let old = s.items.clone();
    s.items = items
        .into_iter()
        .map(|(addr, mode)| {
            let written = old
                .iter()
                .find(|i| i.addr == addr)
                .map(|i| i.written)
                .unwrap_or(f32::NAN);
            Item { addr, mode, written }
        })
        .collect();
}

pub fn clear() {
    state().lock().unwrap().items.clear();
}

fn writer() {
    loop {
        let (handle, items) = {
            let s = state().lock().unwrap();
            (s.handle, s.items.clone())
        };

        if handle == 0 || items.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }

        let h = HANDLE(handle as *mut c_void);
        let mut local = items;

        // a burst per wake-up: sleeping costs far more than the writes, and
        // the game rewrites these every frame
        for _ in 0..20 {
            for it in local.iter_mut() {
                let value = match it.mode {
                    Mode::Set(v) => Some(v),
                    Mode::Scale(m) => {
                        // read what is there; if the game has written a new
                        // base since our last write, scale that base
                        let mut buf = [0u8; 4];
                        let mut got = 0usize;
                        let ok = unsafe {
                            windows::Win32::System::Diagnostics::Debug::ReadProcessMemory(
                                h,
                                it.addr as *const c_void,
                                buf.as_mut_ptr() as *mut c_void,
                                4,
                                Some(&mut got),
                            )
                            .is_ok()
                        };
                        if !ok || got != 4 {
                            None
                        } else {
                            let cur = f32::from_le_bytes(buf);
                            if cur.is_finite()
                                && cur > 0.0
                                && (it.written.is_nan()
                                    || (cur - it.written).abs() > 1e-4)
                            {
                                Some(cur * m)
                            } else {
                                None // already ours; leave it alone
                            }
                        }
                    }
                };

                if let Some(v) = value {
                    let bytes = v.to_le_bytes();
                    let mut put = 0usize;
                    unsafe {
                        let _ = windows::Win32::System::Diagnostics::Debug::WriteProcessMemory(
                            h,
                            it.addr as *const c_void,
                            bytes.as_ptr() as *const c_void,
                            4,
                            Some(&mut put),
                        );
                    }
                    it.written = v;
                }
            }
            std::thread::sleep(std::time::Duration::from_micros(500));
        }

        // carry the `written` memory back so scaling stays stable
        {
            let mut s = state().lock().unwrap();
            for it in &local {
                if let Some(dst) = s.items.iter_mut().find(|d| d.addr == it.addr) {
                    dst.written = it.written;
                }
            }
        }
    }
}
