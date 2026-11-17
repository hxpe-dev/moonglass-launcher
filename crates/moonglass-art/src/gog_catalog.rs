//! GOG's public catalog (`api.gog.com/v2/games/<product ID>`), for GOG
//! games: no key needed. Its `_links` name a box-art cover
//! (`boxArtImage`), a wide background (`galaxyBackgroundImage`, else
//! `backgroundImage`) and a transparent logo (`logo`), all on GOG's image
//! host.

use std::path::Path;

use serde::Deserialize;

use crate::{net, ArtError, FoundArt};

const API: &str = "https://api.gog.com/v2/games";

/// Image URLs for one game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Images {
    pub cover: Option<String>,
    pub hero: Option<String>,
    pub logo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Product {
    #[serde(default, rename = "_links")]
    links: Links,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Links {
    box_art_image: Option<Link>,
    galaxy_background_image: Option<Link>,
    background_image: Option<Link>,
    logo: Option<Link>,
}

#[derive(Debug, Deserialize)]
struct Link {
    href: String,
}

/// Only secure links to GOG's own hosts are followed.
fn trusted(link: Option<Link>) -> Option<String> {
    let href = link?.href;
    let host = href.strip_prefix("https://")?.split('/').next()?;
    (host == "gog-statics.com" || host.ends_with(".gog-statics.com") || host.ends_with(".gog.com"))
        .then_some(href)
}

/// Reads the catalog's answer.
pub fn parse(json: &str) -> Result<Images, String> {
    let p: Product = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let l = p.links;
    Ok(Images {
        cover: trusted(l.box_art_image),
        hero: trusted(l.galaxy_background_image).or_else(|| trusted(l.background_image)),
        logo: trusted(l.logo),
    })
}

/// Looks the game up. `Ok(None)` when GOG does not know the ID.
pub async fn lookup(http: &reqwest::Client, product_id: &str) -> Result<Option<Images>, ArtError> {
    if product_id.is_empty() || !product_id.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let resp = http.get(format!("{API}/{product_id}")).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let text = resp.error_for_status()?.text().await?;
    parse(&text).map(Some).map_err(ArtError::Gog)
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
        out.cover = Some(net::download(http, url, dir, "gog_cover").await?);
    }
    if let (None, Some(url)) = (&want.hero, &images.hero) {
        out.hero = Some(net::download(http, url, dir, "gog_hero").await?);
    }
    if let (None, Some(url)) = (&want.logo, &images.logo) {
        out.logo = Some(net::download(http, url, dir, "gog_logo").await?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_links() {
        let images = parse(
            r#"{"_links": {
                "icon": {"href": "https://images.gog-statics.com/icon.png"},
                "logo": {"href": "https://images.gog-statics.com/logo.png"},
                "boxArtImage": {"href": "https://images.gog-statics.com/box.jpg"},
                "backgroundImage": {"href": "https://images.gog-statics.com/bg.jpg"},
                "galaxyBackgroundImage": {"href": "https://images.gog-statics.com/galaxy.jpg"}
            }}"#,
        )
        .unwrap();
        assert_eq!(
            images,
            Images {
                cover: Some("https://images.gog-statics.com/box.jpg".into()),
                hero: Some("https://images.gog-statics.com/galaxy.jpg".into()),
                logo: Some("https://images.gog-statics.com/logo.png".into()),
            }
        );
    }

    #[test]
    fn ignores_foreign_and_insecure_links() {
        let images = parse(
            r#"{"_links": {
                "logo": {"href": "http://images.gog-statics.com/logo.png"},
                "boxArtImage": {"href": "https://evil.example/box.jpg"},
                "backgroundImage": {"href": "https://images.gog-statics.com/bg.jpg"}
            }}"#,
        )
        .unwrap();
        assert_eq!(
            images,
            Images {
                cover: None,
                hero: Some("https://images.gog-statics.com/bg.jpg".into()),
                logo: None,
            }
        );
        assert_eq!(parse("{}").unwrap(), Images::default());
        assert!(parse("[").is_err());
    }
}
