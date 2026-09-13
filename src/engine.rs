//! The worker: find a config whose game is running, attach, tick it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::art;
use crate::js::Script;
use crate::mem::{find_pid, Proc};

const TICK: Duration = Duration::from_millis(100);

/// What the UI renders. The worker owns writing it.
#[derive(Default)]
pub struct Shared {
    pub title: String,
    /// state phrase only; the game's name is rendered separately
    pub status: String,
    pub names: Vec<String>,
    /// per option: label for its live value line, if it declares one
    pub shows: Vec<Option<String>>,
    /// per option: available multipliers; empty = plain on/off
    pub levels: Vec<Vec<f32>>,
    /// per option: pill captions, when the config supplies them
    pub labels: Vec<Vec<String>>,
    /// per option: 0 = off, else 1-based index into `levels` (1 = on for toggles)
    pub level: Vec<usize>,
    pub values: Vec<Option<String>>,
    pub art: Option<PathBuf>,
    pub ready: bool,
}

pub struct Engine {
    pub shared: Arc<Mutex<Shared>>,
    quit: Arc<AtomicBool>,
}

impl Engine {
    pub fn start() -> Engine {
        let shared = Arc::new(Mutex::new(Shared {
            status: "looking for a supported game...".into(),
            ..Default::default()
        }));
        let quit = Arc::new(AtomicBool::new(false));
        let (s, q) = (shared.clone(), quit.clone());
        std::thread::spawn(move || run(s, q));
        Engine { shared, quit }
    }

    /// Cycle an option: off -> level 1 -> ... -> off.
    pub fn cycle(&self, idx: usize) {
        if let Ok(mut s) = self.shared.lock() {
            let states = s.levels.get(idx).map(|l| l.len().max(1)).unwrap_or(1);
            if let Some(l) = s.level.get_mut(idx) {
                *l = if *l >= states { 0 } else { *l + 1 };
            }
        }
    }

    /// Pick a specific level directly (clicking a 2x / 4x / 8x pill).
    pub fn set_level(&self, idx: usize, level: usize) {
        if let Ok(mut s) = self.shared.lock() {
            if let Some(l) = s.level.get_mut(idx) {
                *l = if *l == level { 0 } else { level };
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        std::thread::sleep(TICK * 2); // let the worker restore `poke`d values
    }
}

/// Set the state phrase. Every caller is a state where nothing is hooked, so
/// this also clears `active`.
fn status(shared: &Arc<Mutex<Shared>>, msg: impl Into<String>) {
    if let Ok(mut s) = shared.lock() {
        s.status = msg.into();
    }
}

/// Drop everything that belongs to a particular game, so the UI falls back to
/// the searching state instead of showing a closed game's art and toggles.
fn forget_game(shared: &Arc<Mutex<Shared>>, msg: &str) {
    if let Ok(mut s) = shared.lock() {
        s.ready = false;
        s.status = msg.into();
        s.title.clear();
        s.art = None;
        s.names.clear();
        s.shows.clear();
        s.levels.clear();
        s.labels.clear();
        s.level.clear();
        s.values.clear();
    }
}

/// Newest mtime across a config and the shared lib it imports, so edits to
/// either trigger a reload.
fn config_stamp(config: &Path) -> u64 {
    let mut newest = 0u64;
    let mut note = |p: &Path| {
        if let Ok(t) = std::fs::metadata(p).and_then(|m| m.modified()) {
            if let Ok(d) = t.duration_since(SystemTime::UNIX_EPOCH) {
                newest = newest.max(d.as_secs());
            }
        }
    };
    note(config);
    if let Some(lib) = config.parent().and_then(|p| p.parent()).map(|p| p.join("lib")) {
        if let Ok(entries) = std::fs::read_dir(lib) {
            for e in entries.flatten() {
                note(&e.path());
            }
        }
    }
    newest
}

/// Where a config's remembered toggles live: `veilguard.js` -> `veilguard.settings`.
///
/// Deliberately a sibling of the config rather than one shared file, so a
/// config carries its own settings around with it. It is not inside `lib/`
/// and is not the config itself, so writing it never trips `config_stamp`
/// and cannot cause a reload loop.
fn settings_path(config: &Path) -> PathBuf {
    config.with_extension("settings")
}

/// `name<TAB>level` per line. Keyed by option NAME, so reordering or adding
/// options in the config leaves the rest of the settings intact.
fn load_settings(config: &Path) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(settings_path(config)) {
        for line in text.lines() {
            if let Some((name, lvl)) = line.rsplit_once('\t') {
                if let Ok(n) = lvl.trim().parse::<usize>() {
                    out.insert(name.to_string(), n);
                }
            }
        }
    }
    out
}

fn save_settings(config: &Path, names: &[String], levels: &[usize]) {
    let mut text = String::new();
    for (name, lvl) in names.iter().zip(levels) {
        text.push_str(name);
        text.push('\t');
        text.push_str(&lvl.to_string());
        text.push('\n');
    }
    let _ = std::fs::write(settings_path(config), text);
}

fn run(shared: Arc<Mutex<Shared>>, quit: Arc<AtomicBool>) {
    // option states survive a reload, matched by name
    let mut pending: HashMap<String, usize> = HashMap::new();
    loop {
        if quit.load(Ordering::Relaxed) {
            return;
        }

        // ---- find a config whose process is running -------------------
        let paths = Script::discover();
        if paths.is_empty() {
            status(&shared, "no configs\\games\\*.js found next to the exe");
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }

        let mut chosen: Option<(Script, u32)> = None;
        let mut errors: Vec<String> = Vec::new();
        for p in &paths {
            match Script::load(p) {
                Ok(s) => {
                    if let Some(pid) = find_pid(&s.process) {
                        chosen = Some((s, pid));
                        break;
                    }
                }
                Err(e) => errors.push(format!(
                    "{}: {e}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                )),
            }
        }

        let Some((script, pid)) = chosen else {
            status(
                &shared,
                if errors.is_empty() {
                    "waiting for a supported game...".to_string()
                } else {
                    format!("config error - {}", errors.join("; "))
                },
            );
            std::thread::sleep(Duration::from_millis(1500));
            continue;
        };

        let Some(proc) = Proc::open(pid) else {
            status(&shared, "cannot open process - run as Administrator");
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        let Some((base, size)) = proc.module(&script.process) else {
            status(&shared, "module not found, retrying...");
            continue;
        };

        status(&shared, "scanning...");
        script.attach(proc, base, size);

        let art = art::resolve(&script);
        let saved = load_settings(&script.path);
        if let Ok(mut s) = shared.lock() {
            s.title = script.title.clone();
            s.names = script.options.iter().map(|o| o.name.clone()).collect();
            s.shows = script.options.iter().map(|o| o.show.clone()).collect();
            s.levels = script.options.iter().map(|o| o.levels.clone()).collect();
            s.labels = script.options.iter().map(|o| o.labels.clone()).collect();
            s.level = script
                .options
                .iter()
                // nothing is on unless it was on last time: a trainer that
                // enables itself is a trainer that surprises you
                .map(|o| {
                    pending
                        .get(&o.name)
                        .copied()
                        .or_else(|| saved.get(&o.name).copied())
                        .unwrap_or(0)
                })
                .collect();
            s.values = script.options.iter().map(|_| None).collect();
            s.art = art;
            s.ready = true;
            s.status = format!("attached (pid {pid})");
            println!("[engine] attached {} pid {pid}, {} options, art={:?}",
                     script.title, s.names.len(), s.art);
            { use std::io::Write; let _ = std::io::stdout().flush(); }
        }

        // ---- tick -----------------------------------------------------
        let stamp = config_stamp(&script.path);
        let mut since_check = 0u32;
        // written only when it actually changes, so a click costs one write
        // and an idle session costs none
        let mut saved_levels: Vec<usize> =
            shared.lock().map(|s| s.level.clone()).unwrap_or_default();
        loop {
            if quit.load(Ordering::Relaxed) {
                turn_all_off(&script);
                script.detach();
                return;
            }
            if find_pid(&script.process) != Some(pid) {
                forget_game(&shared, "game closed - looking for a game...");
                script.detach();
                break;
            }

            // hot reload: pick up edits to the config or the shared lib
            since_check += 1;
            if since_check >= 10 {
                since_check = 0;
                let now = config_stamp(&script.path);
                if now != stamp {
                    println!("[reload] config changed, reloading");
                    { use std::io::Write; let _ = std::io::stdout().flush(); }
                    if let Ok(s) = shared.lock() {
                        for (i, o) in script.options.iter().enumerate() {
                            pending.insert(
                                o.name.clone(),
                                s.level.get(i).copied().unwrap_or(0),
                            );
                        }
                    }
                    turn_all_off(&script);   // undo patches before dropping it
                    script.detach();
                    break;
                }
            }

            let levels: Vec<usize> = shared.lock().map(|s| s.level.clone()).unwrap_or_default();
            if levels != saved_levels {
                let names: Vec<String> =
                    script.options.iter().map(|o| o.name.clone()).collect();
                save_settings(&script.path, &names, &levels);
                saved_levels = levels.clone();
            }
            let mut live = false;

            for (i, opt) in script.options.iter().enumerate() {
                let level = levels.get(i).copied().unwrap_or(0);
                let on = level > 0;
                let mult = if opt.levels.is_empty() {
                    1.0
                } else {
                    opt.levels.get(level.saturating_sub(1)).copied().unwrap_or(1.0)
                };

                let text = script.tick(i, on, mult);
                if text.is_some() {
                    live = true;
                }
                // always publish, so switching an option off clears its line
                // instead of leaving the last value frozen on screen
                if let Ok(mut s) = shared.lock() {
                    if let Some(slot) = s.values.get_mut(i) {
                        *slot = text;
                    }
                }
            }

            let live = script.live().unwrap_or(live);
            if let Ok(mut s) = shared.lock() {
                s.status = if live { "active" } else { "in menu / loading..." }.into();
            }
            std::thread::sleep(TICK);
        }
    }
}

/// Run every option once with on=false so `poke`d constants get restored.
fn turn_all_off(script: &Script) {
    for i in 0..script.options.len() {
        let _ = script.tick(i, false, 1.0);
    }
}
