//! Parse a game config with the real loader and report what it exports.
//!     cfgcheck <path-to-config.js>

#![allow(dead_code)]  // each bin pulls in whole modules

#[path = "../mem.rs"]
mod mem;
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

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: cfgcheck <config.js>");
        std::process::exit(2);
    };
    match js::Script::load(std::path::Path::new(&path)) {
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
