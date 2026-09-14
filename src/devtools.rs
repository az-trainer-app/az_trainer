//! A live JS channel into the attached game.
//!
//! Drop a snippet in `%TEMP%/az_trainer_eval.js` and the engine evaluates it
//! in the config's own context - same `mem`, same attached process, with every
//! hook the config installed still in place - then writes the result to
//! `%TEMP%/az_trainer_eval.out`. That is the point: a hook can be installed,
//! left running, and read back later, which a one-shot tool cannot do.
//!
//! Compiled only with `--features devtools`, so a released build does not
//! carry the ability to evaluate arbitrary code against another process.

use std::path::PathBuf;

use crate::js::Script;

fn request() -> PathBuf {
    std::env::temp_dir().join("az_trainer_eval.js")
}

fn response() -> PathBuf {
    std::env::temp_dir().join("az_trainer_eval.out")
}

/// Run one queued snippet, if there is one. Called from the engine's tick.
pub fn poll(script: &Script) {
    let req = request();
    let Ok(src) = std::fs::read_to_string(&req) else {
        return;
    };
    // Taken before evaluating, so a snippet that panics or hangs is not run
    // again on the next tick.
    let _ = std::fs::remove_file(&req);
    if src.trim().is_empty() {
        return;
    }

    let started = std::time::Instant::now();
    let out = match script.eval(&src) {
        Ok(v) => format!("ok {:?}ms\n{v}\n", started.elapsed().as_millis()),
        Err(e) => format!("error {:?}ms\n{e}\n", started.elapsed().as_millis()),
    };
    let _ = std::fs::write(response(), out);
}
