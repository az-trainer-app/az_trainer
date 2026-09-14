//! Game artwork.
//!
//! Drop an image next to the config with the same name -- `dawnwalker.js` ->
//! `dawnwalker.jpg` -- and it is picked up automatically. Nothing to declare
//! in the config, no Steam install, no appid, no network.
//!
//! A base64 `export const art` still works as a fallback, for configs that
//! need to travel as a single file.

use std::path::{Path, PathBuf};

use image::{imageops, RgbImage};
use sha2::Digest;

use crate::js::Script;

const EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// The banner's width:height (main.rs WIDTH : BANNER_H). Artwork is fitted to it.
const BANNER_RATIO: f32 = 420.0 / 108.0;

pub fn resolve(script: &Script) -> Option<PathBuf> {
    let base = sibling_image(&script.path).or_else(|| embedded(script))?;
    Some(fit_banner(&base).unwrap_or(base))
}

/// Fit artwork to the banner's aspect ratio.
///
/// Steam's widest art (the 1920x620 hero) is still narrower than the banner,
/// so any real source would be cropped top and bottom by an object-fit cover.
/// Instead, a narrower image is centred on a banner-ratio canvas with its side
/// edge columns stretched out and lightly blurred - nothing is cropped
/// vertically, and the blur keeps the fill from reading as hard streaks. An
/// image already at least as wide as the banner is left alone (cover then crops
/// width, keeping full height). Results are cached in the temp dir by content.
fn fit_banner(src: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(src).ok()?;
    let img = image::open(src).ok()?.to_rgb8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w as f32 / h as f32 >= BANNER_RATIO - 0.02 {
        return None; // already wide enough: use the original
    }

    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = format!("{}|{mtime}|{w}x{h}", src.to_string_lossy());
    let digest = sha2::Sha256::digest(key.as_bytes());
    let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let out = std::env::temp_dir().join(format!("az_trainer_banner_{hash}.jpg"));
    if out.exists() {
        return Some(out);
    }

    let cw = (h as f32 * BANNER_RATIO).round() as u32;
    let pad_l = (cw - w) / 2;
    let mut canvas = RgbImage::new(cw, h);

    // stretch each side's edge column across its padding
    for y in 0..h {
        let left = *img.get_pixel(0, y);
        let right = *img.get_pixel(w - 1, y);
        for x in 0..pad_l {
            canvas.put_pixel(x, y, left);
        }
        for x in (pad_l + w)..cw {
            canvas.put_pixel(x, y, right);
        }
    }
    // soften the stretched pads
    let sigma = (h as f32 / 60.0).max(4.0);
    if pad_l > 1 {
        let blurred = imageops::blur(&imageops::crop_imm(&canvas, 0, 0, pad_l, h).to_image(), sigma);
        imageops::replace(&mut canvas, &blurred, 0, 0);
    }
    let rpad = cw - pad_l - w;
    if rpad > 1 {
        let x = (pad_l + w) as i64;
        let blurred = imageops::blur(&imageops::crop_imm(&canvas, pad_l + w, 0, rpad, h).to_image(), sigma);
        imageops::replace(&mut canvas, &blurred, x, 0);
    }
    // the crisp source on top, centred
    imageops::replace(&mut canvas, &img, pad_l as i64, 0);

    canvas.save(&out).ok().map(|_| out)
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
