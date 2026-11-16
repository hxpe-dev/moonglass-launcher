//! Artwork orchestration (spec 5.2): for each game without artwork, try
//! Steam's local cache, then SteamGridDB (with a key), then a placeholder,
//! ingest the result, and store it. Locked (user-chosen) artwork is never
//! replaced.

use std::path::{Path, PathBuf};

use moonglass_art::ingest::{frosted_cover_path, ingest, IngestConfig, Ingested};
use moonglass_art::steamgriddb::{Client, Lookup};
use moonglass_art::{cache_folder_name, steam_cache, FoundArt};
use moonglass_core::{Game, GameId, Source};
use moonglass_db::Db;

pub const SETTING_SGDB_KEY: &str = "steamgriddb_api_key";

#[derive(Debug, Default)]
pub struct ArtSummary {
    pub from_steam: usize,
    pub from_sgdb: usize,
    pub placeholders: usize,
    pub failed: usize,
}

fn needs_art(g: &Game) -> bool {
    g.is_installed
        && !g.artwork_locked
        // Art made before the frosted cover existed is redone once.
        && !g.cover_path.as_deref().is_some_and(|p| {
            let p = Path::new(p);
            p.is_file() && frosted_cover_path(p).is_file()
        })
}

/// Stores ingested artwork; `user_override` also locks it.
async fn store(db: &Db, id: &GameId, art: &Ingested, user_override: bool) -> anyhow::Result<()> {
    db.set_artwork(
        id,
        &art.cover_path.to_string_lossy(),
        &art.hero_path.to_string_lossy(),
        art.logo_path
            .as_ref()
            .map(|p| p.to_string_lossy())
            .as_deref(),
        &art.dominant,
        &art.accent,
        user_override,
    )
    .await?;
    Ok(())
}

/// Fills in artwork for every game that has none. `force` redoes all
/// unlocked games (for example after adding a SteamGridDB key).
pub async fn refresh(db: &Db, cache_dir: &Path, force: bool) -> anyhow::Result<ArtSummary> {
    let _one_at_a_time = crate::library::LIBRARY_WORK.lock().await;
    let steam_root = crate::library::steam_root(db).await?;
    let sgdb = match db.setting::<String>(SETTING_SGDB_KEY).await? {
        Some(k) if !k.trim().is_empty() => Some(Client::new(&k)?),
        _ => None,
    };

    let mut summary = ArtSummary::default();
    let games = db.games().await?;
    for g in games
        .iter()
        .filter(|g| force && g.is_installed && !g.artwork_locked || needs_art(g))
    {
        let dir = cache_dir.join("art").join(cache_folder_name(g.id.as_str()));
        let mut found = FoundArt::default();
        let mut steam_hit = false;
        if let (Source::Steam, Some(root), Some(app)) = (g.source, &steam_root, &g.source_id) {
            found = steam_cache::find(root, app);
            steam_hit = found.cover.is_some();
        }
        let mut sgdb_hit = false;
        if let Some(client) = &sgdb {
            if !found.is_complete() {
                let lookup = match (g.source, &g.source_id) {
                    (Source::Steam, Some(app)) => Lookup::SteamAppId(app),
                    _ => Lookup::Title(&g.title),
                };
                match client.fetch(lookup, &found, &dir).await {
                    Ok(more) => {
                        sgdb_hit = found.cover.is_none() && more.cover.is_some();
                        found.fill_from(more);
                    }
                    Err(e) => tracing::warn!(game = %g.id, error = %e, "SteamGridDB lookup failed"),
                }
            }
        }

        let title = g.title.clone();
        let out = dir.clone();
        let result = tokio::task::spawn_blocking(move || {
            ingest(&found, &title, &out, IngestConfig::default())
        })
        .await?;
        match result {
            Ok(art) => {
                store(db, &g.id, &art, false).await?;
                if art.is_placeholder {
                    summary.placeholders += 1;
                } else if steam_hit {
                    summary.from_steam += 1;
                } else if sgdb_hit {
                    summary.from_sgdb += 1;
                }
            }
            Err(e) => {
                summary.failed += 1;
                tracing::warn!(game = %g.id, error = %e, "artwork ingestion failed");
            }
        }
    }
    Ok(summary)
}

/// `[Y] → Change Artwork`: the user picks an image; it is ingested and
/// locked so no rescan replaces it.
pub async fn set_override(
    db: &Db,
    cache_dir: &Path,
    game: &Game,
    cover: &Path,
) -> anyhow::Result<()> {
    let _one_at_a_time = crate::library::LIBRARY_WORK.lock().await;
    let dir = cache_dir
        .join("art")
        .join(cache_folder_name(game.id.as_str()))
        .join("user");
    let found = FoundArt {
        cover: Some(cover.to_path_buf()),
        hero: game
            .hero_path
            .clone()
            .map(PathBuf::from)
            .filter(|p| p.is_file()),
        logo: game
            .logo_path
            .clone()
            .map(PathBuf::from)
            .filter(|p| p.is_file()),
    };
    let title = game.title.clone();
    let art =
        tokio::task::spawn_blocking(move || ingest(&found, &title, &dir, IngestConfig::default()))
            .await??;
    store(db, &game.id, &art, true).await
}
