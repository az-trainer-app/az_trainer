//! Parse a game config with the real loader and report what it exports.
//!     cfgcheck <path-to-config.js>
//!     cfgcheck --json configs/games        # every config, as JSON
//!
//! The JSON form is what the website's per-game pages are built from, so the
//! options listed there are the ones the trainer really offers.

#![allow(dead_code)]  // each bin pulls in whole modules

#[path = "../mem.rs"]
mod mem;
#[cfg(feature = "devtools")]
#[path = "../dev.rs"]
mod dev;
#[path = "../finder.rs"]
mod finder;
#[path = "../game.rs"]
mod game;
#[path = "../hold.rs"]
mod hold;
#[path = "../js.rs"]
mod js;
#[path = "../keys.rs"]
mod keys;
#[path = "../art.rs"]
mod art;

use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let Some(path) = args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from) else {
        eprintln!("usage: cfgcheck [--json] <config.js | directory>");
        std::process::exit(2);
    };
    if json {
        emit_json(&path);
    } else {
        report(&path);
    }
}

fn report(path: &Path) {
    match js::Script::load(path) {
        Ok(s) => {
            println!("OK  process={}  title={}", s.processes.join(" | "), s.title);
            if !s.builds.is_empty() {
                println!("    builds: {}", s.builds.join(", "));
            }
            println!("    art={:?}", crate::art::resolve(&s));
            println!("    {} options:", s.options.len());
            for o in &s.options {
                println!(
                    "      {:<22} show={:<9} levels={:?} labels={:?}",
                    o.name,
                    o.show.clone().unwrap_or_else(|| "-".into()),
                    o.levels,
                    o.labels
                );
            }
        }
        Err(e) => {
            println!("PARSE FAILED: {e}");
            std::process::exit(1);
        }
    }
}

/// Every config in a directory (or one file) as a JSON array.
fn emit_json(path: &Path) {
    let mut files: Vec<PathBuf> = if path.is_dir() {
        std::fs::read_dir(path)
            .unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "js"))
            .collect()
    } else {
        vec![path.to_path_buf()]
    };
    files.sort();

    let games: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            let s = js::Script::load(f)
                .unwrap_or_else(|e| fail(&format!("{}: PARSE FAILED: {e}", f.display())));
            let id = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let options: Vec<serde_json::Value> = s
                .options
                .iter()
                .map(|o| {
                    serde_json::json!({
                        "separator": o.separator,
                        "name": o.name,
                        "once": o.once,
                        "levels": o.levels,
                        "labels": o.labels,
                        "keys": o.keys,
                    })
                })
                .collect();
            serde_json::json!({
                "id": id,
                "title": s.title,
                "processes": s.processes,
                "builds": s.builds,
                "columns": s.columns,
                "options": options,
            })
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&games).unwrap());
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}
