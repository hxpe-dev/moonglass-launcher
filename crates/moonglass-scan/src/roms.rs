//! ROM directories (spec 2, "Emulated ROMs").
//!
//! Rules:
//! * one entry per `.m3u` playlist; the discs it lists are hidden;
//! * a `.cue` sheet hides the `.bin` files it references;
//! * folder-based games are found by the profile's marker, a path relative
//!   to the game folder (`PS3_GAME/USRDIR/EBOOT.BIN` for RPCS3). The folder
//!   becomes the entry and is not searched further.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use moonglass_core::{GameId, Source};
use walkdir::WalkDir;

use crate::{read_to_string, Normalizer, ScanError, ScannedGame};

/// A registered ROM directory joined with its emulator profile's scanning
/// rules.
#[derive(Debug, Clone)]
pub struct RomDirectory {
    pub id: String,
    pub path: PathBuf,
    pub platform: String,
    pub recursive: bool,
    /// Accepted extensions as stored in the profile: ".iso,.rvz" or "iso".
    pub extensions: Vec<String>,
    pub folder_marker: Option<String>,
}

/// Parses the profile's `file_extensions` column (".iso,.rvz,.gcm").
pub fn parse_extensions(s: &str) -> Vec<String> {
    s.split(',')
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

fn ext_lower(p: &Path) -> Option<String> {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// Files referenced by a playlist or cue sheet, resolved against its folder.
fn referenced_files(sheet: &Path, kind: &str) -> Vec<PathBuf> {
    let Ok(text) = read_to_string(sheet) else {
        return Vec::new();
    };
    let base = sheet.parent().unwrap_or(Path::new(""));
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if kind == "m3u" {
                // Lines are paths; `#` starts a comment or directive.
                (!line.is_empty() && !line.starts_with('#')).then(|| line.to_string())
            } else {
                // FILE "Track 01.bin" BINARY
                let rest = line
                    .strip_prefix("FILE ")
                    .or_else(|| line.strip_prefix("file "))?;
                let rest = rest.trim();
                if let Some(q) = rest.strip_prefix('"') {
                    q.split('"').next().map(str::to_string)
                } else {
                    rest.split_whitespace().next().map(str::to_string)
                }
            }
        })
        .map(|name| base.join(name))
        .collect()
}

/// The text of each `[tag]` in a file name.
fn bracket_tags(stem: &str) -> impl Iterator<Item = &str> {
    stem.split('[')
        .skip(1)
        .filter_map(|s| s.split_once(']'))
        .map(|(tag, _)| tag.trim())
}

/// Whether a Switch `.nsp` is an update or add-on content rather than a
/// game. Dumps carry the title ID in brackets: a game's ends in `000`.
fn is_switch_addon(stem: &str) -> bool {
    bracket_tags(stem).any(|tag| {
        let is_title_id = tag.len() == 16 && tag.bytes().all(|b| b.is_ascii_hexdigit());
        (is_title_id && !tag.ends_with("000"))
            || ["UPD", "UPDATE", "DLC"]
                .iter()
                .any(|t| tag.eq_ignore_ascii_case(t))
    })
}

/// A title from a file name: the tags at its end are dropped, as in
/// `Hades [0100535012974000][v0]` or `Chrono Trigger (USA) (Rev 1)`. Disc
/// numbers stay, so the discs of one game can be told apart.
fn clean_title(stem: &str) -> String {
    let mut title = stem.trim_end();
    loop {
        let open = match title.chars().last() {
            Some(']') => '[',
            Some(')') => '(',
            _ => break,
        };
        let Some(at) = title.rfind(open) else { break };
        let tag = title[at + 1..title.len() - 1].trim().to_ascii_lowercase();
        if tag.starts_with("disc") || tag.starts_with("disk") {
            break;
        }
        title = title[..at].trim_end();
    }
    if title.is_empty() { stem } else { title }.to_string()
}

pub fn scan(dir: &RomDirectory, normalize: Normalizer<'_>) -> Result<Vec<ScannedGame>, ScanError> {
    std::fs::read_dir(&dir.path).map_err(|e| ScanError::io(&dir.path, e))?;

    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    let mut hidden: HashSet<String> = HashSet::new();

    let mut walk = WalkDir::new(&dir.path).follow_links(true).min_depth(1);
    if !dir.recursive {
        walk = walk.max_depth(1);
    }
    let mut it = walk.into_iter();
    while let Some(entry) = it.next() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "skipping unreadable entry in ROM directory");
                continue;
            }
        };
        let path = entry.path();
        if entry.file_type().is_dir() {
            if let Some(marker) = &dir.folder_marker {
                if path.join(marker).is_file() {
                    folders.push(path.to_path_buf());
                    it.skip_current_dir();
                }
            }
            continue;
        }
        let Some(ext) = ext_lower(path) else { continue };
        if ext == "m3u" || ext == "cue" {
            for f in referenced_files(path, &ext) {
                hidden.insert(normalize(&f));
            }
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        if dir.extensions.contains(&ext) && !(ext == "nsp" && is_switch_addon(&stem)) {
            candidates.push(path.to_path_buf());
        }
    }

    let make = |path: &Path, title: String| {
        let normalized = normalize(path);
        ScannedGame {
            id: GameId::rom(&normalized),
            title,
            source: Source::Rom,
            source_id: None,
            launch_target: Some(path.to_string_lossy().into_owned()),
            install_dir: None,
            rom_directory_id: Some(dir.id.clone()),
            platform: Some(dir.platform.clone()),
            last_played_hint: None,
        }
    };

    let mut games: Vec<ScannedGame> = candidates
        .iter()
        .filter(|p| !hidden.contains(&normalize(p)))
        .map(|p| {
            let stem = p.file_stem().unwrap_or_default().to_string_lossy();
            make(p, clean_title(&stem))
        })
        .collect();
    games.extend(folders.iter().map(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        make(p, clean_title(&name))
    }));
    Ok(games)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn norm(p: &Path) -> String {
        // Tests only need a stable key; real normalization lives in the
        // platform layer.
        p.to_string_lossy().replace('\\', "/").to_lowercase()
    }

    fn touch(p: &Path, content: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn titles(dir: &RomDirectory) -> Vec<String> {
        let mut t: Vec<_> = scan(dir, &norm)
            .unwrap()
            .into_iter()
            .map(|g| g.title)
            .collect();
        t.sort();
        t
    }

    #[test]
    fn playlists_cues_and_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        touch(
            &r.join("FF7/Final Fantasy VII.m3u"),
            "# discs\nFF7 (Disc 1).cue\nFF7 (Disc 2).cue\n",
        );
        touch(
            &r.join("FF7/FF7 (Disc 1).cue"),
            "FILE \"FF7 (Disc 1).bin\" BINARY\n",
        );
        touch(&r.join("FF7/FF7 (Disc 1).bin"), "");
        touch(
            &r.join("FF7/FF7 (Disc 2).cue"),
            "FILE \"FF7 (Disc 2).bin\" BINARY\n",
        );
        touch(&r.join("FF7/FF7 (Disc 2).bin"), "");
        touch(
            &r.join("Crash.CUE"),
            "FILE \"Crash (Track 1).bin\" BINARY\nFILE \"Crash (Track 2).bin\" BINARY\n",
        );
        touch(&r.join("Crash (Track 1).bin"), "");
        touch(&r.join("Crash (Track 2).bin"), "");
        touch(&r.join("Spyro.chd"), "");
        touch(&r.join("notes.txt"), "");

        let dir = RomDirectory {
            id: "d1".into(),
            path: r.to_path_buf(),
            platform: "PlayStation".into(),
            recursive: true,
            extensions: parse_extensions(".m3u,.cue,.bin,.chd"),
            folder_marker: None,
        };
        assert_eq!(titles(&dir), ["Crash", "Final Fantasy VII", "Spyro"]);

        let flat = RomDirectory {
            recursive: false,
            ..dir.clone()
        };
        assert_eq!(titles(&flat), ["Crash", "Spyro"]);

        let g = scan(&dir, &norm)
            .unwrap()
            .into_iter()
            .find(|g| g.title == "Spyro")
            .unwrap();
        assert_eq!(g.platform.as_deref(), Some("PlayStation"));
        assert_eq!(g.rom_directory_id.as_deref(), Some("d1"));
        assert!(g.id.as_str().starts_with("rom:"));
        assert_eq!(PathBuf::from(g.launch_target.unwrap()), r.join("Spyro.chd"));
    }

    #[test]
    fn folder_games() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        touch(
            &r.join("Demon's Souls [BLUS30443]/PS3_GAME/USRDIR/EBOOT.BIN"),
            "",
        );
        touch(
            &r.join("Demon's Souls [BLUS30443]/PS3_GAME/USRDIR/data.pkg"),
            "",
        );
        touch(&r.join("Loose/thing.pkg"), "");
        let dir = RomDirectory {
            id: "d2".into(),
            path: r.to_path_buf(),
            platform: "PS3".into(),
            recursive: true,
            extensions: parse_extensions(".pkg"),
            folder_marker: Some("PS3_GAME/USRDIR/EBOOT.BIN".into()),
        };
        // The game folder is one entry; files inside it are not scanned.
        assert_eq!(titles(&dir), ["Demon's Souls", "thing"]);
    }

    #[test]
    fn switch_games_without_their_updates_and_add_ons() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        for name in [
            "Hades [0100535012974000][v0].nsp",
            "Hades [0100535012974800][v196608].nsp",
            "Hades [0100535012975001][v0].nsp",
            "Celeste [UPD].nsp",
            "Celeste.xci",
            "Tetris (World) (En,Ja).nsp",
        ] {
            touch(&r.join(name), "");
        }
        let dir = RomDirectory {
            id: "d3".into(),
            path: r.to_path_buf(),
            platform: "Switch".into(),
            recursive: true,
            extensions: parse_extensions(".nsp,.xci,.nro"),
            folder_marker: None,
        };
        assert_eq!(titles(&dir), ["Celeste", "Hades", "Tetris"]);
    }

    #[test]
    fn titles_lose_their_trailing_tags() {
        assert_eq!(
            clean_title("Chrono Trigger (USA) (Rev 1)"),
            "Chrono Trigger"
        );
        assert_eq!(clean_title("FF7 (Disc 1)"), "FF7 (Disc 1)");
        assert_eq!(clean_title("[BLUS30443]"), "[BLUS30443]");
        assert_eq!(clean_title("Odd ) name"), "Odd ) name");
    }

    #[test]
    fn extensions_parse() {
        assert_eq!(parse_extensions(".iso, .RVZ,gcm,,"), ["iso", "rvz", "gcm"]);
    }

    #[test]
    fn missing_directory_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RomDirectory {
            id: "x".into(),
            path: tmp.path().join("gone"),
            platform: "X".into(),
            recursive: true,
            extensions: vec!["iso".into()],
            folder_marker: None,
        };
        assert!(scan(&dir, &norm).is_err());
    }
}
