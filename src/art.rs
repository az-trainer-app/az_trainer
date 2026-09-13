//! Game artwork.
//!
//! Drop an image next to the config with the same name -- `dawnwalker.js` ->
//! `dawnwalker.jpg` -- and it is picked up automatically. Nothing to declare
//! in the config, no Steam install, no appid, no network.
//!
//! A base64 `export const art` still works as a fallback, for configs that
//! need to travel as a single file.

use std::path::{Path, PathBuf};

use crate::js::Script;

const EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

pub fn resolve(script: &Script) -> Option<PathBuf> {
    sibling_image(&script.path).or_else(|| embedded(script))
}

/// `games/foo.js` -> `games/foo.png` / `.jpg` / `.jpeg` / `.webp`
fn sibling_image(config: &Path) -> Option<PathBuf> {
    let stem = config.file_stem()?;
    let dir = config.parent()?;
    EXTS.iter()
        .map(|ext| dir.join(stem).with_extension(ext))
        .find(|p| p.is_file())
}

/// base64 in the config, decoded to a temp file (the UI loads art by path)
fn embedded(script: &Script) -> Option<PathBuf> {
    let bytes = decode_b64(script.art.as_deref()?)?;
    if bytes.len() < 16 {
        return None;
    }
    let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else { "jpg" };
    let name = format!(
        "az_trainer_art_{}.{ext}",
        script.process.replace(['.', '\\', '/', ':'], "_")
    );
    let path = std::env::temp_dir().join(name);
    if !same_size(&path, &bytes) {
        std::fs::write(&path, &bytes).ok()?;
    }
    Some(path)
}

fn same_size(path: &Path, bytes: &[u8]) -> bool {
    matches!(std::fs::metadata(path), Ok(m) if m.len() as usize == bytes.len())
}

/// Minimal base64 decoder; tolerates whitespace and a `data:` URL prefix.
fn decode_b64(s: &str) -> Option<Vec<u8>> {
    let s = match s.find("base64,") {
        Some(i) => &s[i + 7..],
        None => s,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}
