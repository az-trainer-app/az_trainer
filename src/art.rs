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
/// so an object-fit cover would crop it top and bottom. Instead the image is
/// left-aligned on a banner-ratio canvas and its right edge column is stretched
/// out and lightly blurred to fill the remainder - nothing is cropped
/// vertically, and there is no fill on the left. The bottom is then faded to
/// the window colour so the banner melts into the content below rather than
/// ending on a hard line. Results are cached in the temp dir by content.
fn fit_banner(src: &Path) -> Option<PathBuf> {
    // window background (main.rs BG), the colour the banner fades into
    const BG: [u8; 3] = [0x11, 0x13, 0x1a];

    let meta = std::fs::metadata(src).ok()?;
    let img = image::open(src).ok()?.to_rgb8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // `v2` marks the left-aligned + bottom-fade composition, so stale cached
    // banners from the old centred version are not reused.
    let key = format!("v2|{}|{mtime}|{w}x{h}", src.to_string_lossy());
    let digest = sha2::Sha256::digest(key.as_bytes());
    let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let out = std::env::temp_dir().join(format!("az_trainer_banner_{hash}.jpg"));
    if out.exists() {
        return Some(out);
    }

    let cw = (h as f32 * BANNER_RATIO).round() as u32;
    let mut canvas = RgbImage::new(cw.max(w), h);

    if w >= cw {
        // wide enough: no fill needed, the cover crops width
        imageops::replace(&mut canvas, &img, 0, 0);
    } else {
        // left-aligned: image at x=0, right edge stretched across the remainder
        for y in 0..h {
            let right = *img.get_pixel(w - 1, y);
            for x in w..cw {
                canvas.put_pixel(x, y, right);
            }
        }
        let rpad = cw - w;
        if rpad > 1 {
            let sigma = (h as f32 / 60.0).max(4.0);
            let blurred = imageops::blur(&imageops::crop_imm(&canvas, w, 0, rpad, h).to_image(), sigma);
            imageops::replace(&mut canvas, &blurred, w as i64, 0);
        }
        imageops::replace(&mut canvas, &img, 0, 0);
    }

    // fade the bottom third into the window colour
    let (cw2, ch2) = canvas.dimensions();
    let fade = (ch2 as f32 * 0.34).round() as u32;
    for y in (ch2 - fade)..ch2 {
        let t = (y - (ch2 - fade)) as f32 / fade as f32; // 0 at top of fade, 1 at bottom
        for x in 0..cw2 {
            let p = canvas.get_pixel_mut(x, y);
            for c in 0..3 {
                p[c] = (p[c] as f32 * (1.0 - t) + BG[c] as f32 * t).round() as u8;
            }
        }
    }

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
