//! Game artwork.
//!
//! Drop an image next to the config with the same name -- `dawnwalker.js` ->
//! `dawnwalker.jpg` -- and it is picked up automatically. Nothing to declare
//! in the config, no Steam install, no appid, no network.
//!
//! A base64 `export const art` still works as a fallback, for configs that
//! need to travel as a single file.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::RenderImage;
use image::{imageops, Delay, Frame, RgbImage, RgbaImage};
use sha2::Digest;

use crate::js::Script;

const EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Banner height, in window pixels.
pub const BANNER_H: f32 = 108.0;

/// Window width for a config laid out in this many columns. The banner spans
/// the window, so artwork is fitted to `window_width : BANNER_H`.
pub fn window_width(columns: usize) -> f32 {
    if columns >= 2 {
        740.0
    } else {
        420.0
    }
}

/// window background (main.rs BG), the colour the banner fades into
const BG: [u8; 3] = [0x11, 0x13, 0x1a];

/// The share of the banner's height, at the bottom, faded into BG.
const FADE: f32 = 0.34;

#[derive(Clone, Debug)]
pub struct Art {
    /// the fitted still banner
    pub banner: PathBuf,
    /// the stretched fills either side, animated; empty when the art is wide enough
    pub liquid: Vec<Liquid>,
}

/// A seamless loop of one of the banner's side fills, stirred like a liquid.
#[derive(Clone)]
pub struct Liquid {
    pub frames: Arc<RenderImage>,
    /// where the fill starts and how wide it is, as fractions of the banner width
    pub left: f32,
    pub width: f32,
}

impl fmt::Debug for Liquid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Liquid({} frames)", self.frames.frame_count())
    }
}

pub fn resolve(script: &Script) -> Option<Art> {
    let base = sibling_image(&script.path).or_else(|| embedded(script))?;
    let img = image::open(&base).ok().map(|i| i.to_rgb8());
    let Some(img) = img.filter(|i| i.width() > 0 && i.height() > 0) else {
        return Some(Art { banner: base, liquid: Vec::new() });
    };
    let ratio = window_width(script.columns) / BANNER_H;
    Some(Art {
        banner: fit_banner(&base, &img, ratio).unwrap_or(base),
        liquid: liquids(&img, ratio),
    })
}

/// Canvas width for an image of height `h`, and the art's left edge on it
/// (centred). The right fill is whatever remains past the art.
fn layout(w: u32, h: u32, ratio: f32) -> (u32, u32) {
    let cw = (h as f32 * ratio).round() as u32;
    (cw, cw.saturating_sub(w) / 2)
}

/// The blur applied to the stretched fill, for an image of height `h`.
fn fill_sigma(h: u32) -> f32 {
    (h as f32 / 60.0).max(4.0)
}

/// Fit artwork to the banner's aspect ratio.
///
/// Steam's widest art (the 1920x620 hero) is still narrower than the banner,
/// so an object-fit cover would crop it top and bottom. Instead the image is
/// centred on a banner-ratio canvas and its edge columns are stretched out and
/// lightly blurred to fill either side - nothing is cropped vertically. The
/// bottom is then faded to
/// the window colour so the banner melts into the content below rather than
/// ending on a hard line. Results are cached in the temp dir by content.
fn fit_banner(src: &Path, img: &RgbImage, ratio: f32) -> Option<PathBuf> {
    let meta = std::fs::metadata(src).ok()?;
    let (w, h) = img.dimensions();

    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // `v3` marks the centred, filled-both-sides + bottom-fade composition, so
    // stale cached banners from older layouts are not reused.
    let key = format!("v3|{}|{mtime}|{w}x{h}|{ratio:.4}", src.to_string_lossy());
    let digest = sha2::Sha256::digest(key.as_bytes());
    let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let out = std::env::temp_dir().join(format!("az_trainer_banner_{hash}.jpg"));
    if out.exists() {
        return Some(out);
    }

    let (cw, ox) = layout(w, h, ratio);
    let mut canvas = RgbImage::new(cw.max(w), h);

    if w >= cw {
        // wide enough: no fill needed, the cover crops width
        imageops::replace(&mut canvas, img, 0, 0);
    } else {
        // centred: each edge column stretched across its side, then blurred
        let right_from = ox + w;
        for y in 0..h {
            let (left, right) = (*img.get_pixel(0, y), *img.get_pixel(w - 1, y));
            for x in 0..ox {
                canvas.put_pixel(x, y, left);
            }
            for x in right_from..cw {
                canvas.put_pixel(x, y, right);
            }
        }
        let sigma = fill_sigma(h);
        for (x, pad) in [(0, ox), (right_from, cw - right_from)] {
            if pad > 1 {
                let blurred = imageops::blur(&imageops::crop_imm(&canvas, x, 0, pad, h).to_image(), sigma);
                imageops::replace(&mut canvas, &blurred, x as i64, 0);
            }
        }
        imageops::replace(&mut canvas, img, ox as i64, 0);
    }

    // fade the bottom third into the window colour
    let (cw2, ch2) = canvas.dimensions();
    let fade = (ch2 as f32 * FADE).round() as u32;
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

/// Frame height of the liquid loop.
const LIQUID_H: u32 = 128;
/// Widest a frame may get; wide fills (two-column windows) trade height for it.
const LIQUID_MAX_W: u32 = 220;
/// 150 frames at 40ms: a 6 second loop.
const LIQUID_FRAMES: usize = 150;
const LIQUID_DELAY_MS: u32 = 40;
/// How much of the real art, in from each edge, turns to liquid, in banner
/// heights (capped at a quarter of the art).
const LIQUID_INTO_ART: f32 = 0.45;
/// How much of the art's bottom the liquid leaves out, as a share of its height.
const LIQUID_LIFT: f32 = 0.35;

/// Both side fills, the left one mirrored so its flow runs out to the left.
fn liquids(img: &RgbImage, ratio: f32) -> Vec<Liquid> {
    let (w, h) = img.dimensions();
    let (cw, ox) = layout(w, h, ratio);
    if w >= cw {
        return Vec::new();
    }
    let right_pad = cw - w - ox;
    [(true, ox, 0.0), (false, right_pad, 2.3)]
        .into_iter()
        .filter_map(|(mirror, pad, seed)| {
            let (frames, band, rw) = liquid(img, pad, mirror, seed)?;
            let left = if mirror { ox + band - rw } else { ox + w - band };
            Some(Liquid {
                frames: Arc::new(RenderImage::new(frames)),
                left: left as f32 / cw as f32,
                width: rw as f32 / cw as f32,
            })
        })
        .collect()
}

/// Animate one side fill that `fit_banner` stretches out, `pad` wide: the
/// right one, or with `mirror` the left one, built as the right one of the
/// flipped art and flipped back.
///
/// The loop covers the fill plus a band of the art itself, so the flow starts
/// inside the picture rather than at the seam. Every pixel reads its colour
/// from a point pushed around by a two-layer domain warp (sines bending the
/// coordinates, then sines of the bent coordinates), which folds the art's
/// edge into swirls; its waves travel away from the art, and it softens as it
/// goes, so the picture dissolves into the blurred fill. Across the band the
/// stirring ramps in from nothing, and the frame fades in over the still art,
/// so there is no edge to see. Every term turns a whole number of times per
/// loop, so the last frame runs straight into the first; `seed` shifts the
/// pattern so the two sides differ. The bottom fade goes on after the warp, so
/// it stays put.
///
/// Returns the frames, the band of art covered and the frame width, in image
/// pixels.
fn liquid(img: &RgbImage, pad: u32, mirror: bool, seed: f32) -> Option<(Vec<Frame>, u32, u32)> {
    use std::f32::consts::TAU;

    let (w, h) = img.dimensions();
    if pad < 2 {
        return None;
    }
    // art pixel `x` counted back from the edge this fill continues
    let art = |x: u32, y: u32| *img.get_pixel(if mirror { x } else { w - 1 - x }, y);

    // source, flowing outward along +x: the art's edge band, then the fill
    // rebuilt the way fit_banner does
    let band = ((h as f32 * LIQUID_INTO_ART) as u32).min(w / 4).max(1);
    let edge = RgbImage::from_fn(4, h, |_, y| art(0, y));
    let edge = imageops::blur(&edge, fill_sigma(h));
    let rw = band + pad;
    let region = RgbImage::from_fn(rw, h, |x, y| {
        if x < band {
            art(band - 1 - x, y)
        } else {
            *edge.get_pixel(0, y)
        }
    });

    let fh = LIQUID_H.min(h).min(LIQUID_MAX_W * h / rw).max(16);
    let fw = (rw as f32 / h as f32 * fh as f32).round().max(2.0) as u32;
    let sharp = imageops::resize(&region, fw, fh, imageops::FilterType::Triangle);
    let soft = imageops::blur(&sharp, fh as f32 / 48.0);
    let texels = |im: &RgbImage| -> Vec<[f32; 3]> { im.pixels().map(|p| p.0.map(|c| c as f32)).collect() };
    let (sharp, soft) = (texels(&sharp), texels(&soft));

    // bilinear colour at a fractional texel, mirrored past the top and bottom
    let (nw, n) = (fw as f32, fh as f32);
    let sample = |tex: &[[f32; 3]], sx: f32, sy: f32| -> [f32; 3] {
        let mut r = sy.rem_euclid(2.0 * n);
        if r >= n {
            r = 2.0 * n - r;
        }
        let r = (r - 0.5).clamp(0.0, n - 1.0);
        let c = (sx - 0.5).clamp(0.0, nw - 1.0);
        let (y0, x0) = (r as usize, c as usize);
        let (y1, x1) = ((y0 + 1).min(fh as usize - 1), (x0 + 1).min(fw as usize - 1));
        let (fy, fx) = (r - y0 as f32, c - x0 as f32);
        let at = |x: usize, y: usize| tex[y * fw as usize + x];
        [0, 1, 2].map(|i| {
            let top = at(x0, y0)[i] * (1.0 - fx) + at(x1, y0)[i] * fx;
            let bottom = at(x0, y1)[i] * (1.0 - fx) + at(x1, y1)[i] * fx;
            top * (1.0 - fy) + bottom * fy
        })
    };

    // displacement, in banner heights, at (x, y) in banner heights; the terms
    // in x all run `k * x - m * phi`, so their waves move out, away from the art
    let flow = |x: f32, y: f32, phi: f32| -> f32 {
        let qx = x + 0.28 * (2.6 * y - phi + 0.5 + seed).sin() + 0.12 * (5.3 * y + 2.0 * phi + 1.3 * seed).sin();
        let qy = y + 0.28 * (3.1 * x - phi + 1.9 - seed).sin() + 0.12 * (4.9 * x - 2.0 * phi + 0.7 + 0.6 * seed).sin();
        0.20 * (3.4 * qx + 2.2 * qy - phi + 2.0 * seed).sin()
            + 0.09 * (6.3 * qy + 4.1 * qx - 2.0 * phi + 2.1 - seed).sin()
    };
    let smooth = |e0: f32, e1: f32, v: f32| {
        let t = ((v - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };

    // along the frame: fade in over the first stretch of the band, stir up
    // across the rest of it
    let band_u = band as f32 / rw as f32;
    let (fade_in, stir_from, stir_full) = (band_u * 0.25, band_u * 0.1, band_u * 1.1);

    let frames: Vec<Frame> = (0..LIQUID_FRAMES)
        .map(|k| {
            let phi = TAU * k as f32 / LIQUID_FRAMES as f32;
            let buf = RgbaImage::from_fn(fw, fh, |x, y| {
                let x = if mirror { fw - 1 - x } else { x };
                let u = (x as f32 + 0.5) / nw;
                let (px, py) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);
                let stir = smooth(stir_from, stir_full, u);
                let dy = flow(px, py, phi);
                let dx = 0.7 * flow(py + 1.7, px + 3.1, phi);
                // a little light on the slopes of the surface
                let slope = (flow(px + 1.0 / n, py, phi) - dy) * n;
                let sheen = 1.0 + stir * (slope * 0.08).clamp(-0.1, 0.1);
                // as it stirs, read from the upper part of the art only, stretched
                // over the full height, so its dark bottom does not pool below
                let span = 1.0 - LIQUID_LIFT * stir;
                let (sx, sy) = ((px + stir * dx) * n, (py * span + stir * dy) * n);
                let (a, b) = (sample(&sharp, sx, sy), sample(&soft, sx, sy));
                // fit_banner's linear fade at the seam, easing into a later,
                // steeper one as it stirs: still BG at the very bottom edge
                let fade = ((py - (1.0 - FADE)) / FADE).clamp(0.0, 1.0).powf(1.0 + 1.5 * stir);
                let rgb = [0, 1, 2].map(|i| {
                    let v = ((a[i] * (1.0 - stir) + b[i] * stir) * sheen).min(255.0);
                    (v * (1.0 - fade) + BG[i] as f32 * fade).round() as u8
                });
                let alpha = (smooth(0.0, fade_in, u) * 255.0).round() as u8;
                // gpui wants BGRA
                image::Rgba([rgb[2], rgb[1], rgb[0], alpha])
            });
            Frame::from_parts(buf, 0, 0, Delay::from_numer_denom_ms(LIQUID_DELAY_MS, 1))
        })
        .collect();

    Some((frames, band, rw))
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
        script.processes[0].replace(['.', '\\', '/', ':'], "_")
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
