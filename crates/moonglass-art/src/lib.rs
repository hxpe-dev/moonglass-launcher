//! Artwork pipeline (spec 5.2).
//!
//! Sources, first hit wins per asset type (cover, hero, logo):
//! 1. user override (handled by the caller: locked artwork is never replaced),
//! 2. Steam's local library cache ([`steam_cache`]),
//! 3. the Epic Games Launcher's local catalog, for Epic games ([`epic_catalog`]),
//! 4. SteamGridDB, with the user's API key ([`steamgriddb`]),
//! 5. for emulated games, keyless game databases: Switch eShop images by
//!    title ID, libretro box art for older systems ([`rom_art`]),
//! 6. a generated placeholder ([`placeholder`]).
//!
//! [`ingest`] then prepares what the carousel needs once per asset: a
//! resized cover, one image for each card shape (and a zoomed-in crop of
//! the cover), dominant and accent
//! colors, three pre-blurred hero textures and a motion-blurred one (GPUI
//! has no blur filter, spec 4.1).

use std::path::{Path, PathBuf};

use thiserror::Error;

pub mod color;
pub mod epic_catalog;
pub mod ingest;
mod net;
pub mod placeholder;
pub mod rom_art;
pub mod steam_cache;
pub mod steamgriddb;

#[derive(Debug, Error)]
pub enum ArtError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Image {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("artwork download: {0}")]
    Http(#[from] reqwest::Error),
    #[error("artwork download: {0} is too large")]
    TooLarge(String),
    #[error("SteamGridDB: {0}")]
    Api(String),
    #[error("Epic catalog {path}: {message}")]
    Catalog { path: PathBuf, message: String },
}

/// The HTTP client used for artwork downloads.
pub fn http_client() -> Result<reqwest::Client, ArtError> {
    net::client()
}

impl ArtError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn image(path: &Path, source: image::ImageError) -> Self {
        Self::Image {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Image files found for one game, before ingestion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundArt {
    pub cover: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    pub logo: Option<PathBuf>,
}

impl FoundArt {
    pub fn is_complete(&self) -> bool {
        self.cover.is_some() && self.hero.is_some() && self.logo.is_some()
    }

    /// Fills missing slots from `other` (a lower-priority source).
    pub fn fill_from(&mut self, other: FoundArt) {
        self.cover = self.cover.take().or(other.cover);
        self.hero = self.hero.take().or(other.hero);
        self.logo = self.logo.take().or(other.logo);
    }
}

/// Folder name for a game's artwork, safe on every file system
/// (`steam:620` → `steam_620`).
pub fn cache_folder_name(game_id: &str) -> String {
    game_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Pixel size of an image file, read from its header only (cheap enough to
/// call while laying out the hero zone).
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}
