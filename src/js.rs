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
    pub name: String,
    pub show: Option<String>,
    pub levels: Vec<f32>,
    /// optional pill captions; without these a level renders as "2x"
    pub labels: Vec<String>,
}

pub struct Script {
    _rt: Runtime,
    ctx: Context,
    /// the config file this was loaded from; artwork sits next to it
    pub path: PathBuf,
    pub process: String,
    pub title: String,
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
        let (process, title, art, options) = ctx.with(|ctx| {
            let module = Module::declare(ctx.clone(), name.as_str(), src)
                .catch(&ctx)
                .map_err(|e| format!("{e}"))?;
            let (module, promise) = module.eval().catch(&ctx).map_err(|e| format!("{e}"))?;
            promise.finish::<()>().catch(&ctx).map_err(|e| format!("{e}"))?;

            let process: String = module
                .get::<_, Value>("process")
                .ok()
                .and_then(|v| v.get::<String>().ok())
                .ok_or("config does not export `process`")?;
            let title: String = module
                .get::<_, Value>("title")
                .ok()
                .and_then(|v| v.get::<String>().ok())
                .unwrap_or_else(|| process.clone());
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
                let labels: Vec<String> = o
                    .get::<_, Value>("labels")
                    .ok()
                    .and_then(|v| v.into_array())
                    .map(|a| a.iter::<String>().flatten().collect())
                    .unwrap_or_default();
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
                    separator,
                });
            }
            Ok::<_, String>((process, title, art, metas))
        })?;

        Ok(Script { _rt: rt, ctx, path: path.to_path_buf(), process, title, art, options })
    }

    pub fn attach(&self, proc: Proc, base: u64, size: usize) {
        crate::hold::attach(proc.handle);
        TARGET.with(|t| *t.borrow_mut() = Some(Target { proc, base, size }));
    }

    pub fn detach(&self) {
        crate::hold::detach();
        TARGET.with(|t| *t.borrow_mut() = None);
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

    /// Run one option's `tick({on, mult})`. Returns its display string, if any.
    pub fn tick(&self, index: usize, on: bool, mult: f32) -> Option<String> {
        self.ctx.with(|ctx| {
            let opts: Value = ctx.globals().get("__options").ok()?;
            let arr = opts.into_array()?;
            let o: Object = arr.get(index).ok()?;
            let f: Function = o.get("tick").ok()?;

            let arg = Object::new(ctx.clone()).ok()?;
            arg.set("on", on).ok()?;
            arg.set("mult", mult).ok()?;

            match f.call::<_, Value>((rquickjs::function::This(o), arg)).catch(&ctx) {
                Ok(v) => v.get::<String>().ok(),
                Err(e) => {
                    eprintln!("tick error: {e}");
                    None
                }
            }
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

/// The `configs` folder in use: beside the exe when installed, otherwise the
/// nearest ancestor that has one.
///
/// The fallback is what lets a `target/release` build run straight from a
/// checkout, reading the repo's own `configs/` - so edits are tracked by git
/// and `cargo clean` cannot delete them.
pub fn configs_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .map(|d| d.join("configs"))
        .find(|c| c.join("games").is_dir())
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

            assert!(script.process.to_ascii_lowercase().ends_with(".exe"), "{file}: process");
            assert!(!script.title.is_empty(), "{file}: title");
            assert!(!script.options.is_empty(), "{file}: no options");

            let mut names = std::collections::HashSet::new();
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
            }
            loaded += 1;
        }
        assert!(loaded > 0, "no configs found in {}", games.display());
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
