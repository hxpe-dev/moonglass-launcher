//! SteamGridDB (API v2). Queried by Steam AppID for Steam games and by title
//! search for everything else. Requires the user's own API key; the key is
//! never logged.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{ArtError, FoundArt};

const API: &str = "https://www.steamgriddb.com/api/v2";

/// How a game is looked up.
#[derive(Debug, Clone, Copy)]
pub enum Lookup<'a> {
    SteamAppId(&'a str),
    Title(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cover,
    Hero,
    Logo,
}

impl Kind {
    fn endpoint(self) -> &'static str {
        match self {
            Kind::Cover => "grids",
            Kind::Hero => "heroes",
            Kind::Logo => "logos",
        }
    }

    /// Filters for the request: covers are 2:3 posters.
    fn query(self) -> &'static str {
        match self {
            Kind::Cover => "?dimensions=600x900,342x482,660x930&types=static",
            Kind::Hero | Kind::Logo => "?types=static",
        }
    }

    fn file_stem(self) -> &'static str {
        match self {
            Kind::Cover => "sgdb_cover",
            Kind::Hero => "sgdb_hero",
            Kind::Logo => "sgdb_logo",
        }
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    // Missing `data` deserializes to None without needing `T: Default`.
    #[serde(bound(deserialize = "T: Deserialize<'de>"))]
    data: Option<T>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Deserialize)]
struct Asset {
    url: String,
}

#[derive(Deserialize)]
struct SearchHit {
    id: u64,
}

pub struct Client {
    http: reqwest::Client,
    key: String,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key.
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(api_key: &str) -> Result<Self, ArtError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Moonglass/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            key: api_key.trim().to_string(),
        })
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<Option<T>, ArtError> {
        let resp = self.http.get(url).bearer_auth(&self.key).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ArtError::Api("the API key was rejected".into()));
        }
        let env: Envelope<T> = resp.error_for_status()?.json().await?;
        if !env.success {
            return Err(ArtError::Api(env.errors.join("; ")));
        }
        Ok(env.data)
    }

    /// SteamGridDB's own game ID for a lookup.
    async fn game_path(&self, lookup: Lookup<'_>) -> Result<Option<String>, ArtError> {
        Ok(match lookup {
            Lookup::SteamAppId(id) => Some(format!("steam/{id}")),
            Lookup::Title(title) => {
                let term = encode_path_segment(title);
                let hits: Option<Vec<SearchHit>> = self
                    .get(&format!("{API}/search/autocomplete/{term}"))
                    .await?;
                hits.and_then(|h| h.into_iter().next())
                    .map(|h| format!("game/{}", h.id))
            }
        })
    }

    /// Downloads whichever of `want`'s missing assets SteamGridDB has into
    /// `dir`, returning what was saved.
    pub async fn fetch(
        &self,
        lookup: Lookup<'_>,
        want: &FoundArt,
        dir: &Path,
    ) -> Result<FoundArt, ArtError> {
        let mut out = FoundArt::default();
        let Some(game) = self.game_path(lookup).await? else {
            return Ok(out);
        };
        std::fs::create_dir_all(dir).map_err(|e| ArtError::io(dir, e))?;
        for (kind, missing) in [
            (Kind::Cover, want.cover.is_none()),
            (Kind::Hero, want.hero.is_none()),
            (Kind::Logo, want.logo.is_none()),
        ] {
            if !missing {
                continue;
            }
            let url = format!("{API}/{}/{game}{}", kind.endpoint(), kind.query());
            let assets: Option<Vec<Asset>> = self.get(&url).await?;
            let Some(asset) = assets.and_then(|a| a.into_iter().next()) else {
                continue;
            };
            let saved = self.download(&asset.url, dir, kind).await?;
            match kind {
                Kind::Cover => out.cover = Some(saved),
                Kind::Hero => out.hero = Some(saved),
                Kind::Logo => out.logo = Some(saved),
            }
        }
        Ok(out)
    }

    async fn download(&self, url: &str, dir: &Path, kind: Kind) -> Result<PathBuf, ArtError> {
        let bytes = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let ext = url
            .rsplit('.')
            .next()
            .filter(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "webp"
                )
            })
            .unwrap_or("png");
        let path = dir.join(format!("{}.{ext}", kind.file_stem()));
        std::fs::write(&path, &bytes).map_err(|e| ArtError::io(&path, e))?;
        Ok(path)
    }
}

/// Percent-encodes a title for use as one URL path segment.
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_titles() {
        assert_eq!(
            encode_path_segment("Half-Life 2: Ep/1"),
            "Half-Life%202%3A%20Ep%2F1"
        );
        assert_eq!(encode_path_segment("Pokémon"), "Pok%C3%A9mon");
    }

    #[test]
    fn debug_hides_key() {
        let c = Client::new("secret-key").unwrap();
        assert!(!format!("{c:?}").contains("secret"));
    }
}
