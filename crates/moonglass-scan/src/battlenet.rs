//! Battle.net games (Windows).
//!
//! Battle.net registers every game it installs in Windows' list of
//! installed programs, with an uninstall command that names the game's
//! product UID: `"…\Blizzard Uninstaller.exe" --lang=enUS --uid=s2
//! --displayname="StarCraft II"`. The platform layer reads those entries;
//! [`from_uninstall`] turns them into games.
//!
//! Battle.net starts a game from its product code (`S2`, `WoW`, …), which
//! is not the UID: [`launch_code`] knows the usual ones. A game it does not
//! know is started through its own `<Game> Launcher.exe`, which hands the
//! start to Battle.net.

use std::path::Path;

use moonglass_core::{GameId, Source};

use crate::ScannedGame;

/// One Blizzard entry in Windows' installed programs, as the platform
/// layer read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallEntry {
    /// `DisplayName`.
    pub name: String,
    /// `InstallLocation`.
    pub install_location: String,
    /// `UninstallString`, which carries the product UID.
    pub uninstall_string: String,
}

/// The `--uid=` of a Blizzard uninstall command.
pub fn uid(uninstall_string: &str) -> Option<&str> {
    let rest = uninstall_string.split("--uid=").nth(1)?;
    let rest = rest.trim_start_matches('"');
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"')
        .unwrap_or(rest.len());
    let uid = &rest[..end];
    let ok = !uid.is_empty()
        && uid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.');
    ok.then_some(uid)
}

/// Battle.net's product code for a UID: what `Battle.net.exe --exec="launch
/// <code>"` expects.
pub fn launch_code(uid: &str) -> Option<&'static str> {
    Some(match uid {
        "wow" => "WoW",
        "wow_classic" => "WoWC",
        "d3" => "D3",
        "fenris" => "Fen",
        "osi" => "OSI",
        "anbs" => "ANBS",
        "s1" => "S1",
        "s2" => "S2",
        "w3" => "W3",
        "hs_beta" => "WTCG",
        "heroes" => "Hero",
        "prometheus" => "Pro",
        "rtro" => "RTRO",
        "viper" => "VIPR",
        "odin" => "ODIN",
        "lazarus" => "LAZR",
        "zeus" => "ZEUS",
        "fore" => "FORE",
        "auks" => "AUKS",
        "wlby" => "WLBY",
        _ => return None,
    })
}

/// The game's own `<Game> Launcher.exe` at the top of its folder, if it
/// has one.
fn own_launcher(install_dir: &Path) -> Option<String> {
    std::fs::read_dir(install_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .is_some_and(|n| n.ends_with(" launcher.exe"))
        })
        .map(|p| p.to_string_lossy().into_owned())
}

/// Games from Windows' installed programs. Battle.net itself, entries
/// without a UID and entries whose folder is gone are skipped.
pub fn from_uninstall(entries: impl IntoIterator<Item = UninstallEntry>) -> Vec<ScannedGame> {
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter_map(|e| {
            let uid = uid(&e.uninstall_string)?.to_string();
            if uid.eq_ignore_ascii_case("battle.net") || !seen.insert(uid.clone()) {
                return None;
            }
            let dir = Path::new(&e.install_location);
            if e.install_location.trim().is_empty() || !dir.is_dir() {
                return None;
            }
            let title = Some(e.name.trim().to_string())
                .filter(|n| !n.is_empty())
                .or_else(|| Some(dir.file_name()?.to_string_lossy().into_owned()))?;
            Some(ScannedGame {
                id: GameId::battlenet(&uid),
                title,
                source: Source::BattleNet,
                launch_target: own_launcher(dir),
                source_id: Some(uid),
                install_dir: Some(e.install_location),
                rom_directory_id: None,
                platform: None,
                last_played_hint: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn entry(name: &str, dir: &Path, uid: &str) -> UninstallEntry {
        UninstallEntry {
            name: name.into(),
            install_location: dir.to_string_lossy().into_owned(),
            uninstall_string: format!(
                r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS --uid={uid} --displayname="{name}""#
            ),
        }
    }

    #[test]
    fn reads_the_uid() {
        assert_eq!(
            uid(
                r#""C:\x\Blizzard Uninstaller.exe" --lang=enGB --uid=battle.net --displayname="Battle.net""#
            ),
            Some("battle.net")
        );
        assert_eq!(uid("x --uid=wow_classic"), Some("wow_classic"));
        assert_eq!(uid(r#"x --uid="s2" y"#), Some("s2"));
        assert_eq!(uid("x --uid= y"), None);
        assert_eq!(uid("x --uid=a&b"), None);
        assert_eq!(uid("MsiExec.exe /X{1234}"), None);
    }

    #[test]
    fn games_skip_the_client_and_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let sc2 = tmp.path().join("StarCraft II");
        fs::create_dir_all(&sc2).unwrap();
        let ow = tmp.path().join("Overwatch");
        fs::create_dir_all(&ow).unwrap();
        fs::write(ow.join("Overwatch Launcher.exe"), "").unwrap();
        let bnet = tmp.path().join("Battle.net");
        fs::create_dir_all(&bnet).unwrap();
        let games = from_uninstall([
            entry("Battle.net", &bnet, "battle.net"),
            entry("StarCraft II", &sc2, "s2"),
            entry("Overwatch", &ow, "prometheus"),
            // The same game listed twice (both registry views).
            entry("StarCraft II", &sc2, "s2"),
            // Uninstalled, but the entry was left behind.
            entry("Diablo III", &tmp.path().join("missing"), "d3"),
            UninstallEntry {
                uninstall_string: "MsiExec.exe /X{1234}".into(),
                ..entry("Other", &sc2, "x")
            },
        ]);
        let ids: Vec<_> = games.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["battlenet:s2", "battlenet:prometheus"]);
        assert_eq!(games[0].title, "StarCraft II");
        assert_eq!(games[0].source, Source::BattleNet);
        assert_eq!(games[0].source_id.as_deref(), Some("s2"));
        assert_eq!(games[0].launch_target, None);
        assert!(games[1]
            .launch_target
            .as_deref()
            .unwrap()
            .ends_with("Overwatch Launcher.exe"));
    }

    #[test]
    fn launch_codes() {
        assert_eq!(launch_code("s2"), Some("S2"));
        assert_eq!(launch_code("prometheus"), Some("Pro"));
        assert_eq!(launch_code("something_new"), None);
        // Codes go on Battle.net's command line: letters and digits only.
        for uid in ["wow", "wow_classic", "d3", "fenris", "hs_beta", "auks"] {
            assert!(launch_code(uid)
                .unwrap()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric()));
        }
    }
}
