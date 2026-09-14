//! The worker: find a config whose game is running, attach, tick it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::art;
use crate::game::Identity;
use crate::js::{Build, Script};
use crate::mem::{find_pid, Proc};

const TICK: Duration = Duration::from_millis(100);

/// What the UI renders. The worker owns writing it.
#[derive(Default)]
pub struct Shared {
    pub title: String,
    /// state phrase only; the game's name is rendered separately
    pub status: String,
    pub names: Vec<String>,
    /// per option: `Some(heading)` when the entry is a divider, not an option
    pub separators: Vec<Option<String>>,
    /// per option: label for its live value line, if it declares one
    pub shows: Vec<Option<String>>,
    /// per option: available multipliers; empty = plain on/off
    pub levels: Vec<Vec<f32>>,
    /// per option: pill captions, when the config supplies them
    pub labels: Vec<Vec<String>>,
    /// per option: shortcuts, one per level (one for a toggle); registered
    /// system-wide while a game is attached
    pub keys: Vec<Vec<String>>,
    /// per option: 0 = off, else 1-based index into `levels` (1 = on for toggles)
    pub level: Vec<usize>,
    pub values: Vec<Option<String>>,
    /// option columns the config asks for
    pub columns: usize,
    pub art: Option<art::Art>,
    /// status bar text for the loaded config: file, update date, checksum
    pub config_info: Option<String>,
    /// the loaded config relative to `configs/`, e.g. `games/dawnwalker.js`
    pub config_rel: Option<String>,
    pub ready: bool,
}

pub struct Engine {
    pub shared: Arc<Mutex<Shared>>,
    quit: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn start() -> Engine {
        let shared = Arc::new(Mutex::new(Shared {
            status: "looking for a supported game...".into(),
            ..Default::default()
        }));
        let quit = Arc::new(AtomicBool::new(false));
        let (s, q) = (shared.clone(), quit.clone());
        let worker = Some(std::thread::spawn(move || run(s, q)));
        #[cfg(windows)]
        crate::hotkey::start(shared.clone());
        Engine { shared, quit, worker }
    }

    /// A config's window with no game attached, showing its saved settings.
    ///
    /// For screenshots and for checking a script's options. Nothing ticks,
    /// nothing touches a process, and clicks are not saved.
    pub fn preview(path: PathBuf) -> Engine {
        let shared = Arc::new(Mutex::new(Shared {
            status: "loading preview...".into(),
            ..Default::default()
        }));
        let quit = Arc::new(AtomicBool::new(false));
        let (s, q) = (shared.clone(), quit.clone());
        let worker = Some(std::thread::spawn(move || match Script::load(&path) {
            Ok(script) => {
                // defaults: everything off, so previews and screenshots show a
                // clean window regardless of the developer's saved settings
                let level = vec![0; script.options.len()];
                if let Ok(mut st) = s.lock() {
                    publish(&mut st, &script, level);
                }
                nap(&q, Duration::MAX);
            }
            Err(e) => status(&s, format!("preview failed - {e}")),
        }));
        Engine { shared, quit, worker }
    }

    /// Stop the worker and wait until it has restored every patch.
    ///
    /// Waiting is the point: a restart that launches the new exe while the old
    /// one's byte patches are still in the game leaves the new instance unable
    /// to find its own signatures.
    pub fn shutdown(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
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
        press(&self.shared, idx, level);
    }
}

/// An option's state for a toast: `"Speed: 4x"`, `"Speed: 1x"` when off,
/// `"Infinite Health: ON"`, or a level's own caption (`"Denarius: 99k"`).
pub fn describe(shared: &Arc<Mutex<Shared>>, idx: usize) -> Option<String> {
    let s = shared.lock().ok()?;
    let name = s.names.get(idx)?;
    let level = s.level.get(idx).copied().unwrap_or(0);
    let levels = s.levels.get(idx).map(Vec::as_slice).unwrap_or_default();
    let labels = s.labels.get(idx).map(Vec::as_slice).unwrap_or_default();
    let multiplier = |v: f32| format!("{}x", v);
    let state = match (levels.is_empty(), level) {
        (true, 0) => "OFF".to_string(),
        (true, _) => "ON".to_string(),
        // plain multipliers read as 1x when off; captioned levels just say OFF
        (false, 0) if labels.is_empty() => multiplier(1.0),
        (false, 0) => "OFF".to_string(),
        (false, l) => labels.get(l - 1).cloned().unwrap_or_else(|| multiplier(levels[l - 1])),
    };
    Some(format!("{name}: {state}"))
}

/// Select `level` (1-based) of an option, or switch it off when that level is
/// already selected - a pill click and a shortcut behave the same.
pub fn press(shared: &Arc<Mutex<Shared>>, idx: usize, level: usize) {
    if let Ok(mut s) = shared.lock() {
        if let Some(l) = s.level.get_mut(idx) {
            *l = if *l == level { 0 } else { level };
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Sleep, but wake as soon as a shutdown is requested, so `shutdown()` never
/// waits out a full retry delay.
fn nap(quit: &AtomicBool, total: Duration) {
    let mut left = total;
    while !left.is_zero() && !quit.load(Ordering::Relaxed) {
        let step = left.min(TICK);
        std::thread::sleep(step);
        left -= step;
    }
}

/// Point the UI at a loaded script: its title, artwork and options.
fn publish(s: &mut Shared, script: &Script, level: Vec<usize>) {
    s.title = script.title.clone();
    s.names = script.options.iter().map(|o| o.name.clone()).collect();
    s.separators = script.options.iter().map(|o| o.separator.clone()).collect();
    s.shows = script.options.iter().map(|o| o.show.clone()).collect();
    s.levels = script.options.iter().map(|o| o.levels.clone()).collect();
    s.labels = script.options.iter().map(|o| o.labels.clone()).collect();
    s.keys = script.options.iter().map(|o| o.keys.clone()).collect();
    s.level = level;
    s.values = script.options.iter().map(|_| None).collect();
    s.columns = script.columns;
    s.art = art::resolve(script);
    s.config_info = Some(config_info(&script.path));
    s.config_rel = script
        .path
        .file_name()
        .map(|n| format!("games/{}", n.to_string_lossy()));
    s.ready = true;
}

/// `dawnwalker.js  ·  updated 2026-09-13  ·  sha256 1a2b3c4d5e6f`
///
/// The checksum covers the config file itself, so two people can tell at a
/// glance whether they are running the same script.
fn config_info(config: &Path) -> String {
    use sha2::Digest;
    let name = config
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let date = std::fs::metadata(config)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| ymd(d.as_secs()))
        .unwrap_or_else(|| "unknown".into());
    let sum = match std::fs::read(config) {
        Ok(bytes) => crate::update::hex(&sha2::Sha256::digest(&bytes))[..12].to_string(),
        Err(_) => "unreadable".into(),
    };
    format!("{name}  ·  updated {date}  ·  sha256 {sum}")
}

/// Seconds since the Unix epoch to a UTC `YYYY-MM-DD` (Hinnant's civil_from_days).
fn ymd(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}")
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
        s.config_info = None;
        s.config_rel = None;
        s.names.clear();
        s.separators.clear();
        s.shows.clear();
        s.levels.clear();
        s.labels.clear();
        s.keys.clear(); // releases the shortcuts for other programs
        s.level.clear();
        s.values.clear();
        s.columns = 1;
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
        if name.is_empty() {
            continue; // a separator, not a setting
        }
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
        if crate::update::syncing() {
            status(&shared, "downloading game scripts...");
            nap(&quit, Duration::from_millis(500));
            continue;
        }
        let paths = Script::discover();
        if paths.is_empty() {
            status(&shared, "downloading game scripts...");
            nap(&quit, Duration::from_secs(3));
            continue;
        }

        let mut chosen: Option<(Script, String, u32)> = None;
        let mut errors: Vec<String> = Vec::new();
        for p in &paths {
            match Script::load(p) {
                Ok(s) => {
                    let running = s.processes.iter().find_map(|exe| Some((exe.clone(), find_pid(exe)?)));
                    if let Some((exe, pid)) = running {
                        chosen = Some((s, exe, pid));
                        break;
                    }
                }
                Err(e) => errors.push(format!(
                    "{}: {e}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                )),
            }
        }

        let Some((script, exe, pid)) = chosen else {
            status(
                &shared,
                if errors.is_empty() {
                    "waiting for a supported game...".to_string()
                } else {
                    format!("config error - {}", errors.join("; "))
                },
            );
            nap(&quit, Duration::from_millis(1500));
            continue;
        };

        let Some(proc) = Proc::open(pid) else {
            status(&shared, "cannot open process - run as Administrator");
            nap(&quit, Duration::from_secs(2));
            continue;
        };
        let Some((base, size)) = proc.module(&exe) else {
            status(&shared, "module not found, retrying...");
            continue;
        };

        status(&shared, "scanning...");
        let identity = Identity::read(&proc, &exe, base);
        script.attach(proc, base, size);
        let build = script.identify(&identity);
        println!("[engine] {} is {} - {build:?}", exe, identity.describe());

        if build == Build::Unknown {
            // the config's addresses are for other builds: writing them here
            // would poke whatever this build keeps there instead
            println!("[engine] no build matches; {} lists: {}", script.title, script.builds.join(", "));
            script.detach();
            status(
                &shared,
                format!("unsupported game version ({}) - waiting for a script update", identity.short()),
            );
            let stamp = config_stamp(&script.path);
            while !quit.load(Ordering::Relaxed)
                && find_pid(&exe) == Some(pid)
                && config_stamp(&script.path) == stamp
            {
                nap(&quit, Duration::from_secs(1));
            }
            continue;
        }

        let saved = load_settings(&script.path);
        if let Ok(mut s) = shared.lock() {
            let level = script
                .options
                .iter()
                // nothing is on unless it was on last time: a trainer that
                // enables itself is a trainer that surprises you
                .map(|o| {
                    if o.once {
                        return 0; // one-shots always start off
                    }
                    pending
                        .get(&o.name)
                        .copied()
                        .or_else(|| saved.get(&o.name).copied())
                        .unwrap_or(0)
                })
                .collect();
            publish(&mut s, &script, level);
            if let (Build::Known(name), Some(info)) = (&build, s.config_info.as_mut()) {
                info.push_str(&format!("  ·  {name}"));
            }
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
            if find_pid(&exe) != Some(pid) {
                forget_game(&shared, "game closed - looking for a game...");
                script.detach();
                break;
            }

            // hot reload: pick up edits to the config or the shared lib
            since_check += 1;
            if since_check >= 10 {
                since_check = 0;
                let now = config_stamp(&script.path);
                // mid-sync the files are a mix of old and new: wait it out
                if now != stamp && !crate::update::syncing() {
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

            #[cfg(feature = "devtools")]
            crate::devtools::poll(&script);

            let levels: Vec<usize> = shared.lock().map(|s| s.level.clone()).unwrap_or_default();
            if levels != saved_levels {
                // one-shots are never saved, like separators
                let names: Vec<String> = script
                    .options
                    .iter()
                    .map(|o| if o.once { String::new() } else { o.name.clone() })
                    .collect();
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

                let (text, done) = script.tick(i, on, mult);
                if done && opt.once {
                    if let Ok(mut s) = shared.lock() {
                        if let Some(l) = s.level.get_mut(i) {
                            *l = 0;
                        }
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Speed (plain multipliers), a checkbox, and captioned levels.
    fn shortcut_state() -> Arc<Mutex<Shared>> {
        Arc::new(Mutex::new(Shared {
            names: vec!["Speed".into(), "Infinite Health".into(), "Denarius".into()],
            levels: vec![vec![2.0, 4.0, 8.0], vec![], vec![1000.0, 10000.0, 99999.0]],
            labels: vec![vec![], vec![], vec!["1k".into(), "10k".into(), "99k".into()]],
            level: vec![0, 0, 0],
            ..Default::default()
        }))
    }

    #[test]
    fn a_shortcut_selects_its_level_and_a_repeat_switches_it_off() {
        let s = shortcut_state();
        assert_eq!(describe(&s, 0).as_deref(), Some("Speed: 1x"), "off reads as 1x");
        press(&s, 0, 2);
        assert_eq!(describe(&s, 0).as_deref(), Some("Speed: 4x"));
        press(&s, 0, 3);
        assert_eq!(describe(&s, 0).as_deref(), Some("Speed: 8x"), "another level switches over");
        press(&s, 0, 3);
        assert_eq!(describe(&s, 0).as_deref(), Some("Speed: 1x"), "the same key again turns it off");
    }

    #[test]
    fn toasts_say_on_off_for_checkboxes_and_use_level_captions() {
        let s = shortcut_state();
        press(&s, 1, 1);
        assert_eq!(describe(&s, 1).as_deref(), Some("Infinite Health: ON"));
        press(&s, 1, 1);
        assert_eq!(describe(&s, 1).as_deref(), Some("Infinite Health: OFF"));
        press(&s, 2, 3);
        assert_eq!(describe(&s, 2).as_deref(), Some("Denarius: 99k"));
        press(&s, 2, 3);
        assert_eq!(describe(&s, 2).as_deref(), Some("Denarius: OFF"), "captioned levels do not read as 1x");
        assert_eq!(describe(&s, 9), None);
    }

    fn temp_config(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("az_trainer_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("game.js")
    }

    #[test]
    fn settings_round_trip_by_name() {
        let config = temp_config("settings");
        let names: Vec<String> = ["Infinite Health", "Speed", "Gold 99,999"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        save_settings(&config, &names, &[1, 3, 0]);

        let back = load_settings(&config);
        assert_eq!(back.get("Infinite Health"), Some(&1));
        assert_eq!(back.get("Speed"), Some(&3));
        assert_eq!(back.get("Gold 99,999"), Some(&0));
        assert_eq!(settings_path(&config).file_name().unwrap(), "game.settings");
        std::fs::remove_dir_all(config.parent().unwrap()).ok();
    }

    #[test]
    fn unreadable_settings_start_everything_off() {
        let config = temp_config("corrupt");
        assert!(load_settings(&config).is_empty(), "no file yet");

        std::fs::write(settings_path(&config), "garbage
Speed	not-a-number
Gold	2
").unwrap();
        let back = load_settings(&config);
        assert_eq!(back.len(), 1, "only the well-formed line survives");
        assert_eq!(back.get("Gold"), Some(&2));
        std::fs::remove_dir_all(config.parent().unwrap()).ok();
    }

    #[test]
    fn nap_wakes_immediately_on_shutdown() {
        let quit = AtomicBool::new(true);
        let started = std::time::Instant::now();
        nap(&quit, Duration::from_secs(10));
        assert!(started.elapsed() < Duration::from_millis(250));
    }
}
