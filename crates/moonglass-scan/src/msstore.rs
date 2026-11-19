//! Microsoft Store and Xbox app (PC Game Pass) games (Windows).
//!
//! Every Store app is a package; the platform layer lists the packages
//! installed for the current user with their folder and display name.
//! Games are the packages built with Microsoft's game kit (GDK), which ship
//! a `MicrosoftGame.config` next to their `AppxManifest.xml`. Other apps
//! (the Xbox app itself, Gaming Services, ordinary Store apps) have none.
//!
//! A game is started by its Application User Model ID,
//! `<package family name>!<application ID>`: the family name comes from
//! the package's full name, the application ID from its manifest.

use std::path::Path;

use moonglass_core::{GameId, Source};

use crate::ScannedGame;

/// One installed package, as the platform layer read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Package {
    /// `Name_Version_Architecture_ResourceId_PublisherId`.
    pub full_name: String,
    /// The package's display name, already resolved when it was a resource
    /// reference; may be empty.
    pub display_name: String,
    /// The package's folder.
    pub root: String,
}

/// `Name_PublisherId` from a package full name. Package names cannot hold
/// `_`, so the first and last parts are the ones wanted.
pub fn family_name(full_name: &str) -> Option<String> {
    let mut parts = full_name.split('_');
    let name = parts.next().filter(|s| !s.is_empty())?;
    let publisher = parts.next_back().filter(|s| !s.is_empty())?;
    Some(format!("{name}_{publisher}"))
}

/// Reads a manifest-like text file: UTF-8 (with or without a BOM) or
/// UTF-16 with a BOM.
fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if le {
                    u16::from_le_bytes(c)
                } else {
                    u16::from_be_bytes(c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    Some(match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        b => String::from_utf8_lossy(b).into_owned(),
    })
}

/// The opening tags named `tag` (`<tag …>`), comments left out.
fn tags<'a>(xml: &'a str, tag: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let open = format!("<{tag}");
    let mut rest = xml;
    std::iter::from_fn(move || loop {
        let comment = rest.find("<!--");
        let at = rest.find(&open)?;
        if let Some(c) = comment.filter(|&c| c < at) {
            rest = &rest[c..];
            let end = rest.find("-->").map_or(rest.len(), |e| e + 3);
            rest = &rest[end..];
            continue;
        }
        let after = &rest[at + open.len()..];
        // `<Application` must not match `<Applications>`.
        if !after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            rest = after;
            continue;
        }
        let end = after.find('>').unwrap_or(after.len());
        rest = &after[end..];
        return Some(&after[..end]);
    })
}

/// An attribute's value in a tag's text, with the usual XML entities
/// decoded.
fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let mut from = 0;
    while let Some(i) = tag[from..].find(&needle) {
        let at = from + i;
        let starts_word = tag[..at].ends_with(|c: char| c.is_whitespace());
        let value_start = at + needle.len();
        if starts_word {
            let end = tag[value_start..].find('"')? + value_start;
            return Some(
                tag[value_start..end]
                    .replace("&quot;", "\"")
                    .replace("&apos;", "'")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&"),
            );
        }
        from = value_start;
    }
    None
}

/// A name the player can read: not empty, not an unresolved resource.
fn readable(name: &str) -> Option<String> {
    let n = name.trim();
    (!n.is_empty() && !n.starts_with("@{") && !n.starts_with("ms-resource:")).then(|| n.to_string())
}

/// The package's game, if it is one.
fn game(p: Package) -> Option<ScannedGame> {
    let root = Path::new(&p.root);
    let config = read_text(&root.join("MicrosoftGame.config"))?;
    let manifest = read_text(&root.join("AppxManifest.xml"))?;
    let family = family_name(&p.full_name)?;
    let app_id = tags(&manifest, "Application").find_map(|t| attr(t, "Id"))?;
    let title = readable(&p.display_name)
        .or_else(|| {
            tags(&config, "ShellVisuals")
                .find_map(|t| attr(t, "DefaultDisplayName"))
                .and_then(|n| readable(&n))
        })
        .or_else(|| {
            tags(&config, "Executable")
                .find_map(|t| attr(t, "OverrideDisplayName"))
                .and_then(|n| readable(&n))
        })
        .unwrap_or_else(|| family.split('_').next().unwrap_or_default().to_string());
    Some(ScannedGame {
        id: GameId::msstore(&family),
        title,
        source: Source::MsStore,
        source_id: Some(format!("{family}!{app_id}")),
        launch_target: None,
        install_dir: Some(p.root),
        rom_directory_id: None,
        platform: None,
        last_played_hint: None,
    })
}

/// Games among the installed packages. A package listed under two versions
/// (during an update) gives one game.
pub fn from_packages(packages: impl IntoIterator<Item = Package>) -> Vec<ScannedGame> {
    let mut seen = std::collections::HashSet::new();
    packages
        .into_iter()
        .filter_map(game)
        .filter(|g| seen.insert(g.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const FULL: &str = "Microsoft.ForteBaseGame_3.483.970.0_x64__8wekyb3d8bbwe";

    fn package(dir: &Path, display: &str, config: Option<&str>) -> Package {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("AppxManifest.xml"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<Package>
  <!-- <Application Id="Commented"> -->
  <Applications>
    <Application Id="Forzahorizon6" Executable="GameLaunchHelper.exe" EntryPoint="Windows.FullTrustApplication">
    </Application>
  </Applications>
</Package>"#,
        )
        .unwrap();
        if let Some(c) = config {
            fs::write(dir.join("MicrosoftGame.config"), c).unwrap();
        }
        Package {
            full_name: FULL.into(),
            display_name: display.into(),
            root: dir.to_string_lossy().into_owned(),
        }
    }

    const CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Game configVersion="1">
  <ExecutableList>
    <!-- <Executable Name="Commented.exe" OverrideDisplayName="Wrong" /> -->
    <Executable Name="forzahorizon6.exe" Id="Forzahorizon6" OverrideDisplayName="Forza Horizon 6 (exe)" TargetDeviceFamily="PC" />
  </ExecutableList>
  <ShellVisuals DefaultDisplayName="Forza Horizon 6 &amp; More" PublisherDisplayName="Microsoft Studios" />
</Game>"#;

    #[test]
    fn family_names() {
        assert_eq!(
            family_name(FULL).as_deref(),
            Some("Microsoft.ForteBaseGame_8wekyb3d8bbwe")
        );
        assert_eq!(family_name("NoUnderscore"), None);
    }

    #[test]
    fn games_are_packages_with_a_game_config() {
        let tmp = tempfile::tempdir().unwrap();
        let games = from_packages([
            package(&tmp.path().join("fh6"), "Forza Horizon 6", Some(CONFIG)),
            // Not a game: no MicrosoftGame.config.
            Package {
                full_name: "Microsoft.GamingApp_2.0.0.0_x64__8wekyb3d8bbwe".into(),
                ..package(&tmp.path().join("xbox"), "Xbox", None)
            },
            // The same game under a second version.
            package(&tmp.path().join("fh6-old"), "Forza Horizon 6", Some(CONFIG)),
            // Gone.
            Package {
                root: tmp.path().join("missing").to_string_lossy().into_owned(),
                ..Package::default()
            },
        ]);
        assert_eq!(games.len(), 1);
        let g = &games[0];
        assert_eq!(
            g.id.as_str(),
            "msstore:Microsoft.ForteBaseGame_8wekyb3d8bbwe"
        );
        assert_eq!(g.title, "Forza Horizon 6");
        assert_eq!(g.source, Source::MsStore);
        assert_eq!(
            g.source_id.as_deref(),
            Some("Microsoft.ForteBaseGame_8wekyb3d8bbwe!Forzahorizon6")
        );
        assert!(g.install_dir.as_deref().unwrap().ends_with("fh6"));
    }

    #[test]
    fn titles_fall_back_to_the_game_config() {
        let tmp = tempfile::tempdir().unwrap();
        let unresolved = "@{Microsoft.ForteBaseGame?ms-resource://x/Resources/Title}";
        let games = from_packages([package(&tmp.path().join("a"), unresolved, Some(CONFIG))]);
        assert_eq!(games[0].title, "Forza Horizon 6 & More");

        let bare = CONFIG.replace("DefaultDisplayName=\"Forza Horizon 6 &amp; More\"", "");
        let games = from_packages([package(&tmp.path().join("b"), "", Some(&bare))]);
        assert_eq!(games[0].title, "Forza Horizon 6 (exe)");
    }

    #[test]
    fn reads_utf16_files() {
        let tmp = tempfile::tempdir().unwrap();
        let p = package(&tmp.path().join("a"), "", None);
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(CONFIG.encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(tmp.path().join("a/MicrosoftGame.config"), bytes).unwrap();
        assert_eq!(from_packages([p])[0].title, "Forza Horizon 6 & More");
    }
}
