//! The Epic Games Launcher's own catalog cache (Windows):
//! `C:\ProgramData\Epic\EpicGamesLauncher\Data\Catalog\catcache.bin`, a
//! base64-encoded JSON list of everything in the player's library. Each
//! entry names its artwork on Epic's image host, so Epic games get real
//! posters without an API key.

use std::path::Path;

use base64::Engine;
use serde::Deserialize;

use crate::{net, ArtError, FoundArt};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    #[serde(default)]
    key_images: Vec<KeyImage>,
    #[serde(default)]
    release_info: Vec<Release>,
}

#[derive(Deserialize)]
struct KeyImage {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    #[serde(default)]
    app_id: Option<String>,
}

/// Image kinds in order of preference.
const COVER_KINDS: &[&str] = &["DieselGameBoxTall", "OfferImageTall"];
const HERO_KINDS: &[&str] = &["DieselGameBox", "OfferImageWide", "Featured"];
const LOGO_KINDS: &[&str] = &["DieselGameBoxLogo"];

/// Where one game's artwork can be downloaded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Images {
    pub cover: Option<String>,
    pub hero: Option<String>,
    pub logo: Option<String>,
}

pub struct Catalog {
    items: Vec<Item>,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, ArtError> {
        let raw = std::fs::read(path).map_err(|e| ArtError::io(path, e))?;
        Self::parse(&raw).map_err(|message| ArtError::Catalog {
            path: path.to_path_buf(),
            message,
        })
    }

    fn parse(raw: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(raw).map_err(|e| e.to_string())?;
        let json = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|e| e.to_string())?;
        let items = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
        Ok(Self { items })
    }

    /// Artwork for the game whose manifest `AppName` is `app_name`.
    pub fn images(&self, app_name: &str) -> Option<Images> {
        let item = self.items.iter().find(|i| {
            i.release_info
                .iter()
                .any(|r| r.app_id.as_deref() == Some(app_name))
        })?;
        let pick = |kinds: &[&str]| {
            kinds.iter().find_map(|k| {
                item.key_images
                    .iter()
                    // Only secure links are followed.
                    .find(|i| i.kind == *k && i.url.starts_with("https://"))
                    .map(|i| i.url.clone())
            })
        };
        Some(Images {
            cover: pick(COVER_KINDS),
            hero: pick(HERO_KINDS),
            logo: pick(LOGO_KINDS),
        })
    }
}

/// Downloads whichever of `want`'s missing assets `images` has into `dir`.
pub async fn fetch(
    http: &reqwest::Client,
    images: &Images,
    want: &FoundArt,
    dir: &Path,
) -> Result<FoundArt, ArtError> {
    let mut out = FoundArt::default();
    if let (None, Some(url)) = (&want.cover, &images.cover) {
        out.cover = Some(net::download(http, url, dir, "epic_cover").await?);
    }
    if let (None, Some(url)) = (&want.hero, &images.hero) {
        out.hero = Some(net::download(http, url, dir, "epic_hero").await?);
    }
    if let (None, Some(url)) = (&want.logo, &images.logo) {
        out.logo = Some(net::download(http, url, dir, "epic_logo").await?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(json: &str) -> Catalog {
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        Catalog::parse(format!("{encoded}\n").as_bytes()).unwrap()
    }

    #[test]
    fn finds_images_by_app_name() {
        let c = catalog(
            r#"[
              {"title": "Other", "releaseInfo": [{"appId": "Heron"}], "keyImages": []},
              {"title": "Game", "releaseInfo": [{"appId": "Potoo", "platform": ["Windows"]}],
               "keyImages": [
                 {"type": "DieselGameBox", "url": "https://img.example/wide.jpg", "width": 1920},
                 {"type": "OfferImageTall", "url": "https://img.example/offer.jpg"},
                 {"type": "DieselGameBoxTall", "url": "https://img.example/tall.png"},
                 {"type": "DieselGameBoxLogo", "url": "http://img.example/insecure.png"}
               ]}
            ]"#,
        );
        assert_eq!(
            c.images("Potoo"),
            Some(Images {
                cover: Some("https://img.example/tall.png".into()),
                hero: Some("https://img.example/wide.jpg".into()),
                logo: None,
            })
        );
        assert_eq!(c.images("Heron"), Some(Images::default()));
        assert_eq!(c.images("Missing"), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Catalog::parse(b"not base64 !!").is_err());
        let not_json = base64::engine::general_purpose::STANDARD.encode("{");
        assert!(Catalog::parse(not_json.as_bytes()).is_err());
    }
}
