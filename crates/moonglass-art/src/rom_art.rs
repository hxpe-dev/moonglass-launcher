//! Artwork for emulated games without an API key.
//!
//! * Switch games are looked up by title ID on nlib.cc, which serves each
//!   title's eShop icon (square, used as the cover) and its 16:9 banner
//!   (the hero art). The title ID comes from the dump's file name
//!   (`Hades [0100535012974000][v0].nsp`) or, failing that, from the
//!   emulator's own list of games it has run.
//! * Older systems use the libretro thumbnail collection, whose box art is
//!   named after No-Intro / Redump dump names ("Chrono Trigger (USA)").

use std::path::Path;

use crate::{net, ArtError, FoundArt};

const NLIB: &str = "https://api.nlib.cc/nx";
const LIBRETRO: &str = "https://thumbnails.libretro.com";

/// A game's 16-hex-digit Switch title ID from `[tags]` in its file name.
/// Only base games (ending in `000`) count; updates and add-ons have their
/// own IDs.
pub fn switch_title_id(file_stem: &str) -> Option<String> {
    file_stem
        .split('[')
        .skip(1)
        .filter_map(|s| s.split_once(']'))
        .map(|(tag, _)| tag.trim())
        .find(|t| t.len() == 16 && t.bytes().all(|b| b.is_ascii_hexdigit()) && t.ends_with("000"))
        .map(str::to_ascii_uppercase)
}

/// Whether a game file is a Switch game, by extension or emulator name.
pub fn is_switch(file: &Path, emulator: &str) -> bool {
    let ext = extension(file);
    let emu = emulator.to_ascii_lowercase();
    matches!(ext.as_str(), "nsp" | "xci" | "nro" | "nca")
        || ["ryujinx", "yuzu", "eden", "citron", "sudachi", "suyu"]
            .iter()
            .any(|n| emu.contains(n))
}

fn extension(file: &Path) -> String {
    file.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Downloads the icon (as the cover) and banner (as the hero) of a Switch
/// title, for whichever of `want`'s slots are empty.
pub async fn fetch_switch(
    http: &reqwest::Client,
    title_id: &str,
    want: &FoundArt,
    dir: &Path,
) -> Result<FoundArt, ArtError> {
    let mut out = FoundArt::default();
    if want.cover.is_none() {
        let url = format!("{NLIB}/{title_id}/icon/1024/1024");
        out.cover = net::download_if_found(http, &url, dir, "nx_icon", "jpg").await?;
    }
    if want.hero.is_none() {
        let url = format!("{NLIB}/{title_id}/banner/1920/1080");
        out.hero = net::download_if_found(http, &url, dir, "nx_banner", "jpg").await?;
    }
    Ok(out)
}

/// The libretro thumbnail systems a game file may belong to, most likely
/// first. Empty when the file says nothing about its system (a `.zip`
/// without a known emulator).
pub fn libretro_systems(file: &Path, emulator: &str) -> &'static [&'static str] {
    const PS1: &str = "Sony - PlayStation";
    const PS2: &str = "Sony - PlayStation 2";
    const GC: &str = "Nintendo - GameCube";
    const WII: &str = "Nintendo - Wii";
    let emu = emulator.to_ascii_lowercase();
    let ext = extension(file);
    // Disc images say little by themselves: the emulator decides.
    if emu.contains("pcsx2") {
        return &[PS2];
    }
    if emu.contains("rpcs3") {
        return &["Sony - PlayStation 3"];
    }
    if emu.contains("ppsspp") {
        return &["Sony - PlayStation Portable"];
    }
    if emu.contains("duckstation") {
        return &[PS1];
    }
    if emu.contains("dolphin") {
        return match ext.as_str() {
            "wbfs" | "wad" => &[WII],
            _ => &[GC, WII],
        };
    }
    match ext.as_str() {
        "nes" => &["Nintendo - Nintendo Entertainment System"],
        "sfc" | "smc" => &["Nintendo - Super Nintendo Entertainment System"],
        "gb" => &["Nintendo - Game Boy"],
        "gbc" => &["Nintendo - Game Boy Color"],
        "gba" => &["Nintendo - Game Boy Advance"],
        "n64" | "z64" | "v64" => &["Nintendo - Nintendo 64"],
        "nds" => &["Nintendo - Nintendo DS"],
        "3ds" | "cci" | "cia" => &["Nintendo - Nintendo 3DS"],
        "md" | "gen" | "smd" => &["Sega - Mega Drive - Genesis"],
        "sms" => &["Sega - Master System - Mark III"],
        "gg" => &["Sega - Game Gear"],
        "pce" => &["NEC - PC Engine - TurboGrafx 16"],
        "gcm" | "gcz" | "rvz" | "ciso" => &[GC, WII],
        "wbfs" | "wad" => &[WII],
        "cue" | "chd" | "m3u" | "pbp" => &[PS1, PS2],
        "cso" => &["Sony - PlayStation Portable", PS2],
        "iso" => &[PS2, GC, WII],
        _ => &[],
    }
}

/// libretro's file names replace these characters with `_`.
fn libretro_name(name: &str) -> String {
    name.chars()
        .map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c })
        .collect()
}

/// Names to try, best first: the dump's own name, then the clean title
/// with the usual region tags.
fn candidate_names(file_stem: &str, title: &str) -> Vec<String> {
    let mut names = vec![file_stem.to_string()];
    // "Game (USA) (Rev 1)" → "Game (USA)": revisions share the box.
    if let Some(i) = file_stem.find(')') {
        names.push(file_stem[..=i].to_string());
    }
    for region in ["USA", "World", "Europe", "USA, Europe", "Japan"] {
        names.push(format!("{title} ({region})"));
    }
    names.push(title.to_string());
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| !n.trim().is_empty() && seen.insert(n.clone()));
    names
}

/// Box art from the libretro thumbnails, as the cover.
pub async fn fetch_libretro(
    http: &reqwest::Client,
    systems: &[&str],
    file_stem: &str,
    title: &str,
    dir: &Path,
) -> Result<FoundArt, ArtError> {
    let mut out = FoundArt::default();
    let names = candidate_names(file_stem, title);
    for system in systems {
        for name in &names {
            let url = format!(
                "{LIBRETRO}/{}/Named_Boxarts/{}.png",
                net::encode_path_segment(system),
                net::encode_path_segment(&libretro_name(name))
            );
            if let Some(path) = net::download_if_found(http, &url, dir, "boxart", "png").await? {
                out.cover = Some(path);
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// Title IDs of the games an emulator has run, by normalized title: the
/// Ryujinx family keeps `games/<title id>/gui/metadata.json` with the
/// game's name in its data folder. For dumps named without their ID.
pub fn switch_title_ids_from_emulators() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for root in emulator_data_dirs() {
        let Ok(entries) = std::fs::read_dir(root.join("games")) else {
            continue;
        };
        for e in entries.flatten() {
            let id = e.file_name().to_string_lossy().to_ascii_uppercase();
            if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let meta = e.path().join("gui").join("metadata.json");
            let Ok(text) = std::fs::read_to_string(&meta) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if let Some(title) = v.get("title").and_then(|t| t.as_str()) {
                out.push((match_key(title), id));
            }
        }
    }
    out
}

/// Lowercase letters and digits only, so "The Legend of Zelda™: Breath of
/// the Wild" and "The Legend of Zelda Breath of the Wild" are equal.
pub fn match_key(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn emulator_data_dirs() -> Vec<std::path::PathBuf> {
    let Some(base) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let config = base.config_dir();
    ["Ryujinx", "ryujinx"]
        .iter()
        .map(|n| config.join(n))
        .filter(|p| p.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_ids() {
        assert_eq!(
            switch_title_id("Fire Emblem Engage [0100A6301214E000][v0](axekin.com)").as_deref(),
            Some("0100A6301214E000")
        );
        assert_eq!(switch_title_id("Hades [0100535012974800][v196608]"), None);
        assert_eq!(switch_title_id("Celeste"), None);
    }

    #[test]
    fn systems() {
        assert_eq!(
            libretro_systems(Path::new("a.sfc"), "RetroArch"),
            ["Nintendo - Super Nintendo Entertainment System"]
        );
        assert_eq!(
            libretro_systems(Path::new("a.iso"), "PCSX2"),
            ["Sony - PlayStation 2"]
        );
        assert!(libretro_systems(Path::new("a.zip"), "RetroArch").is_empty());
        assert!(is_switch(Path::new("a.nsp"), "Another emulator"));
    }

    #[test]
    fn names() {
        let n = candidate_names("Chrono Trigger (USA) (Rev 1)", "Chrono Trigger");
        assert_eq!(n[0], "Chrono Trigger (USA) (Rev 1)");
        assert_eq!(n[1], "Chrono Trigger (USA)");
        assert!(n.iter().filter(|x| *x == "Chrono Trigger (USA)").count() == 1);
        assert_eq!(libretro_name("Q*bert: Rebooted?"), "Q_bert_ Rebooted_");
    }

    #[test]
    fn keys() {
        assert_eq!(
            match_key("The Legend of Zelda™: Breath of the Wild"),
            match_key("The Legend of Zelda Breath of the Wild")
        );
    }
}
