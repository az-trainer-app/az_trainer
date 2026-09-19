//! Fetch a game's Steam artwork and save it as a config's banner image.
//!
//!     cargo run --bin artwork -- "Black Myth Wukong" wukong
//!     cargo run --bin artwork -- 2358720 wukong
//!
//! Takes a Steam app id, a store URL, or a name to look up, and writes
//! `configs/games/<config>.jpg`: the library hero (the widest art Steam has,
//! 1920x620), scaled to 1280 wide - where the rest of the artwork sits, and
//! roughly what the 420px banner needs at any display scale. The trainer fits
//! it to the banner itself (art.rs), so only the download size is at stake.
//!
//! Prints what it picked, so a wrong match is obvious before it is committed.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

/// Steam's own art, widest first: the hero is the only one wide enough to fill
/// the banner without much stretching either side.
const ART: [&str; 3] = ["library_hero.jpg", "library_600x900.jpg", "header.jpg"];

/// What the stored artwork is scaled down to.
const WIDTH: u32 = 1280;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [query, config] = args.as_slice() else {
        eprintln!("usage: artwork <steam app id | store url | game name> <config name>");
        eprintln!("   eg: artwork \"Black Myth Wukong\" wukong");
        std::process::exit(2);
    };
    if let Err(e) = run(query, config) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run(query: &str, config: &str) -> Result<(), String> {
    let out = PathBuf::from("configs/games").join(format!("{config}.jpg"));
    let games = out.parent().filter(|d| d.is_dir()).ok_or_else(|| {
        format!("{} is not there - run this from the repository root", out.display())
    })?;
    let script = games.join(format!("{config}.js"));
    if !script.exists() {
        println!("note: {} does not exist yet", script.display());
    }

    let (appid, name) = app(query)?;
    println!("Steam app {appid}: {name}");

    let (from, bytes) = ART
        .iter()
        .find_map(|art| {
            let url = format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/{art}");
            get(&url).ok().map(|b| (url, b))
        })
        .ok_or("Steam has no artwork for that app id")?;
    println!("  {from} ({} KB)", bytes.len() / 1024);

    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = (img.width(), img.height());
    let img = if w > WIDTH {
        let nh = (h as f32 * WIDTH as f32 / w as f32).round().max(1.0) as u32;
        image::imageops::resize(&img, WIDTH, nh, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let mut file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, 88)
        .encode_image(&img)
        .map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!(
        "  wrote {} - {}x{} from {w}x{h}, {} KB",
        out.display(),
        img.width(),
        img.height(),
        size / 1024
    );
    Ok(())
}

/// The app id and name behind a query: an id, a store URL, or a name to search.
fn app(query: &str) -> Result<(u64, String), String> {
    let q = query.trim();
    let from_url = q
        .split("/app/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|id| id.parse::<u64>().ok());
    if let Some(id) = q.parse::<u64>().ok().or(from_url) {
        return Ok((id, name(id).unwrap_or_else(|| "?".into())));
    }

    // Steam's own store search, the one the client's search box uses
    let url = format!("https://steamcommunity.com/actions/SearchApps/{}", encode(q));
    let hits: serde_json::Value =
        serde_json::from_slice(&get(&url)?).map_err(|e| e.to_string())?;
    let hits = hits.as_array().filter(|h| !h.is_empty()).ok_or("no Steam game by that name")?;
    for extra in hits.iter().skip(1).take(3) {
        println!("  (also matched {})", extra["name"].as_str().unwrap_or("?"));
    }
    let hit = &hits[0];
    let id = hit["appid"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| hit["appid"].as_u64())
        .ok_or("Steam's search returned no app id")?;
    Ok((id, hit["name"].as_str().unwrap_or("?").to_string()))
}

/// The store's name for an app id, when it answers.
fn name(appid: u64) -> Option<String> {
    let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&filters=basic");
    let v: serde_json::Value = serde_json::from_slice(&get(&url).ok()?).ok()?;
    v[appid.to_string()]["data"]["name"].as_str().map(str::to_string)
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "%20".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url)
        .set("User-Agent", concat!("az_trainer/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_reader().take(32 * 1024 * 1024).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}
