//! Steam: `libraryfolders.vdf` and `appmanifest_*.acf` (identical on both
//! platforms).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use keyvalues_parser::{Obj, Value};
use moonglass_core::{GameId, Source};

use crate::{read_to_string, ScanError, ScannedGame};

/// Tools and runtimes that are "installed apps" to Steam but not games.
/// Users can extend this through Settings.
pub const DENY_APP_IDS: &[u32] = &[
    228980,  // Steamworks Common Redistributables
    250820,  // SteamVR
    1007,    // Steam Client
    961940,  // Proton 3.16
    930400,  // Proton 3.7
    1054830, // Proton 4.2
    1113280, // Proton 4.11
    1245040, // Proton 5.0
    1420170, // Proton 5.13
    1580130, // Proton 6.3
    1887720, // Proton 7.0
    2348590, // Proton 8.0
    2805730, // Proton 9.0
    1493710, // Proton Experimental
    2180100, // Proton Hotfix
    1826330, // Proton EasyAntiCheat Runtime
    1161040, // Proton BattlEye Runtime
    1070560, // Steam Linux Runtime 1.0 (scout)
    1391110, // Steam Linux Runtime 2.0 (soldier)
    1628350, // Steam Linux Runtime 3.0 (sniper)
];

/// New Proton and runtime releases get new AppIDs; catch them by name too.
const DENY_NAME_PREFIXES: &[&str] = &[
    "Proton ",
    "Steam Linux Runtime",
    "Steamworks Common Redistributables",
    "Steamworks SDK Redist",
];

/// `StateFlags` bit meaning the app's files are fully installed. Bit 2
/// ("update required") may be set alongside it; such games are still listed
/// because Steam updates them on launch.
const STATE_FULLY_INSTALLED: u64 = 4;

fn parse_vdf(path: &Path, text: &str) -> Result<Obj<'static>, ScanError> {
    // Steam escapes backslashes in paths; fall back to literal parsing for
    // the odd hand-edited file.
    let vdf = match keyvalues_parser::parse(text) {
        Ok(v) => v,
        Err(_) => keyvalues_parser::Parser::new()
            .literal_special_chars(true)
            .parse(text)
            .map_err(|e| ScanError::parse(path, e))?,
    };
    match vdf.value.into_owned() {
        Value::Obj(o) => Ok(o),
        Value::Str(_) => Err(ScanError::parse(path, "top-level value is not an object")),
    }
}

fn get_str<'a>(obj: &'a Obj<'_>, key: &str) -> Option<&'a str> {
    obj.get(key)?.first()?.get_str()
}

/// Library roots listed in `<steam_root>/steamapps/libraryfolders.vdf`,
/// always including `steam_root` itself, without duplicates.
pub fn library_paths(steam_root: &Path) -> Result<Vec<PathBuf>, ScanError> {
    let mut out = vec![steam_root.to_path_buf()];
    let vdf_path = steam_root.join("steamapps").join("libraryfolders.vdf");
    if !vdf_path.exists() {
        return Ok(out);
    }
    let obj = parse_vdf(&vdf_path, &read_to_string(&vdf_path)?)?;
    for values in obj.values() {
        for v in values {
            // New format: "0" { "path" "..." ... }. Old format: "1" "D:\\Lib".
            let path = match v {
                Value::Obj(o) => get_str(o, "path").map(str::to_string),
                Value::Str(s) => Some(s.to_string()),
            };
            if let Some(p) = path.filter(|p| !p.is_empty()) {
                let p = PathBuf::from(p);
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    // The old format also has non-numeric keys such as "TimeNextStatsReport".
    out.retain(|p| p.is_absolute());
    Ok(out)
}

/// One parsed `appmanifest_*.acf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppManifest {
    pub app_id: u32,
    pub name: String,
    pub install_dir: String,
    pub state_flags: u64,
    pub last_played: Option<i64>,
}

impl AppManifest {
    pub fn is_fully_installed(&self) -> bool {
        self.state_flags & STATE_FULLY_INSTALLED != 0
    }
}

pub fn parse_app_manifest(path: &Path, text: &str) -> Result<AppManifest, ScanError> {
    let obj = parse_vdf(path, text)?;
    let field =
        |k: &str| get_str(&obj, k).ok_or_else(|| ScanError::parse(path, format!("missing {k}")));
    let app_id = field("appid")?
        .parse()
        .map_err(|_| ScanError::parse(path, "appid is not a number"))?;
    Ok(AppManifest {
        app_id,
        name: field("name")?.to_string(),
        install_dir: field("installdir")?.to_string(),
        state_flags: get_str(&obj, "StateFlags")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        last_played: get_str(&obj, "LastPlayed")
            .and_then(|s| s.parse().ok())
            .filter(|&t: &i64| t > 0),
    })
}

fn is_denied(m: &AppManifest, extra_deny: &HashSet<u32>) -> bool {
    DENY_APP_IDS.contains(&m.app_id)
        || extra_deny.contains(&m.app_id)
        || DENY_NAME_PREFIXES.iter().any(|p| m.name.starts_with(p))
}

/// Scans every library of the Steam install at `steam_root`.
///
/// A library that cannot be read (an unplugged drive) is skipped with a
/// warning rather than failing the whole scan; failing to read the root
/// library is an error.
pub fn scan(steam_root: &Path, extra_deny: &HashSet<u32>) -> Result<Vec<ScannedGame>, ScanError> {
    let mut games = Vec::new();
    let mut seen = HashSet::new();
    for (i, lib) in library_paths(steam_root)?.into_iter().enumerate() {
        let steamapps = lib.join("steamapps");
        let entries = match std::fs::read_dir(&steamapps) {
            Ok(e) => e,
            Err(e) if i == 0 => return Err(ScanError::io(&steamapps, e)),
            Err(e) => {
                tracing::warn!(library = %lib.display(), error = %e, "skipping unreadable Steam library");
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let manifest = match read_to_string(&path).and_then(|t| parse_app_manifest(&path, &t)) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping bad Steam manifest");
                    continue;
                }
            };
            if !manifest.is_fully_installed()
                || is_denied(&manifest, extra_deny)
                || !seen.insert(manifest.app_id)
            {
                continue;
            }
            let app_id = manifest.app_id.to_string();
            let install_dir = steamapps.join("common").join(&manifest.install_dir);
            games.push(ScannedGame {
                id: GameId::steam(&app_id),
                title: manifest.name,
                source: Source::Steam,
                source_id: Some(app_id),
                launch_target: None,
                install_dir: Some(install_dir.to_string_lossy().into_owned()),
                rom_directory_id: None,
                platform: None,
                last_played_hint: manifest.last_played,
            });
        }
    }
    Ok(games)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const LIBRARYFOLDERS: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"ROOT"
		"label"		""
		"apps" { "620" "123" }
	}
	"1"
	{
		"path"		"LIB2"
		"apps" { "1145360" "456" }
	}
	"2"
	{
		"path"		"MISSING"
	}
}
"#;

    fn manifest(appid: u32, name: &str, dir: &str, flags: u32, last: i64) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"StateFlags\"\t\t\"{flags}\"\n\t\"installdir\"\t\t\"{dir}\"\n\t\"LastPlayed\"\t\t\"{last}\"\n}}\n"
        )
    }

    fn vdf_path(p: &Path) -> String {
        p.to_string_lossy().replace('\\', "\\\\")
    }

    #[test]
    fn scans_all_libraries_and_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Steam");
        let lib2 = tmp.path().join("Lib Two");
        let missing = tmp.path().join("Unplugged");
        fs::create_dir_all(root.join("steamapps")).unwrap();
        fs::create_dir_all(lib2.join("steamapps")).unwrap();
        fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            LIBRARYFOLDERS
                .replace("ROOT", &vdf_path(&root))
                .replace("LIB2", &vdf_path(&lib2))
                .replace("MISSING", &vdf_path(&missing)),
        )
        .unwrap();
        let sa = root.join("steamapps");
        fs::write(
            sa.join("appmanifest_620.acf"),
            manifest(620, "Portal 2", "Portal 2", 4, 1700000000),
        )
        .unwrap();
        fs::write(
            sa.join("appmanifest_228980.acf"),
            manifest(
                228980,
                "Steamworks Common Redistributables",
                "Steamworks Shared",
                4,
                0,
            ),
        )
        .unwrap();
        fs::write(
            sa.join("appmanifest_9999999.acf"),
            manifest(9999999, "Proton 11.0", "Proton 11.0", 4, 0),
        )
        .unwrap();
        fs::write(
            sa.join("appmanifest_70.acf"),
            manifest(70, "Half-Life", "Half-Life", 1026, 0),
        )
        .unwrap(); // downloading
        fs::write(sa.join("appmanifest_broken.acf"), "not a manifest {").unwrap();
        let sa2 = lib2.join("steamapps");
        fs::write(
            sa2.join("appmanifest_1145360.acf"),
            manifest(1145360, "Hades", "Hades", 6, 0),
        )
        .unwrap(); // update required
        fs::write(
            sa2.join("appmanifest_400.acf"),
            manifest(400, "Portal", "Portal", 4, 0),
        )
        .unwrap();

        let mut games = scan(&root, &HashSet::from([400])).unwrap();
        games.sort_by(|a, b| a.title.cmp(&b.title));
        let titles: Vec<_> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Hades", "Portal 2"]);

        let portal = &games[1];
        assert_eq!(portal.id.as_str(), "steam:620");
        assert_eq!(portal.last_played_hint, Some(1700000000));
        assert_eq!(
            PathBuf::from(portal.install_dir.as_ref().unwrap()),
            sa.join("common").join("Portal 2")
        );
        assert_eq!(games[0].last_played_hint, None);
        assert_eq!(
            PathBuf::from(games[0].install_dir.as_ref().unwrap()),
            sa2.join("common").join("Hades")
        );
    }

    #[test]
    fn old_libraryfolders_format() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Steam");
        fs::create_dir_all(root.join("steamapps")).unwrap();
        let lib = tmp.path().join("Lib");
        fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"LibraryFolders\"\n{{\n\"TimeNextStatsReport\" \"1234\"\n\"ContentStatsID\" \"-42\"\n\"1\" \"{}\"\n}}\n",
                vdf_path(&lib)
            ),
        )
        .unwrap();
        assert_eq!(library_paths(&root).unwrap(), [root.clone(), lib]);
    }

    #[test]
    fn missing_root_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan(&tmp.path().join("nope"), &HashSet::new()).is_err());
    }
}
