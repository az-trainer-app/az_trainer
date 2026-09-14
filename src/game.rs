//! Which build of a game is running.
//!
//! A config that relies on fixed addresses supports only the builds it lists
//! (`export const builds`), so the engine needs something stable to tell them
//! apart. Two things are read, both without touching the game's code:
//!
//! - the main module's PE header: its link timestamp and image size, which
//!   change with every build of the executable;
//! - the Steam build id from the library's `appmanifest_*.acf`, when the game
//!   is installed through Steam - the number SteamDB and patch notes use.

use std::path::Path;

use crate::mem::Proc;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Identity {
    /// executable name that was matched, as the config spelled it
    pub exe: String,
    /// PE header `TimeDateStamp`
    pub timestamp: u32,
    /// PE header `SizeOfImage`
    pub size: u32,
    /// Steam's build id for the install, if it is a Steam install
    pub steam_build: Option<u64>,
}

impl Identity {
    /// Read the identity of the module loaded at `base`.
    pub fn read(proc: &Proc, exe: &str, base: u64) -> Identity {
        let u32_at = |addr: u64| {
            let mut b = [0u8; 4];
            proc.read(addr, &mut b).then(|| u32::from_le_bytes(b))
        };
        let (timestamp, size) = u32_at(base + 0x3c)
            .map(|lfanew| base + lfanew as u64)
            // "PE\0\0", then the file header: TimeDateStamp at +8; the
            // optional header starts at +0x18 and has SizeOfImage at +0x38
            .filter(|&nt| u32_at(nt) == Some(0x4550))
            .map(|nt| (u32_at(nt + 8).unwrap_or(0), u32_at(nt + 0x50).unwrap_or(0)))
            .unwrap_or((0, 0));
        let steam_build = proc.image_path().and_then(|p| steam_build(&p));
        Identity { exe: exe.to_string(), timestamp, size, steam_build }
    }

    /// `Steam build 24769601, timestamp 0x68a1b2c3, size 0xd2e1000`
    pub fn describe(&self) -> String {
        let steam = self.steam_build.map(|b| format!("Steam build {b}, ")).unwrap_or_default();
        format!("{steam}timestamp 0x{:x}, size 0x{:x}", self.timestamp, self.size)
    }

    /// The shortest thing that tells this build apart, for the status line.
    pub fn short(&self) -> String {
        match self.steam_build {
            Some(b) => format!("Steam build {b}"),
            None => format!("timestamp 0x{:x}", self.timestamp),
        }
    }
}

/// The Steam build id of the install holding `exe`, from its app manifest.
///
/// Steam installs games under `<library>/steamapps/common/<installdir>/`, and
/// `<library>/steamapps/appmanifest_<appid>.acf` names that `installdir` along
/// with the `buildid`.
pub fn steam_build(exe: &Path) -> Option<u64> {
    let mut parts = exe.ancestors();
    let (install, steamapps) = loop {
        let dir = parts.next()?;
        let common = dir.parent()?;
        if common.file_name()?.eq_ignore_ascii_case("common")
            && common.parent()?.file_name()?.eq_ignore_ascii_case("steamapps")
        {
            break (dir.file_name()?.to_string_lossy().into_owned(), common.parent()?.to_path_buf());
        }
    };
    std::fs::read_dir(steamapps)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase());
            name.is_some_and(|n| n.starts_with("appmanifest_") && n.ends_with(".acf"))
        })
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .find(|acf| acf_value(acf, "installdir").is_some_and(|d| d.eq_ignore_ascii_case(&install)))
        .and_then(|acf| acf_value(&acf, "buildid")?.parse().ok())
}

/// A top-level `"key"  "value"` pair from a Valve KeyValues file.
fn acf_value(acf: &str, key: &str) -> Option<String> {
    acf.lines().find_map(|line| {
        let mut quoted = line.split('"').skip(1).step_by(2);
        let k = quoted.next()?;
        let v = quoted.next()?;
        k.eq_ignore_ascii_case(key).then(|| v.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PE offsets and the image path, against a real process: this one.
    #[test]
    fn identity_reads_this_process_header() {
        let exe_path = std::env::current_exe().unwrap();
        let exe = exe_path.file_name().unwrap().to_string_lossy().into_owned();
        let proc = Proc::open(std::process::id()).expect("open own process");
        let (base, size) = proc.module(&exe).expect("own module");

        let id = Identity::read(&proc, &exe, base);
        assert_eq!(id.size as usize, size, "SizeOfImage agrees with the loader");
        assert_ne!(id.timestamp, 0);
        assert_eq!(id.steam_build, None);
        let image = proc.image_path().expect("image path");
        assert!(image.ends_with(&exe), "{}", image.display());
    }

    #[test]
    fn steam_build_comes_from_the_manifest_naming_the_install() {
        let lib = std::env::temp_dir().join(format!("az_trainer_steam_{}", std::process::id()));
        let game = lib.join("steamapps").join("common").join("Some Game").join("bin");
        std::fs::create_dir_all(&game).unwrap();
        let manifest = |appid: u32, dir: &str, build: u64| {
            std::fs::write(
                lib.join("steamapps").join(format!("appmanifest_{appid}.acf")),
                format!(
                    "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"installdir\"\t\t\"{dir}\"\n\
                     \t\"buildid\"\t\t\"{build}\"\n\t\"InstalledDepots\"\n\t{{\n\t}}\n}}\n"
                ),
            )
            .unwrap();
        };
        manifest(10, "Other Game", 111);
        manifest(20, "Some Game", 24769601);

        assert_eq!(steam_build(&game.join("Game.exe")), Some(24769601));
        assert_eq!(steam_build(&lib.join("Game.exe")), None, "not under steamapps/common");
        std::fs::remove_dir_all(&lib).ok();
    }
}
