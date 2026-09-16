//! The JS layer: loads a game config, exposes memory primitives, runs ticks.
//!
//! Configs are ES modules under `configs/games/*.js` and may import shared
//! helpers (`configs/lib/unreal.js`). The host exposes only memory reads and
//! writes, so a config cannot touch the filesystem or the network.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use rquickjs::loader::{Loader, Resolver};
use rquickjs::loader::ImportAttributes;
use rquickjs::module::Declared;
use rquickjs::{CatchResultExt, Context, Ctx, Error, Function, Module, Object, Runtime, Value};

use crate::game::Identity;
use crate::mem::{Pattern, Proc};

/// The attached process, reachable from JS callbacks on this thread.
struct Target {
    proc: Proc,
    base: u64,
    size: usize,
}

thread_local! {
    static TARGET: RefCell<Option<Target>> = const { RefCell::new(None) };
}

fn with_target<T>(f: impl FnOnce(&Target) -> T, default: T) -> T {
    TARGET.with(|t| match t.borrow().as_ref() {
        Some(t) => f(t),
        None => default,
    })
}

#[derive(Debug, Clone)]
pub struct OptMeta {
    /// `Some` for a divider row: its heading, or empty for a plain line.
    pub separator: Option<String>,
    /// A one-shot action: switched back off once its tick returns `true`,
    /// and never saved, so it cannot fire again on a later attach or load.
    pub once: bool,
    pub name: String,
    pub show: Option<String>,
    pub levels: Vec<f32>,
    /// optional pill captions; without these a level renders as "2x"
    pub labels: Vec<String>,
    /// optional shortcuts, one per level (or one for a toggle), e.g. "Alt+F1"
    pub keys: Vec<String>,
}

pub struct Script {
    _rt: Runtime,
    ctx: Context,
    /// the config file this was loaded from; artwork sits next to it
    pub path: PathBuf,
    /// executables to attach to - several when builds ship under different names
    pub processes: Vec<String>,
    pub title: String,
    /// names of the builds the config lists; empty when it does not care
    pub builds: Vec<String>,
    /// option columns in the window: 1 or 2
    pub columns: usize,
    /// base64 image embedded in the config, used when Steam art is unavailable
    pub art: Option<String>,
    pub options: Vec<OptMeta>,
}

impl Script {
    /// Evaluate a config. Memory reads return 0 until `attach` is called, so a
    /// config's top level must not depend on the game being present.
    pub fn load(path: &Path) -> Result<Script, String> {
        let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

        let rt = Runtime::new().map_err(|e| e.to_string())?;
        rt.set_loader(FsResolver { root: dir.clone() }, FsLoader);

        let ctx = Context::full(&rt).map_err(|e| e.to_string())?;

        register_host(&ctx)?;

        let name = path.to_string_lossy().replace('\\', "/");
        let (processes, title, art, options, builds, columns) = ctx.with(|ctx| {
            let module = Module::declare(ctx.clone(), name.as_str(), src)
                .catch(&ctx)
                .map_err(|e| format!("{e}"))?;
            let (module, promise) = module.eval().catch(&ctx).map_err(|e| format!("{e}"))?;
            promise.finish::<()>().catch(&ctx).map_err(|e| format!("{e}"))?;

            // Research builds get lib/research.js as the `research` global, so
            // an eval snippet can call it without an import of its own.
            // Declared beside the config, so the relative path resolves the
            // way the config's own imports do. Optional: a missing lib only
            // means no helpers.
            #[cfg(feature = "devtools")]
            {
                let src = "import * as research from '../lib/research.js'; globalThis.research = research;";
                let loaded = Module::declare(ctx.clone(), format!("{name}#research"), src)
                    .and_then(|m| m.eval())
                    .and_then(|(_, p)| p.finish::<()>());
                if let Err(e) = loaded {
                    eprintln!("research helpers not loaded: {e}");
                }
            }

            // `'Game.exe'` or `['Game.exe', 'Game-WinGDK.exe']`
            let processes: Vec<String> = match module.get::<_, Value>("process") {
                Ok(v) if v.is_string() => v.get::<String>().ok().into_iter().collect(),
                Ok(v) if v.is_array() => v
                    .into_array()
                    .map(|a| a.iter::<String>().flatten().collect())
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            if processes.is_empty() {
                return Err("config does not export `process`".to_string());
            }
            let title: String = module
                .get::<_, Value>("title")
                .ok()
                .and_then(|v| v.get::<String>().ok())
                .unwrap_or_else(|| processes[0].clone());

            let columns = match module.get::<_, Value>("columns") {
                Ok(v) if v.is_undefined() => 1,
                Ok(v) => match v.as_number() {
                    Some(n) if n == 1.0 || n == 2.0 => n as usize,
                    _ => return Err("`columns` must be 1 or 2".to_string()),
                },
                Err(_) => 1,
            };

            let builds: Vec<String> = match module.get::<_, Value>("builds") {
                Ok(v) if v.is_undefined() => Vec::new(),
                Ok(v) => {
                    let arr = v.into_array().ok_or("`builds` must be an array")?;
                    let mut names = Vec::new();
                    for i in 0..arr.len() {
                        let b: Object = arr.get(i).map_err(|_| format!("builds[{i}] must be an object"))?;
                        if let Ok(m) = b.get::<_, Object>("match") {
                            for key in m.keys::<String>().flatten() {
                                if !MATCH_KEYS.contains(&key.as_str()) {
                                    return Err(format!(
                                        "builds[{i}].match: unknown key `{key}` (use {})",
                                        MATCH_KEYS.join(", ")
                                    ));
                                }
                            }
                        }
                        names.push(build_name(&b, i));
                    }
                    ctx.globals().set("__builds", arr).map_err(|e| e.to_string())?;
                    names
                }
                Err(_) => Vec::new(),
            };
            let art: Option<String> = module
                .get::<_, Value>("art")
                .ok()
                .and_then(|v| v.get::<String>().ok());

            // keep the options array alive on the global object for later ticks
            let options: Value = module
                .get::<_, Value>("options")
                .map_err(|_| "config does not export `options`")?;
            ctx.globals()
                .set("__options", options.clone())
                .map_err(|e| e.to_string())?;
            if let Ok(f) = module.get::<_, Value>("live") {
                let _ = ctx.globals().set("__live", f);
            }

            let arr = options.into_array().ok_or("`options` must be an array")?;
            let mut metas = Vec::new();
            for i in 0..arr.len() {
                let o: Object = arr.get(i).map_err(|e| e.to_string())?;
                let levels: Vec<f32> = o
                    .get::<_, Value>("levels")
                    .ok()
                    .and_then(|v| v.into_array())
                    .map(|a| a.iter::<f32>().flatten().collect())
                    .unwrap_or_default();
                let strings = |field: &str| -> Vec<String> {
                    o.get::<_, Value>(field)
                        .ok()
                        .and_then(|v| v.into_array())
                        .map(|a| a.iter::<String>().flatten().collect())
                        .unwrap_or_default()
                };
                let labels = strings("labels");
                let keys = strings("keys");
                // `{ separator: 'Title' }` or `{ separator: true }`
                let separator = match o.get::<_, Value>("separator") {
                    Ok(v) if v.is_string() => v.get::<String>().ok(),
                    Ok(v) if v.as_bool() == Some(true) => Some(String::new()),
                    _ => None,
                };
                metas.push(OptMeta {
                    // a separator has no name, so it is never saved as a setting
                    name: if separator.is_some() {
                        String::new()
                    } else {
                        o.get("name").unwrap_or_else(|_| format!("option {i}"))
                    },
                    show: o.get::<_, String>("show").ok(),
                    levels,
                    labels,
                    keys,
                    separator,
                    once: o.get::<_, bool>("once").unwrap_or(false),
                });
            }
            Ok::<_, String>((processes, title, art, metas, builds, columns))
        })?;

        Ok(Script { _rt: rt, ctx, path: path.to_path_buf(), processes, title, builds, columns, art, options })
    }

    /// Tell the config which game is running: set the `game` global, and pick
    /// the first of its `builds` whose `match` fits.
    ///
    /// A build with no `match` fits anything, so a config can end its list
    /// with a fallback. Without `builds` at all every game is accepted.
    pub fn identify(&self, id: &Identity) -> Build {
        self.ctx.with(|ctx| {
            let run = || -> rquickjs::Result<Build> {
                let game = Object::new(ctx.clone())?;
                game.set("exe", id.exe.as_str())?;
                game.set("timestamp", id.timestamp as f64)?;
                game.set("size", id.size as f64)?;
                game.set("steamBuild", id.steam_build.map(|b| b as f64))?;
                game.set("build", Value::new_null(ctx.clone()))?;
                ctx.globals().set("game", game.clone())?;

                let Ok(builds) = ctx.globals().get::<_, rquickjs::Array>("__builds") else {
                    return Ok(Build::Any);
                };
                for i in 0..builds.len() {
                    let b: Object = builds.get(i)?;
                    if b.get::<_, Object>("match").map_or(true, |m| fits(&m, id)) {
                        game.set("build", b.clone())?;
                        return Ok(Build::Known(build_name(&b, i)));
                    }
                }
                Ok(Build::Unknown)
            };
            run().unwrap_or(Build::Unknown)
        })
    }

    pub fn attach(&self, proc: Proc, base: u64, size: usize) {
        crate::hold::attach(proc.handle);
        TARGET.with(|t| *t.borrow_mut() = Some(Target { proc, base, size }));
    }

    pub fn detach(&self) {
        crate::hold::detach();
        TARGET.with(|t| *t.borrow_mut() = None);
    }

    /// Evaluate a snippet in the config's own context.
    ///
    /// Same `mem`, same attached process, same hooks: research can install
    /// something, leave it running, and come back for what it recorded.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn eval(&self, src: &str) -> Result<String, String> {
        self.ctx.with(|ctx| {
            let v: Value = ctx.eval(src).catch(&ctx).map_err(|e| format!("{e}"))?;
            if let Some(s) = v.as_string().and_then(|s| s.to_string().ok()) {
                return Ok(s);
            }
            let text = ctx
                .globals()
                .get::<_, Object>("JSON")
                .ok()
                .and_then(|j| j.get::<_, Function>("stringify").ok())
                .and_then(|f| f.call::<_, String>((v.clone(),)).ok());
            Ok(text.unwrap_or_else(|| "undefined".into()))
        })
    }

    /// Ask the config whether the game is in a playable state, if it says.
    /// Without this the engine can only guess from whether an option produced
    /// a display string - which is always false once readings are turned off.
    pub fn live(&self) -> Option<bool> {
        self.ctx.with(|ctx| {
            let f: Function = ctx.globals().get::<_, Function>("__live").ok()?;
            f.call::<_, bool>(()).ok()
        })
    }

    /// Run one option's `tick({on, mult})`. Returns its display string, if
    /// any, and whether it returned `true` - a one-shot saying it is done.
    pub fn tick(&self, index: usize, on: bool, mult: f32) -> (Option<String>, bool) {
        self.ctx.with(|ctx| {
            let run = || -> Option<(Option<String>, bool)> {
                let opts: Value = ctx.globals().get("__options").ok()?;
                let arr = opts.into_array()?;
                let o: Object = arr.get(index).ok()?;
                let f: Function = o.get("tick").ok()?;

                let arg = Object::new(ctx.clone()).ok()?;
                arg.set("on", on).ok()?;
                arg.set("mult", mult).ok()?;

                match f.call::<_, Value>((rquickjs::function::This(o), arg)).catch(&ctx) {
                    Ok(v) => Some((v.get::<String>().ok(), v.as_bool() == Some(true))),
                    Err(e) => {
                        eprintln!("tick error: {e}");
                        None
                    }
                }
            };
            run().unwrap_or((None, false))
        })
    }

    /// Every `configs/games/*.js` beside the executable.
    pub fn discover() -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Some(dir) = configs_dir().map(|c| c.join("games")) else { return out };
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().map(|x| x.eq_ignore_ascii_case("js")) == Some(true) {
                    out.push(p);
                }
            }
        }
        out
    }
}

/// What a config's `builds` make of the running game.
#[derive(Debug, Clone, PartialEq)]
pub enum Build {
    /// the config lists no builds, so it takes any
    Any,
    /// this entry of `builds`, by name
    Known(String),
    /// the config lists builds and this game is none of them
    Unknown,
}

/// Fields a build's `match` may test; anything else is a typo that would
/// otherwise make the entry match every build.
const MATCH_KEYS: [&str; 4] = ["exe", "timestamp", "size", "steamBuild"];

/// `name`, or `build 2` for the second entry without one.
fn build_name(b: &Object, i: usize) -> String {
    b.get::<_, String>("name").unwrap_or_else(|_| format!("build {}", i + 1))
}

/// Whether every field `m` gives agrees with the running game.
fn fits(m: &Object, id: &Identity) -> bool {
    // numbers may be written as numbers or strings ("24769601", "0x68a1b2c3")
    let number = |key: &str| -> Option<Option<f64>> {
        let v: Value = m.get(key).ok()?;
        if v.is_undefined() {
            return None;
        }
        Some(v.as_number().or_else(|| {
            let s = v.as_string()?.to_string().ok()?;
            let s = s.trim();
            match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(hex) => u64::from_str_radix(hex, 16).ok().map(|n| n as f64),
                None => s.parse().ok(),
            }
        }))
    };
    let agrees = |key: &str, actual: Option<f64>| match number(key) {
        None => true,
        Some(want) => want.is_some() && want == actual,
    };
    let exe_ok = match m.get::<_, Value>("exe") {
        Ok(v) if !v.is_undefined() => v
            .as_string()
            .and_then(|s| s.to_string().ok())
            .is_some_and(|e| e.eq_ignore_ascii_case(&id.exe)),
        _ => true,
    };
    exe_ok
        && agrees("timestamp", Some(id.timestamp as f64))
        && agrees("size", Some(id.size as f64))
        && agrees("steamBuild", id.steam_build.map(|b| b as f64))
}

/// The `configs` folder in use: the nearest one beside the exe or above it,
/// else a new `configs` beside the exe for the updater to download into.
///
/// Searching upwards is what lets a `target/release` build run straight from
/// a checkout, reading the repo's own `configs/` - so edits are tracked by git
/// and `cargo clean` cannot delete them. A released exe ships alone and fills
/// its own `configs` on first start.
pub fn configs_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .map(|d| d.join("configs"))
        .find(|c| c.join("games").is_dir())
        .or_else(|| Some(exe.parent()?.join("configs")))
}

/// Strip Windows' `\\?\` verbatim prefix and normalise separators.
fn tidy(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_string()
}

/// Resolves `./x.js` and `../lib/x.js` against the importing module's directory.
/// Bare names resolve against the config root. Everything stays inside `configs/`.
struct FsResolver {
    root: PathBuf,
}

impl Resolver for FsResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attrs: Option<ImportAttributes<'js>>,
    ) -> Result<String, Error> {
        let joined = if name.starts_with('.') {
            Path::new(base)
                .parent()
                .unwrap_or(Path::new("."))
                .join(name)
        } else {
            self.root.join(name)
        };
        // canonicalize collapses `..`; fall back to the raw join if it is missing
        let path = std::fs::canonicalize(&joined).unwrap_or(joined);
        if !path.is_file() {
            return Err(Error::new_resolving(base.to_string(), name.to_string()));
        }
        Ok(tidy(&path))
    }
}

struct FsLoader;

impl Loader for FsLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &str,
        _attrs: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>, Error> {
        let src = std::fs::read_to_string(path)
            .map_err(|_| Error::new_loading(path.to_string()))?;
        Module::declare(ctx.clone(), path, src)
    }
}

/// A float scan being narrowed across several calls - a first scan, then next
/// scans as the value is made to change in the game. Held here because each
/// step is a separate trip through the devtools channel.
static VALUE_SCAN: std::sync::Mutex<Vec<crate::mem::ValueScan>> = std::sync::Mutex::new(Vec::new());

/// How many candidates a first scan may keep: 8 bytes each, so ~320 MB.
const SCAN_LIMIT: usize = 40_000_000;

/// How many pointers one findPointers call may return.
const POINTER_LIMIT: usize = 1_000_000;

/// Install `mem.*` and `log()`.
fn register_host(ctx: &Context) -> Result<(), String> {
    ctx.with(|ctx| {
        let mem = Object::new(ctx.clone()).map_err(|e| e.to_string())?;

        mem.set(
            "u64",
            Function::new(ctx.clone(), |addr: f64| -> f64 {
                with_target(|t| t.proc.read_u64(addr as u64).unwrap_or(0) as f64, 0.0)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "i32",
            Function::new(ctx.clone(), |addr: f64| -> f64 {
                with_target(|t| t.proc.read_i32(addr as u64).unwrap_or(0) as f64, 0.0)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "f32",
            Function::new(ctx.clone(), |addr: f64| -> f64 {
                with_target(|t| t.proc.read_f32(addr as u64).unwrap_or(0.0) as f64, 0.0)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "writeF32",
            Function::new(ctx.clone(), |addr: f64, v: f64| -> bool {
                with_target(|t| t.proc.write_f32(addr as u64, v as f32), false)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "aob",
            Function::new(ctx.clone(), |sig: String| -> f64 {
                with_target(
                    |t| match Pattern::parse(&sig) {
                        Some(p) => t.proc.scan(t.base, t.size, &p).unwrap_or(0) as f64,
                        None => 0.0,
                    },
                    0.0,
                )
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "rip",
            Function::new(ctx.clone(), |hit: f64, pos: f64, len: f64| -> f64 {
                with_target(
                    |t| match t.proc.read_i32(hit as u64 + pos as u64) {
                        Some(rel) => {
                            ((hit as u64 + len as u64).wrapping_add(rel as i64 as u64)) as f64
                        }
                        None => 0.0,
                    },
                    0.0,
                )
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // [base, size, protect, type] for the region an address is in, or []
        // if it is not committed. `protect & 0xF0` covers the executable
        // values (0x10 EXECUTE, 0x20 EXECUTE_READ, 0x40 EXECUTE_READWRITE,
        // 0x80 EXECUTE_WRITECOPY).
        mem.set(
            "region",
            Function::new(ctx.clone(), |addr: f64| -> Vec<f64> {
                with_target(
                    |t| match t.proc.region_of(addr as u64) {
                        Some((b, s, p, ty)) => vec![b as f64, s as f64, p as f64, ty as f64],
                        None => Vec::new(),
                    },
                    Vec::new(),
                )
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // Value search. scanStart(lo, hi[, type]) keeps every value in range -
        // "float" (the default), "int", or "any" for both. Each
        // scanNext(mode[, a[, b]]) re-reads the survivors and keeps those that
        // "decreased", "increased", stayed "unchanged", "changed", are "equal"
        // to a (within b, default 0.01), or lie "between" a and b. Both return
        // how many are left, or -1 for an unknown type or mode.
        mem.set(
            "scanStart",
            Function::new(
                ctx.clone(),
                |lo: f64, hi: f64, kind: rquickjs::function::Opt<String>| -> f64 {
                    let (floats, ints) = match kind.0.as_deref().unwrap_or("float") {
                        "float" => (true, false),
                        "int" => (false, true),
                        "any" => (true, true),
                        _ => return -1.0,
                    };
                    with_target(
                        |t| {
                            let scan = t.proc.snapshot(lo, hi, floats, ints, SCAN_LIMIT);
                            let n: usize = scan.iter().map(|s| s.idx.len()).sum();
                            if let Ok(mut g) = VALUE_SCAN.lock() {
                                *g = scan;
                            }
                            n as f64
                        },
                        0.0,
                    )
                },
            )
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "scanNext",
            Function::new(
                ctx.clone(),
                |mode: String, a: rquickjs::function::Opt<f64>, b: rquickjs::function::Opt<f64>| -> f64 {
                    let (a, b) = (a.0.unwrap_or(0.0), b.0);
                    let keep: Box<dyn Fn(f64, f64) -> bool> = match mode.as_str() {
                        "decreased" => Box::new(|was, now| now < was),
                        "increased" => Box::new(|was, now| now > was),
                        "unchanged" => Box::new(|was, now| now == was),
                        "changed" => Box::new(|was, now| now != was),
                        "equal" => {
                            let tol = b.unwrap_or(0.01);
                            Box::new(move |_, now| (now - a).abs() <= tol)
                        }
                        "between" => {
                            let (lo, hi) = (a.min(b.unwrap_or(a)), a.max(b.unwrap_or(a)));
                            Box::new(move |_, now| now >= lo && now <= hi)
                        }
                        _ => return -1.0,
                    };
                    with_target(
                        |t| match VALUE_SCAN.lock() {
                            Ok(mut g) => t.proc.refine(&mut g, keep) as f64,
                            Err(_) => 0.0,
                        },
                        0.0,
                    )
                },
            )
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // Up to n survivors as a flat [addr, value, isInt, ...].
        mem.set(
            "scanResults",
            Function::new(ctx.clone(), |n: f64| -> Vec<f64> {
                let mut out = Vec::new();
                if let Ok(g) = VALUE_SCAN.lock() {
                    'regions: for s in g.iter() {
                        for (k, &i) in s.idx.iter().enumerate() {
                            if out.len() / 3 >= n as usize {
                                break 'regions;
                            }
                            out.push((s.base + i as u64 * 4) as f64);
                            out.push(s.value(s.raw[k]));
                            out.push(if s.int { 1.0 } else { 0.0 });
                        }
                    }
                }
                out
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // Every 8-byte slot anywhere in readable memory holding a value in
        // [lo, hi), as a flat [at, value, ...] - the step a reverse pointer
        // search (lib/research.js staticPaths) repeats.
        mem.set(
            "findPointers",
            Function::new(ctx.clone(), |lo: f64, hi: f64, limit: rquickjs::function::Opt<f64>| -> Vec<f64> {
                let limit = limit.0.map_or(POINTER_LIMIT, |l| (l as usize).min(POINTER_LIMIT));
                with_target(
                    |t| {
                        t.proc
                            .find_pointers(lo as u64, hi as u64, limit)
                            .into_iter()
                            .flat_map(|(at, v)| [at as f64, v as f64])
                            .collect()
                    },
                    Vec::new(),
                )
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // A timer is the one thing that falls by the wall-clock time between
        // two reads, so this finds one without knowing anything else about it.
        // Returns a flat [addr, from, to, ...]; the sweep is native because a
        // JS pass over gigabytes would not finish.
        mem.set(
            "findFalling",
            Function::new(
                ctx.clone(),
                |lo: f64, hi: f64, ms: f64, limit: f64| -> Vec<f64> {
                    with_target(
                        |t| {
                            let mut out = Vec::new();
                            for (a, from, to) in
                                t.proc.find_falling(lo as f32, hi as f32, ms as u64, limit as usize)
                            {
                                out.push(a as f64);
                                out.push(from as f64);
                                out.push(to as f64);
                            }
                            out
                        },
                        Vec::new(),
                    )
                },
            )
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // ---- code injection primitives ----------------------------------
        mem.set(
            "alloc",
            Function::new(ctx.clone(), |size: f64, near: f64| -> f64 {
                with_target(|t| t.proc.alloc_near(size as usize, near as u64) as f64, 0.0)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "free",
            Function::new(ctx.clone(), |addr: f64| -> bool {
                with_target(|t| t.proc.free(addr as u64), false)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "readBytes",
            Function::new(ctx.clone(), |addr: f64, n: f64| -> Vec<u8> {
                with_target(
                    |t| t.proc.read_bytes(addr as u64, n as usize).unwrap_or_default(),
                    Vec::new(),
                )
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "writeBytes",
            Function::new(ctx.clone(), |addr: f64, data: Vec<u8>| -> bool {
                with_target(|t| t.proc.write_bytes(addr as u64, &data), false)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "protect",
            Function::new(ctx.clone(), |addr: f64, size: f64| -> f64 {
                with_target(|t| t.proc.protect_rwx(addr as u64, size as usize) as f64, 0.0)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // ---- hardware-breakpoint accessor finder -------------------------
        mem.set(
            "findAccessors",
            Function::new(
                ctx.clone(),
                |addr: f64, size: f64, access: f64, ms: f64| -> Vec<String> {
                    with_target(
                        |t| {
                            match crate::finder::find_accessors(
                                &t.proc,
                                t.proc.pid,
                                addr as u64,
                                size as usize,
                                access as u64,
                                ms as u32,
                                (t.base, t.size),
                            ) {
                                Ok(hits) => hits
                                    .iter()
                                    .take(12)
                                    .map(|h| {
                                        let off = match h.module_offset {
                                            Some(o) => format!("module+0x{o:X}"),
                                            None => "outside main module".into(),
                                        };
                                        let before: Vec<String> = h
                                            .before
                                            .iter()
                                            .rev()
                                            .take(12)
                                            .rev()
                                            .map(|b| format!("{b:02X}"))
                                            .collect();
                                        let at: Vec<String> =
                                            h.at.iter().take(8).map(|b| format!("{b:02X}")).collect();
                                        format!(
                                            "hits={:<5} rip=0x{:X} ({})  before=[{}]  at=[{}]",
                                            h.hits,
                                            h.rip,
                                            off,
                                            before.join(" "),
                                            at.join(" ")
                                        )
                                    })
                                    .collect(),
                                Err(e) => vec![format!("finder failed: {e}")],
                            }
                        },
                        vec!["no process attached".to_string()],
                    )
                },
            )
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "moduleBase",
            Function::new(ctx.clone(), || -> f64 { with_target(|t| t.base as f64, 0.0) })
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "moduleSize",
            Function::new(ctx.clone(), || -> f64 { with_target(|t| t.size as f64, 0.0) })
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // The thread the process started on. For Unreal that is the game
        // thread, the only one allowed to touch UObjects - code caves compare
        // against it. Chosen by creation time, because thread enumeration
        // order drifts once a game has been running a while.
        mem.set(
            "mainThreadId",
            Function::new(ctx.clone(), || -> f64 {
                with_target(|t| crate::finder::main_thread_id(t.proc.pid) as f64, 0.0)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // Hold a float at high frequency. Needed for values the game rewrites
        // every frame - writing those from the 10Hz tick loop only flickers.
        mem.set(
            "hold",
            Function::new(ctx.clone(), |addr: f64, val: f64| {
                crate::hold::set(vec![(addr as u64, crate::hold::Mode::Set(val as f32))]);
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // Multiply whatever the game writes, preserving its own tiers.
        mem.set(
            "holdScale",
            Function::new(ctx.clone(), |addr: f64, mult: f64| {
                crate::hold::set(vec![(addr as u64, crate::hold::Mode::Scale(mult as f32))]);
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        mem.set(
            "clearHolds",
            Function::new(ctx.clone(), || crate::hold::clear())
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        // All matches, not just the first: some patch sites are only
        // identifiable by inspecting each candidate's operands.
        mem.set(
            "aobAll",
            Function::new(ctx.clone(), |sig: String, limit: f64| -> Vec<f64> {
                with_target(
                    |t| match Pattern::parse(&sig) {
                        Some(p) => t
                            .proc
                            .scan_all(t.base, t.size, &p, limit as usize)
                            .into_iter()
                            .map(|a| a as f64)
                            .collect(),
                        None => Vec::new(),
                    },
                    Vec::new(),
                )
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        ctx.globals().set("mem", mem).map_err(|e| e.to_string())?;
        ctx.globals()
            .set(
                "log",
                Function::new(ctx.clone(), |msg: String| {
                    use std::io::Write;
                    println!("[js] {msg}");
                    let _ = std::io::stdout().flush();   // unbuffer: logs are
                                                          // useless if they sit
                                                          // in a pipe buffer
                })
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped config must load in the real QuickJS host - not just
    /// parse under Node - and satisfy what the engine relies on.
    #[test]
    #[cfg(feature = "devtools")]
    fn research_builds_expose_the_research_helpers() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs").join("games").join("innocence.js");
        let script = Script::load(&path).unwrap();
        assert_eq!(script.eval("typeof research.staticPaths").unwrap(), "function");
        assert_eq!(script.eval("typeof research.changes").unwrap(), "function");
    }

    #[test]
    fn every_game_config_loads() {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs").join("games");
        let mut loaded = 0;
        for entry in std::fs::read_dir(&games).expect("configs/games") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("js") {
                continue;
            }
            let file = path.display().to_string();
            let script = Script::load(&path).unwrap_or_else(|e| panic!("{file}: {e}"));

            for exe in &script.processes {
                assert!(exe.to_ascii_lowercase().ends_with(".exe"), "{file}: process {exe:?}");
            }
            assert!(!script.title.is_empty(), "{file}: title");
            assert!(!script.options.is_empty(), "{file}: no options");

            let mut names = std::collections::HashSet::new();
            let mut taken = std::collections::HashSet::new();
            for o in &script.options {
                if o.separator.is_some() {
                    continue;
                }
                assert!(
                    names.insert(o.name.clone()),
                    "{file}: duplicate option {:?} - settings are saved by name",
                    o.name
                );
                assert!(
                    o.labels.is_empty() || o.labels.len() == o.levels.len(),
                    "{file}: {:?} has {} labels for {} levels",
                    o.name,
                    o.labels.len(),
                    o.levels.len()
                );
                assert!(
                    o.keys.is_empty() || o.keys.len() == o.levels.len().max(1),
                    "{file}: {:?} has {} keys for {} levels",
                    o.name,
                    o.keys.len(),
                    o.levels.len()
                );
                for k in &o.keys {
                    assert!(crate::keys::parse(k).is_some(), "{file}: {:?} bad key {k:?}", o.name);
                    assert!(taken.insert(k.to_ascii_lowercase()), "{file}: key {k:?} used twice");
                }
            }
            loaded += 1;
        }
        assert!(loaded > 0, "no configs found in {}", games.display());
    }

    fn temp_script(tag: &str, src: &str) -> (Script, PathBuf) {
        let dir = std::env::temp_dir().join(format!("az_trainer_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}.js"));
        std::fs::write(&path, src).unwrap();
        (Script::load(&path).unwrap(), dir)
    }

    fn eval(script: &Script, src: &str) -> String {
        script.ctx.with(|ctx| {
            let v: Value = ctx.eval(src).unwrap();
            v.get::<String>().unwrap_or_else(|_| format!("{v:?}"))
        })
    }

    #[test]
    fn builds_pick_the_first_entry_that_fits_the_running_game() {
        let (script, dir) = temp_script(
            "builds",
            "export const process = ['Game.exe', 'Game-WinGDK.exe'];\n\
             export const title = 'X';\n\
             export const builds = [\n\
               { name: 'Steam 100', match: { steamBuild: 100 }, player: 0x10 },\n\
               { name: 'Game Pass', match: { exe: 'game-wingdk.exe', timestamp: '0x6000' }, player: 0x20 },\n\
               { match: { timestamp: 0x7000, size: 0x9000 }, player: 0x30 },\n\
             ];\n\
             export const options = [{ name: 'A', tick() { return String(game.build && game.build.player); } }];\n",
        );
        assert_eq!(script.processes, vec!["Game.exe", "Game-WinGDK.exe"]);
        assert_eq!(script.builds, vec!["Steam 100", "Game Pass", "build 3"]);

        let id = |exe: &str, timestamp, size, steam_build| Identity {
            exe: exe.into(),
            timestamp,
            size,
            steam_build,
        };
        assert_eq!(script.identify(&id("Game.exe", 1, 1, Some(100))), Build::Known("Steam 100".into()));
        assert_eq!(script.tick(0, true, 1.0).0.as_deref(), Some("16"));
        assert_eq!(eval(&script, "String(game.steamBuild)"), "100");

        assert_eq!(script.identify(&id("Game-WinGDK.exe", 0x6000, 1, None)), Build::Known("Game Pass".into()));
        assert_eq!(script.identify(&id("Game.exe", 0x6000, 1, None)), Build::Unknown, "exe must agree too");
        assert_eq!(script.identify(&id("Game.exe", 0x7000, 0x9000, None)), Build::Known("build 3".into()));
        assert_eq!(script.identify(&id("Game.exe", 0x7000, 0x9001, None)), Build::Unknown, "every field must agree");
        assert_eq!(eval(&script, "String(game.build)"), "null", "no stale build after a miss");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_build_without_match_is_a_fallback_and_no_builds_accepts_anything() {
        let (fallback, d1) = temp_script(
            "fallback",
            "export const process = 'X.exe';\n\
             export const builds = [{ name: 'v1', match: { timestamp: 1 } }, { name: 'other' }];\n\
             export const options = [];\n",
        );
        let id = Identity { exe: "X.exe".into(), timestamp: 2, ..Default::default() };
        assert_eq!(fallback.identify(&id), Build::Known("other".into()));

        let (plain, d2) = temp_script("plain", "export const process = 'X.exe';\nexport const options = [];\n");
        assert_eq!(plain.identify(&id), Build::Any);
        assert_eq!(eval(&plain, "game.exe"), "X.exe");
        std::fs::remove_dir_all(&d1).ok();
        std::fs::remove_dir_all(&d2).ok();
    }

    #[test]
    fn a_misspelt_match_key_is_an_error_not_a_match_for_everything() {
        let dir = std::env::temp_dir().join(format!("az_trainer_typo_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("typo.js");
        std::fs::write(
            &path,
            "export const process = 'X.exe';\n\
             export const builds = [{ match: { timeStamp: 1 } }];\n\
             export const options = [];\n",
        )
        .unwrap();
        let err = Script::load(&path).err().expect("load should fail");
        assert!(err.contains("timeStamp"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn columns_default_to_one_and_only_one_or_two_load() {
        let (one, d1) = temp_script("cols1", "export const process = 'X.exe';\nexport const options = [];\n");
        let (two, d2) = temp_script(
            "cols2",
            "export const process = 'X.exe';\nexport const columns = 2;\nexport const options = [];\n",
        );
        assert_eq!((one.columns, two.columns), (1, 2));

        let dir = std::env::temp_dir().join(format!("az_trainer_cols3_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cols3.js");
        std::fs::write(&path, "export const process = 'X.exe';\nexport const columns = 3;\nexport const options = [];\n").unwrap();
        assert!(Script::load(&path).is_err());
        for d in [d1, d2, dir] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn separators_parse_with_and_without_a_title() {
        let dir = std::env::temp_dir().join(format!("az_trainer_sep_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sep.js");
        std::fs::write(
            &path,
            "export const process = 'X.exe';\n\
             export const title = 'X';\n\
             export const options = [\n\
               { separator: 'Combat' },\n\
               { name: 'A', tick() {} },\n\
               { separator: true },\n\
               { name: 'B', tick() {} },\n\
             ];\n",
        )
        .unwrap();

        let script = Script::load(&path).unwrap();
        let kinds: Vec<Option<String>> =
            script.options.iter().map(|o| o.separator.clone()).collect();
        assert_eq!(kinds, vec![Some("Combat".into()), None, Some(String::new()), None]);
        assert_eq!(script.options[1].name, "A");
        assert_eq!(script.options[0].name, "", "separators are never saved");
        std::fs::remove_dir_all(&dir).ok();
    }
}
