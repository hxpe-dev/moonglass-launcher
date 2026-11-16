//! Epic Games Launcher on Windows: `.item` manifests in
//! `C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests`.

use std::path::Path;

use moonglass_core::{GameId, Source};
use serde::Deserialize;

use crate::{read_to_string, ScanError, ScannedGame};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemManifest {
    pub display_name: String,
    pub app_name: String,
    #[serde(default)]
    pub install_location: String,
    #[serde(default)]
    pub catalog_namespace: String,
    #[serde(default)]
    pub catalog_item_id: String,
    #[serde(default)]
    pub main_game_app_name: String,
    #[serde(default)]
    pub app_categories: Vec<String>,
    #[serde(default, rename = "bIsApplication")]
    pub is_application: bool,
    #[serde(default, rename = "bIsIncompleteInstall")]
    pub is_incomplete_install: bool,
}

impl ItemManifest {
    /// Applications in a games category, not DLC/add-ons, not Unreal Engine
    /// installs, and fully installed.
    pub fn is_listable_game(&self) -> bool {
        let is_addon =
            !self.main_game_app_name.is_empty() && self.main_game_app_name != self.app_name;
        let is_engine =
            self.app_name.starts_with("UE_") || self.app_categories.iter().any(|c| c == "engines");
        self.is_application
            && self.app_categories.iter().any(|c| c == "games")
            && !is_addon
            && !is_engine
            && !self.is_incomplete_install
    }

    /// Primary launch URI, using the bare `AppName`.
    pub fn launch_uri(&self) -> String {
        launch_uri(&self.app_name)
    }

    /// Fallback for titles that do not launch from the bare `AppName`.
    pub fn fallback_launch_uri(&self) -> String {
        format!(
            "com.epicgames.launcher://apps/{}%3A{}%3A{}?action=launch&silent=true",
            self.catalog_namespace, self.catalog_item_id, self.app_name
        )
    }
}

pub fn launch_uri(app_name: &str) -> String {
    format!("com.epicgames.launcher://apps/{app_name}?action=launch&silent=true")
}

fn read_manifests(dir: &Path) -> Result<Vec<ItemManifest>, ScanError> {
    let entries = std::fs::read_dir(dir).map_err(|e| ScanError::io(dir, e))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("item") {
            continue;
        }
        match read_to_string(&path).and_then(|t| {
            serde_json::from_str::<ItemManifest>(&t).map_err(|e| ScanError::parse(&path, e))
        }) {
            Ok(m) => out.push(m),
            Err(e) => tracing::warn!(error = %e, "skipping bad Epic manifest"),
        }
    }
    Ok(out)
}

/// Finds one game's manifest, for the namespaced launch fallback.
pub fn find_manifest(dir: &Path, app_name: &str) -> Result<Option<ItemManifest>, ScanError> {
    Ok(read_manifests(dir)?
        .into_iter()
        .find(|m| m.app_name == app_name))
}

pub fn scan(manifest_dir: &Path) -> Result<Vec<ScannedGame>, ScanError> {
    Ok(read_manifests(manifest_dir)?
        .into_iter()
        .filter(ItemManifest::is_listable_game)
        .map(|m| ScannedGame {
            id: GameId::epic(&m.app_name),
            title: m.display_name,
            source: Source::Epic,
            source_id: Some(m.app_name),
            launch_target: None,
            install_dir: Some(m.install_location).filter(|s| !s.is_empty()),
            rom_directory_id: None,
            platform: None,
            last_played_hint: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn item(app: &str, name: &str, cats: &[&str], app_flag: bool, main: &str) -> String {
        serde_json::json!({
            "FormatVersion": 0,
            "bIsIncompleteInstall": false,
            "DisplayName": name,
            "AppName": app,
            "InstallLocation": format!("C:\\Games\\{name}"),
            "CatalogNamespace": "ns1",
            "CatalogItemId": "item1",
            "MainGameAppName": main,
            "AppCategories": cats,
            "bIsApplication": app_flag,
        })
        .to_string()
    }

    #[test]
    fn filters_dlc_engines_and_non_games() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::write(
            d.join("a.item"),
            item(
                "Kestrel",
                "Hades",
                &["public", "games", "applications"],
                true,
                "Kestrel",
            ),
        )
        .unwrap();
        fs::write(
            d.join("b.item"),
            item(
                "KestrelDLC",
                "Hades Soundtrack",
                &["public", "addons"],
                true,
                "Kestrel",
            ),
        )
        .unwrap();
        fs::write(
            d.join("c.item"),
            item(
                "UE_5.3",
                "Unreal Engine 5.3",
                &["public", "engines"],
                true,
                "UE_5.3",
            ),
        )
        .unwrap();
        fs::write(
            d.join("d.item"),
            item(
                "Tool",
                "Some Tool",
                &["public", "applications"],
                true,
                "Tool",
            ),
        )
        .unwrap();
        fs::write(
            d.join("e.item"),
            item("Old", "Old Format Game", &["games"], true, ""),
        )
        .unwrap();
        fs::write(d.join("f.item"), "{ broken").unwrap();
        fs::write(d.join("readme.txt"), "ignored").unwrap();

        let mut games = scan(d).unwrap();
        games.sort_by(|a, b| a.title.cmp(&b.title));
        let ids: Vec<_> = games.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["epic:Kestrel", "epic:Old"]);
        assert_eq!(games[0].install_dir.as_deref(), Some("C:\\Games\\Hades"));
    }

    #[test]
    fn uris() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("a.item"),
            item("Kestrel", "Hades", &["games"], true, "Kestrel"),
        )
        .unwrap();
        let m = find_manifest(tmp.path(), "Kestrel").unwrap().unwrap();
        assert_eq!(
            m.launch_uri(),
            "com.epicgames.launcher://apps/Kestrel?action=launch&silent=true"
        );
        assert_eq!(
            m.fallback_launch_uri(),
            "com.epicgames.launcher://apps/ns1%3Aitem1%3AKestrel?action=launch&silent=true"
        );
    }
}
