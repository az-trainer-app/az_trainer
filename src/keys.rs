//! Shortcut text such as `"Alt+F1"`, parsed into what `RegisterHotKey` takes.

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

/// `"Alt+F1"`, `"Ctrl+Shift+G"`, `"Num5"` - modifiers, then exactly one key:
/// F1-F24, A-Z, 0-9 or Num0-Num9. Case-insensitive.
pub fn parse(text: &str) -> Option<Hotkey> {
    let mut mods = 0;
    let mut vk = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "alt" => mods |= MOD_ALT,
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "shift" => mods |= MOD_SHIFT,
            "win" => mods |= MOD_WIN,
            key if vk.is_none() => vk = Some(virtual_key(key)?),
            _ => return None, // two keys
        }
    }
    Some(Hotkey { mods, vk: vk? })
}

fn virtual_key(key: &str) -> Option<u32> {
    let number = |prefix: &str| key.strip_prefix(prefix).and_then(|n| n.parse::<u32>().ok());
    if let Some(n) = number("f") {
        return (1..=24).contains(&n).then(|| 0x6f + n); // VK_F1 = 0x70
    }
    if let Some(n) = number("num") {
        return (n <= 9).then(|| 0x60 + n); // VK_NUMPAD0 = 0x60
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c @ 'a'..='z'), None) => Some(c.to_ascii_uppercase() as u32),
        (Some(c @ '0'..='9'), None) => Some(c as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_keys() {
        assert_eq!(parse("Alt+F1"), Some(Hotkey { mods: MOD_ALT, vk: 0x70 }));
        assert_eq!(parse("alt + f3"), Some(Hotkey { mods: MOD_ALT, vk: 0x72 }));
        assert_eq!(parse("Ctrl+Shift+G"), Some(Hotkey { mods: MOD_CONTROL | MOD_SHIFT, vk: 0x47 }));
        assert_eq!(parse("Win+7"), Some(Hotkey { mods: MOD_WIN, vk: 0x37 }));
        assert_eq!(parse("Num5"), Some(Hotkey { mods: 0, vk: 0x65 }));
        assert_eq!(parse("F24"), Some(Hotkey { mods: 0, vk: 0x87 }));
        assert_eq!(parse("Alt+F"), Some(Hotkey { mods: MOD_ALT, vk: 0x46 }));
    }

    #[test]
    fn rejects_what_it_cannot_register() {
        for bad in ["", "Alt", "Alt+", "F25", "F0", "Num10", "Alt+F1+F2", "Hyper+A", "Esc"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }
}
