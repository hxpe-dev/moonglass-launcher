//! Artwork orchestration (spec 5.2): for each game without artwork, try
//! Steam's local cache, then the Epic launcher's or GOG's catalog, then SteamGridDB
//! (with a key), then, for emulated games, the keyless game databases
//! ([`moonglass_art::rom_art`]), then a placeholder; ingest the result, and
//! store it. Locked (user-chosen) artwork is never replaced.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use moonglass_art::epic_catalog::{self, Catalog};
use moonglass_art::gog_catalog;
use moonglass_art::ingest::{
    card_fill_image_path, card_image_path, ensure_fill_cards, ensure_motion_hero, ingest,
    is_placeholder_cover, CardCrop, IngestConfig, Ingested,
};
use moonglass_art::rom_art;
use moonglass_art::steamgriddb::{Client, Lookup};
use moonglass_art::{cache_folder_name, steam_cache, FoundArt};
use moonglass_core::{Game, GameId, Source};
use moonglass_db::Db;

pub const SETTING_SGDB_KEY: &str = "steamgriddb_api_key";

/// Games left with a placeholder that were already looked up again in
/// this run.
static RETRIED: std::sync::Mutex<std::collections::BTreeSet<GameId>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

#[derive(Debug, Default)]
pub struct ArtSummary {
    pub from_steam: usize,
    pub from_epic: usize,
    pub from_gog: usize,
    pub from_sgdb: usize,
    pub from_rom_db: usize,
    pub placeholders: usize,
    pub failed: usize,
}

impl std::fmt::Display for ArtSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Artwork: {} from Steam, {} from Epic, {} from SteamGridDB",
            self.from_steam, self.from_epic, self.from_sgdb
        )?;
        if self.from_gog > 0 {
            write!(f, ", {} from GOG", self.from_gog)?;
        }
        if self.from_rom_db > 0 {
            write!(f, ", {} from game databases", self.from_rom_db)?;
        }
        write!(f, ", {} placeholders", self.placeholders)?;
        if self.failed > 0 {
            write!(f, ", {} failed", self.failed)?;
        }
        Ok(())
    }
}

/// The Epic Games Launcher's catalog cache, next to its manifests folder.
/// `None` where Epic games come from Heroic, or the file cannot be read.
fn epic_catalog() -> Option<Catalog> {
    let moonglass_platform::EpicSource::EpicManifests(manifests) =
        moonglass_platform::epic_source()?
    else {
        return None;
    };
    let path = manifests.parent()?.join("Catalog").join("catcache.bin");
    match Catalog::load(&path) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(error = %e, "Epic artwork unavailable");
            None
        }
    }
}

/// A game whose stored cover is the generated placeholder.
fn has_placeholder(g: &Game) -> bool {
    g.cover_path
        .as_deref()
        .is_some_and(|p| is_placeholder_cover(Path::new(p)) && Path::new(p).is_file())
}

fn needs_art(g: &Game) -> bool {
    g.is_installed
        && !g.artwork_locked
        // Art made before the per-shape card images existed is redone once.
        && !g.cover_path.as_deref().is_some_and(|p| {
            let p = Path::new(p);
            p.is_file() && card_image_path(p, CardCrop::Poster).is_file()
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
    let unlocked = |g: &Game| g.is_installed && !g.artwork_locked;
    let epic = if games
        .iter()
        .any(|g| g.source == Source::Epic && unlocked(g))
    {
        epic_catalog()
    } else {
        None
    };
    let http = moonglass_art::http_client()?;
    // Which emulator runs each ROM folder, to tell the system of a game.
    let emulator_names: HashMap<String, String> = db
        .emulators()
        .await?
        .into_iter()
        .map(|e| {
            let exe = Path::new(&e.executable_path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (e.id, format!("{} {exe}", e.name))
        })
        .collect();
    let rom_emulator: HashMap<String, String> = db
        .rom_folders()
        .await?
        .into_iter()
        .filter_map(|d| Some((d.id, emulator_names.get(&d.emulator_id)?.clone())))
        .collect();
    let switch_ids = if games.iter().any(|g| g.source == Source::Rom && unlocked(g)) {
        tokio::task::spawn_blocking(rom_art::switch_title_ids_from_emulators).await?
    } else {
        Vec::new()
    };
    // An Epic or emulated game left with a placeholder is tried again: the
    // catalog may have gained it, or the last download may have failed.
    // Each is retried once per run (refreshes also follow every library
    // change and game session; a game no source knows would be looked up
    // again each time).
    let retry = |g: &Game| {
        has_placeholder(g)
            && ((g.source == Source::Epic && epic.is_some())
                || matches!(g.source, Source::Gog | Source::Rom))
            && RETRIED.lock().unwrap().insert(g.id.clone())
    };
    for g in games
        .iter()
        .filter(|g| unlocked(g) && (force || needs_art(g) || retry(g)))
    {
        let dir = cache_dir.join("art").join(cache_folder_name(g.id.as_str()));
        let mut found = FoundArt::default();
        let mut steam_hit = false;
        if let (Source::Steam, Some(root), Some(app)) = (g.source, &steam_root, &g.source_id) {
            found = steam_cache::find(root, app);
            steam_hit = found.cover.is_some();
        }
        let mut epic_hit = false;
        if let (Source::Epic, Some(catalog), Some(app)) = (g.source, &epic, &g.source_id) {
            if let Some(images) = catalog.images(app) {
                match epic_catalog::fetch(&http, &images, &found, &dir).await {
                    Ok(more) => {
                        epic_hit = more.cover.is_some();
                        found.fill_from(more);
                    }
                    Err(e) => {
                        tracing::warn!(game = %g.id, error = %e, "Epic artwork download failed")
                    }
                }
            }
        }
        let mut gog_hit = false;
        if let (Source::Gog, Some(id)) = (g.source, &g.source_id) {
            let more = match gog_catalog::lookup(&http, id).await {
                Ok(Some(images)) => gog_catalog::fetch(&http, &images, &found, &dir).await,
                Ok(None) => Ok(FoundArt::default()),
                Err(e) => Err(e),
            };
            match more {
                Ok(more) => {
                    gog_hit = more.cover.is_some();
                    found.fill_from(more);
                }
                Err(e) => tracing::warn!(game = %g.id, error = %e, "GOG artwork download failed"),
            }
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

        let mut rom_hit = false;
        if g.source == Source::Rom && !found.is_complete() {
            let emulator = g
                .rom_directory_id
                .as_ref()
                .and_then(|d| rom_emulator.get(d))
                .map_or("", String::as_str);
            let more = rom_artwork(&http, g, emulator, &switch_ids, &found, &dir).await;
            rom_hit = found.cover.is_none() && more.cover.is_some();
            found.fill_from(more);
        }

        // Nothing better than the placeholder it already has.
        if found.cover.is_none() && has_placeholder(g) && !force {
            continue;
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
                } else if epic_hit {
                    summary.from_epic += 1;
                } else if gog_hit {
                    summary.from_gog += 1;
                } else if sgdb_hit {
                    summary.from_sgdb += 1;
                } else if rom_hit {
                    summary.from_rom_db += 1;
                }
            }
            Err(e) => {
                summary.failed += 1;
                tracing::warn!(game = %g.id, error = %e, "artwork ingestion failed");
            }
        }
    }

    // Images added to the pipeline later (the motion-blurred background,
    // the zoomed-in cards), for artwork made before they existed.
    let games = db.games().await?;
    let heroes: Vec<PathBuf> = games
        .iter()
        .filter_map(|g| g.hero_path.as_ref().map(PathBuf::from))
        .filter(|h| h.is_file() && !moonglass_art::ingest::motion_hero_path(h).is_file())
        .collect();
    let covers: Vec<PathBuf> = games
        .iter()
        .filter_map(|g| g.cover_path.as_ref().map(PathBuf::from))
        .filter(|c| c.is_file() && !card_fill_image_path(c, CardCrop::Poster).is_file())
        .collect();
    if !heroes.is_empty() || !covers.is_empty() {
        tokio::task::spawn_blocking(move || {
            for h in heroes {
                if let Err(e) = ensure_motion_hero(&h) {
                    tracing::warn!(error = %e, "motion background not made");
                }
            }
            for c in covers {
                if let Err(e) = ensure_fill_cards(&c) {
                    tracing::warn!(error = %e, "zoomed card images not made");
                }
            }
        })
        .await?;
    }
    Ok(summary)
}

/// Keyless artwork for an emulated game: the Switch eShop images by title
/// ID, or libretro box art. Failures are logged and leave slots empty.
async fn rom_artwork(
    http: &reqwest::Client,
    g: &Game,
    emulator: &str,
    switch_ids: &[(String, String)],
    want: &FoundArt,
    dir: &Path,
) -> FoundArt {
    let Some(file) = g.launch_target.as_deref().map(Path::new) else {
        return FoundArt::default();
    };
    // A folder game (RPCS3) is named by its folder.
    let stem = if file.is_dir() {
        file.file_name()
    } else {
        file.file_stem()
    }
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_default();
    let result = if rom_art::is_switch(file, emulator) {
        let key = rom_art::match_key(&g.title);
        let id = rom_art::switch_title_id(&stem).or_else(|| {
            switch_ids
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, id)| id.clone())
        });
        match id {
            Some(id) => rom_art::fetch_switch(http, &id, want, dir).await,
            None => return FoundArt::default(),
        }
    } else if want.cover.is_none() {
        let systems = rom_art::libretro_systems(file, emulator);
        rom_art::fetch_libretro(http, systems, &stem, &g.title, dir).await
    } else {
        return FoundArt::default();
    };
    result.unwrap_or_else(|e| {
        tracing::warn!(game = %g.id, error = %e, "emulated game artwork lookup failed");
        FoundArt::default()
    })
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
