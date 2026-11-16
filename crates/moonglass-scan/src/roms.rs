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
        if dir.extensions.contains(&ext) {
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
            let title = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            make(p, title)
        })
        .collect();
    games.extend(folders.iter().map(|p| {
        let title = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        make(p, title)
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
        assert_eq!(titles(&dir), ["Demon's Souls [BLUS30443]", "thing"]);
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
