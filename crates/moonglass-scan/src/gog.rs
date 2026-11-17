//! GOG games.
//!
//! * Windows: every GOG install, through GOG Galaxy or an offline
//!   installer, registers itself under
//!   `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<product ID>`. The platform
//!   layer reads those keys; [`from_registry`] turns them into games.
//! * Linux: Heroic's GOG backend lists installs in
//!   `gog_store/installed.json` under its config folder ([`scan_heroic`]).
//!
//! Either way the install folder holds `goggame-<product ID>.info`, GOG's
//! own description of the game: its name, whether it is an add-on of
//! another product (`rootGameId`), and its play tasks. It is preferred
//! over the other sources' names, which can be abbreviated.

use std::path::{Path, PathBuf};

use moonglass_core::{GameId, Source};
use serde::Deserialize;

use crate::{read_to_string, ScanError, ScannedGame};

/// One `goggame-<id>.info` file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    #[serde(default)]
    pub game_id: String,
    #[serde(default)]
    pub root_game_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub play_tasks: Vec<PlayTask>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayTask {
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub arguments: String,
}

impl GameInfo {
    /// An add-on (DLC) installed into another game's folder.
    pub fn is_addon(&self) -> bool {
        !self.root_game_id.is_empty() && self.root_game_id != self.game_id
    }

    /// The primary play task's program, relative to the install folder.
    pub fn primary_executable(&self) -> Option<&str> {
        self.play_tasks
            .iter()
            .find(|t| t.is_primary && t.kind == "FileTask" && !t.path.is_empty())
            .map(|t| t.path.as_str())
    }
}

/// Reads `<install_dir>/goggame-<product_id>.info`. `None` when it is
/// missing or unreadable: older installs may not have one.
pub fn read_info(install_dir: &Path, product_id: &str) -> Option<GameInfo> {
    let path = install_dir.join(format!("goggame-{product_id}.info"));
    let text = read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(info) => Some(info),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "unreadable GOG game info");
            None
        }
    }
}

/// One `GOG.com\Games\<id>` registry key, as the platform layer read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryGame {
    /// The key name: the product ID.
    pub product_id: String,
    /// `gameName`.
    pub name: String,
    /// `path`: the install folder.
    pub path: String,
    /// `exe`: the program GOG starts, absolute.
    pub exe: String,
    /// `dependsOn`: set for add-ons, which are not games of their own.
    pub depends_on: String,
}

fn numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Games from the registry. Add-ons, entries whose folder is gone (a
/// leftover key after an uninstall) and non-numeric IDs are skipped.
pub fn from_registry(entries: impl IntoIterator<Item = RegistryGame>) -> Vec<ScannedGame> {
    entries
        .into_iter()
        .filter(|e| numeric(&e.product_id) && e.depends_on.trim().is_empty())
        .filter(|e| Path::new(&e.path).is_dir())
        .filter_map(|e| {
            let info = read_info(Path::new(&e.path), &e.product_id);
            if info.as_ref().is_some_and(GameInfo::is_addon) {
                return None;
            }
            let title = info
                .map(|i| i.name)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(e.name);
            Some(ScannedGame {
                id: GameId::gog(&e.product_id),
                title,
                source: Source::Gog,
                source_id: Some(e.product_id),
                launch_target: Some(e.exe).filter(|s| !s.is_empty()),
                install_dir: Some(e.path),
                rom_directory_id: None,
                platform: None,
                last_played_hint: None,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct HeroicInstalled {
    #[serde(default)]
    installed: Vec<HeroicGame>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeroicGame {
    app_name: String,
    #[serde(default, rename = "install_path")]
    install_path: String,
    #[serde(default, rename = "is_dlc")]
    is_dlc: bool,
}

/// Scans Heroic's `gog_store/installed.json`. A missing file means no GOG
/// games are installed through Heroic: an empty list, not an error.
pub fn scan_heroic(installed_json: &Path) -> Result<Vec<ScannedGame>, ScanError> {
    if !installed_json.exists() {
        return Ok(Vec::new());
    }
    let text = read_to_string(installed_json)?;
    let parsed: HeroicInstalled =
        serde_json::from_str(&text).map_err(|e| ScanError::parse(installed_json, e))?;
    Ok(parsed
        .installed
        .into_iter()
        .filter(|g| !g.is_dlc && numeric(&g.app_name) && !g.install_path.is_empty())
        .filter_map(|g| {
            let dir = PathBuf::from(&g.install_path);
            let info = read_info(&dir, &g.app_name);
            if info.as_ref().is_some_and(GameInfo::is_addon) {
                return None;
            }
            let title = info
                .map(|i| i.name)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| Some(dir.file_name()?.to_string_lossy().into_owned()))?;
            Some(ScannedGame {
                id: GameId::gog(&g.app_name),
                title,
                source: Source::Gog,
                source_id: Some(g.app_name),
                launch_target: None,
                install_dir: Some(g.install_path),
                rom_directory_id: None,
                platform: None,
                last_played_hint: None,
            })
        })
        .collect())
}

/// Arguments after the Heroic command, as for Epic games
/// ([`crate::heroic::launch_args`]) but with Heroic's GOG runner.
pub fn heroic_launch_args(product_id: &str) -> Vec<String> {
    vec![
        "--no-gui".to_string(),
        format!("heroic://launch?appName={product_id}&runner=gog"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn info(dir: &Path, id: &str, root: &str, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(format!("goggame-{id}.info")),
            serde_json::json!({
                "gameId": id,
                "rootGameId": root,
                "name": name,
                "playTasks": [
                    {"isPrimary": false, "type": "URLTask", "link": "https://example.com"},
                    {"isPrimary": true, "type": "FileTask", "path": "System\\witcher.exe", "arguments": ""}
                ]
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn registry_games_skip_addons_and_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let witcher = tmp.path().join("The Witcher");
        info(
            &witcher,
            "1207658924",
            "1207658924",
            "The Witcher: Enhanced Edition",
        );
        let gwent = tmp.path().join("Gwent");
        fs::create_dir_all(&gwent).unwrap();
        let dlc = tmp.path().join("Addon");
        info(&dlc, "1400000000", "1207658924", "Some DLC");
        let entry = |id: &str, name: &str, path: &Path| RegistryGame {
            product_id: id.into(),
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
            exe: path.join("game.exe").to_string_lossy().into_owned(),
            depends_on: String::new(),
        };
        let games = from_registry([
            entry("1207658924", "The Witcher", &witcher),
            // No .info file: the registry's name is used.
            entry("1971477531", "Gwent", &gwent),
            // An add-on, told apart by its info file.
            entry("1400000000", "Some DLC", &dlc),
            // An add-on, told apart by the registry.
            RegistryGame {
                depends_on: "1207658924".into(),
                ..entry("1500000000", "Other DLC", &gwent)
            },
            // Uninstalled but the key was left behind.
            entry("1600000000", "Gone", &tmp.path().join("missing")),
            entry("not-a-number", "Odd", &gwent),
        ]);
        let titles: Vec<_> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["The Witcher: Enhanced Edition", "Gwent"]);
        assert_eq!(games[0].id.as_str(), "gog:1207658924");
        assert_eq!(games[0].source, Source::Gog);
        assert!(games[0]
            .launch_target
            .as_deref()
            .unwrap()
            .ends_with("game.exe"));
    }

    #[test]
    fn info_primary_task() {
        let tmp = tempfile::tempdir().unwrap();
        info(tmp.path(), "1", "1", "Game");
        let i = read_info(tmp.path(), "1").unwrap();
        assert_eq!(i.primary_executable(), Some("System\\witcher.exe"));
        assert!(!i.is_addon());
        assert!(read_info(tmp.path(), "2").is_none());
    }

    #[test]
    fn heroic_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path().join("Games/The Witcher");
        info(&game_dir, "1207658924", "1207658924", "The Witcher");
        let bare = tmp.path().join("Games/Bare Game");
        fs::create_dir_all(&bare).unwrap();
        let p = tmp.path().join("installed.json");
        fs::write(
            &p,
            serde_json::json!({"installed": [
                {"appName": "1207658924", "install_path": game_dir, "is_dlc": false, "platform": "windows"},
                {"appName": "1111", "install_path": bare, "is_dlc": false},
                {"appName": "2222", "install_path": bare, "is_dlc": true},
            ]})
            .to_string(),
        )
        .unwrap();
        let games = scan_heroic(&p).unwrap();
        let titles: Vec<_> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["The Witcher", "Bare Game"]);
        assert!(scan_heroic(&tmp.path().join("absent.json"))
            .unwrap()
            .is_empty());
        fs::write(&p, "{ broken").unwrap();
        assert!(scan_heroic(&p).is_err());
    }

    #[test]
    fn heroic_args() {
        assert_eq!(
            heroic_launch_args("1207658924"),
            ["--no-gui", "heroic://launch?appName=1207658924&runner=gog"]
        );
    }
}
