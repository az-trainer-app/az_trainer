//! Updates from GitHub: the app itself from Releases, trainer scripts from
//! the repository's `configs/` folder.
//!
//! The repository is private, so requests carry a token when one is
//! available (`AZ_TRAINER_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, or the `gh`
//! CLI's login). Without one they go out anonymously, which only works once
//! the repository is public.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::Digest;

const REPO: &str = "da-z/az_trainer";
const BRANCH: &str = "main";
const EXE_ASSET: &str = "az_trainer.exe";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY_WITHOUT_SCRIPTS: Duration = Duration::from_secs(60);
/// Records the blob hash of every script this updater wrote, so a file whose
/// hash no longer matches was edited by hand and is left alone.
const RECORD: &str = ".sync.json";

/// What the UI shows about updates.
#[derive(Default, Clone)]
pub struct State {
    /// A newer exe has been downloaded, verified and swapped in; it runs on
    /// the next start.
    pub staged: Option<String>,
    /// Config paths (`games/dawnwalker.js`) present in the repository, as of
    /// the last tree listing. Empty until one succeeds.
    pub remote: std::collections::HashSet<String>,
}

/// Where a config lives on GitHub, when the last listing saw it there.
pub fn github_url(state: &State, rel: &str) -> Option<String> {
    state
        .remote
        .contains(rel)
        .then(|| format!("https://github.com/{REPO}/blob/{BRANCH}/configs/{rel}"))
}

pub fn start(state: Arc<Mutex<State>>) {
    std::thread::spawn(move || {
        remove_previous_exe();
        loop {
            let token = token();
            // Scripts first: a fresh install has none and cannot do anything
            // until they arrive.
            if let Err(e) = sync_scripts(&state, token.as_deref()) {
                log(&format!("script sync: {e}"));
            }
            if let Err(e) = update_app(&state, token.as_deref()) {
                log(&format!("app update: {e}"));
            }
            // Without any game scripts yet (offline on first start), try
            // again soon instead of leaving the trainer empty for hours.
            let wait = if crate::js::Script::discover().is_empty() {
                RETRY_WITHOUT_SCRIPTS
            } else {
                CHECK_EVERY
            };
            std::thread::sleep(wait);
        }
    });
}

// ---- app -------------------------------------------------------------------

fn update_app(state: &Arc<Mutex<State>>, token: Option<&str>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if is_cargo_build(&exe) && !forced() {
        return Ok(()); // a dev build silently replaced by a release would be baffling
    }
    if state.lock().map(|s| s.staged.is_some()).unwrap_or(true) {
        return Ok(());
    }

    let body = get(
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        "application/vnd.github+json",
        token,
    )?;
    let rel: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tag = rel["tag_name"].as_str().ok_or("release has no tag")?;
    let (Some(remote), Some(local)) = (version(tag), version(VERSION)) else {
        return Err(format!("cannot compare versions {tag} and {VERSION}"));
    };
    if remote <= local {
        return Ok(());
    }

    let assets = rel["assets"].as_array().ok_or("release has no assets")?;
    let asset = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .and_then(|a| a["url"].as_str())
            .map(str::to_owned)
    };
    let exe_url = asset(EXE_ASSET).ok_or(format!("{tag} has no {EXE_ASSET}"))?;
    // Without a checksum there is no way to tell a truncated download from a
    // good one, so an unchecked binary is never installed.
    let sum_url = asset(&format!("{EXE_ASSET}.sha256"))
        .ok_or(format!("{tag} has no checksum - not installing"))?;

    let sum = get(&sum_url, "application/octet-stream", token)?;
    let want = String::from_utf8_lossy(&sum)
        .split_whitespace()
        .next()
        .ok_or("empty checksum")?
        .to_ascii_lowercase();
    let bin = get(&exe_url, "application/octet-stream", token)?;
    let got = hex(&sha2::Sha256::digest(&bin));
    if got != want {
        return Err(format!("{tag}: checksum mismatch, discarded"));
    }

    // Windows will not overwrite a running exe, but it will rename one. Move
    // ours aside and put the new one in its place; it starts next launch.
    let new = exe.with_extension("exe.new");
    let old = exe.with_extension("exe.old");
    std::fs::write(&new, &bin).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("could not install {tag}: {e}"));
    }

    let v = tag.trim_start_matches(['v', 'V']).to_string();
    log(&format!("installed {v}, applies on restart"));
    if let Ok(mut s) = state.lock() {
        s.staged = Some(v);
    }
    Ok(())
}

/// The exe moved aside by the last update. It was running then, so it could
/// not be deleted until now.
fn remove_previous_exe() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
}

fn version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+']);
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn is_cargo_build(exe: &Path) -> bool {
    let parts: Vec<_> = exe.components().collect();
    parts.windows(2).any(|w| {
        w[0].as_os_str().eq_ignore_ascii_case("target")
            && (w[1].as_os_str().eq_ignore_ascii_case("release")
                || w[1].as_os_str().eq_ignore_ascii_case("debug"))
    })
}

// ---- scripts ---------------------------------------------------------------

fn sync_scripts(state: &Arc<Mutex<State>>, token: Option<&str>) -> Result<(), String> {
    let Some(dir) = crate::js::configs_dir() else {
        return Ok(());
    };

    let body = get(
        &format!("https://api.github.com/repos/{REPO}/git/trees/{BRANCH}?recursive=1"),
        "application/vnd.github+json",
        token,
    )?;
    let tree: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let items = tree["tree"].as_array().ok_or("unexpected tree response")?;

    // Recorded before the checkout test, so a dev build still links its
    // configs to the repository.
    let remote = items
        .iter()
        .filter(|i| i["type"].as_str() == Some("blob"))
        .filter_map(|i| i["path"].as_str()?.strip_prefix("configs/"))
        .map(str::to_owned)
        .collect();
    if let Ok(mut s) = state.lock() {
        s.remote = remote;
    }

    // A checkout is managed with git; pulling files into it behind the
    // developer's back would fight their working tree.
    if dir.ancestors().any(|d| d.join(".git").exists()) && !forced() {
        return Ok(());
    }

    let record_path = dir.join(RECORD);
    let first_sync = !record_path.exists();
    let mut record: HashMap<String, String> = std::fs::read(&record_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    // The engine holds off loading scripts until this is dropped, so it never
    // sees a game script whose libraries are still on their way.
    let _busy = Syncing::begin();

    let (mut added, mut updated) = (0, 0);
    let mut edited: Vec<String> = Vec::new();
    for item in items {
        if item["type"].as_str() != Some("blob") {
            continue;
        }
        let (Some(path), Some(remote)) = (item["path"].as_str(), item["sha"].as_str()) else {
            continue;
        };
        let Some(rel) = path.strip_prefix("configs/").filter(|r| syncable(r)) else {
            continue;
        };
        let local_path = dir.join(rel);
        let local = std::fs::read(&local_path).ok().map(|b| blob_sha(&b));

        match &local {
            Some(l) if l == remote => {
                record.insert(rel.to_string(), remote.to_string());
                continue;
            }
            // Changed upstream. Only replace what this updater put there; a
            // hash we did not write means someone edited the file. The very
            // first sync has nothing to compare against, and a fresh install
            // has not been edited, so it takes everything.
            Some(l) if !first_sync && record.get(rel) != Some(l) => {
                edited.push(rel.to_string());
                continue;
            }
            _ => {}
        }

        let bytes = get(
            &format!("https://api.github.com/repos/{REPO}/contents/configs/{rel}?ref={BRANCH}"),
            "application/vnd.github.raw",
            token,
        )?;
        if blob_sha(&bytes) != remote {
            return Err(format!("{rel}: download does not match the repository"));
        }
        write_replace(&local_path, &bytes)?;
        record.insert(rel.to_string(), remote.to_string());
        if local.is_some() {
            updated += 1;
        } else {
            added += 1;
        }
    }

    let json = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
    write_replace(&record_path, &json)?;

    let mut parts = Vec::new();
    if added > 0 {
        parts.push(format!("{added} new"));
    }
    if updated > 0 {
        parts.push(format!("{updated} updated"));
    }
    if !edited.is_empty() {
        log(&format!("kept local edits: {}", edited.join(", ")));
        parts.push(format!("{} kept (edited locally)", edited.len()));
    }
    if !parts.is_empty() {
        log(&format!("Scripts: {}", parts.join(", ")));
    }
    Ok(())
}

static SYNCING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True while scripts are being written. The engine waits rather than load a
/// half-downloaded set.
pub fn syncing() -> bool {
    SYNCING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Marks a sync in progress until dropped - including on an early error.
struct Syncing;

impl Syncing {
    fn begin() -> Syncing {
        SYNCING.store(true, std::sync::atomic::Ordering::Relaxed);
        Syncing
    }
}

impl Drop for Syncing {
    fn drop(&mut self) {
        SYNCING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Only the shapes the trainer loads - `games/<name>.js`, its artwork, and
/// `lib/<name>.js` - with plain names, so nothing from the tree can land
/// outside `configs/`.
fn syncable(rel: &str) -> bool {
    let path = Path::new(rel);
    if path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return false;
    }
    let mut parts = rel.split('/');
    let (Some(folder), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let plain = !file.is_empty()
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    let ext = file.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    plain
        && match folder {
            "games" => matches!(ext.as_str(), "js" | "png" | "jpg" | "jpeg" | "webp"),
            "lib" => ext == "js",
            _ => false,
        }
}

/// Git's object id for a file, which the tree listing reports - so unchanged
/// files are recognised without downloading them.
fn blob_sha(bytes: &[u8]) -> String {
    let mut h = sha1_smol::Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    h.digest().to_string()
}

/// Write beside the target, then rename over it, so the hot-reload watcher
/// never picks up a half-written script.
fn write_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp: PathBuf = path.with_extension("download");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

// ---- plumbing --------------------------------------------------------------

fn token() -> Option<String> {
    for var in ["AZ_TRAINER_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(t) = std::env::var(var) {
            if !t.trim().is_empty() {
                return Some(t.trim().to_string());
            }
        }
    }
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["auth", "token"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
    }
    let out = cmd.output().ok()?;
    let t = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

fn get(url: &str, accept: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
    let mut req = ureq::get(url)
        .set("User-Agent", concat!("az_trainer/", env!("CARGO_PKG_VERSION")))
        .set("Accept", accept)
        .set("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(60));
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(404, _) if token.is_none() => {
            "not found - the repository is private and no GitHub token is available".to_string()
        }
        other => other.to_string(),
    })?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Test hook: run the updater from a cargo build or a git checkout anyway.
fn forced() -> bool {
    std::env::var_os("AZ_TRAINER_FORCE_UPDATE").is_some()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn log(msg: &str) {
    use std::io::Write;
    println!("[update] {msg}");
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order() {
        assert!(version("v1.1.0") > version("1.0.0"));
        assert!(version("v1.0.10") > version("v1.0.9"));
        assert_eq!(version("v2"), Some((2, 0, 0)));
        assert_eq!(version("1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(version("latest"), None);
    }

    #[test]
    fn blob_sha_matches_git() {
        // printf 'hello\n' | git hash-object --stdin
        assert_eq!(blob_sha(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn only_trainer_files_sync() {
        assert!(syncable("games/veilguard.js"));
        assert!(syncable("games/veilguard.jpg"));
        assert!(syncable("lib/hook.js"));
        assert!(!syncable("games/veilguard.settings"));
        assert!(!syncable(".sync.json"));
        assert!(!syncable("lib/nested/x.js"));
        assert!(!syncable("../src/main.rs"));
        assert!(!syncable("games/a b.js"));
        assert!(!syncable("tools/scan.py"));
    }

    #[test]
    fn spots_cargo_builds() {
        assert!(is_cargo_build(Path::new(r"C:\dev\app\target\release\az_trainer.exe")));
        assert!(!is_cargo_build(Path::new(r"C:\Games\AZ Trainer\az_trainer.exe")));
    }
}
