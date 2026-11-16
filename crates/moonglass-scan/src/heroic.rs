//! Epic on Linux, through Heroic Games Launcher's Legendary backend.
//!
//! Heroic keeps Legendary's `installed.json` under its config folder
//! (`legendaryConfig/legendary/installed.json`), either in the native or in
//! the Flatpak location. The platform layer finds the file and the Heroic
//! command; this module only parses it and builds launch arguments.

use std::collections::BTreeMap;
use std::path::Path;

use moonglass_core::{GameId, Source};
use serde::Deserialize;

use crate::{read_to_string, ScanError, ScannedGame};

#[derive(Debug, Clone, Deserialize)]
pub struct InstalledGame {
    pub app_name: String,
    pub title: String,
    #[serde(default)]
    pub install_path: String,
    #[serde(default)]
    pub is_dlc: bool,
}

pub fn parse_installed(path: &Path, text: &str) -> Result<Vec<InstalledGame>, ScanError> {
    let map: BTreeMap<String, InstalledGame> =
        serde_json::from_str(text).map_err(|e| ScanError::parse(path, e))?;
    Ok(map.into_values().collect())
}

/// Scans Legendary's `installed.json`. A missing file means Heroic has no
/// Epic games installed, which is an empty list, not an error.
pub fn scan(installed_json: &Path) -> Result<Vec<ScannedGame>, ScanError> {
    if !installed_json.exists() {
        return Ok(Vec::new());
    }
    let text = read_to_string(installed_json)?;
    Ok(parse_installed(installed_json, &text)?
        .into_iter()
        .filter(|g| !g.is_dlc)
        .map(|g| ScannedGame {
            id: GameId::epic(&g.app_name),
            title: g.title,
            source: Source::Epic,
            source_id: Some(g.app_name),
            launch_target: None,
            install_dir: Some(g.install_path).filter(|s| !s.is_empty()),
            rom_directory_id: None,
            platform: None,
            last_played_hint: None,
        })
        .collect())
}

/// Arguments after the Heroic command (`heroic` or
/// `flatpak run com.heroicgameslauncher.hgl`). `--no-gui` keeps Heroic's
/// window from appearing, which Moonglass could not close on Wayland.
pub fn launch_args(app_name: &str) -> Vec<String> {
    vec![
        "--no-gui".to_string(),
        format!("heroic://launch?appName={app_name}&runner=legendary"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_skips_dlc() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("installed.json");
        std::fs::write(
            &p,
            r#"{
              "Kestrel": {"app_name": "Kestrel", "title": "Hades", "install_path": "/home/u/Games/Heroic/Hades", "is_dlc": false, "platform": "Windows", "executable": "x64/Hades.exe"},
              "KestrelDLC": {"app_name": "KestrelDLC", "title": "Hades OST", "install_path": "/x", "is_dlc": true}
            }"#,
        )
        .unwrap();
        let games = scan(&p).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].id.as_str(), "epic:Kestrel");
        assert_eq!(
            games[0].install_dir.as_deref(),
            Some("/home/u/Games/Heroic/Hades")
        );
        assert!(scan(&tmp.path().join("absent.json")).unwrap().is_empty());
    }

    #[test]
    fn args() {
        assert_eq!(
            launch_args("Kestrel"),
            [
                "--no-gui",
                "heroic://launch?appName=Kestrel&runner=legendary"
            ]
        );
    }
}
