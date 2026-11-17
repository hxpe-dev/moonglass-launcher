//! Scan orchestration (spec 5.1): runs every scanner, reconciles each one
//! independently, and refreshes standalone executables' installed flag.
//! Shared by the carousel and the debug CLI.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use moonglass_core::Source;
use moonglass_db::{Db, ScanScope};
use moonglass_platform::{EpicSource, GogSource};
use moonglass_scan::{epic, gog, heroic, roms, steam, ScanError, ScannedGame};

/// Settings keys.
pub const SETTING_STEAM_ROOT: &str = "steam_root";
pub const SETTING_STEAM_DENY: &str = "steam_extra_deny_app_ids";

/// Scans and artwork refreshes are started from several places (startup,
/// file watchers, Settings, the end of a game). They run one at a time, so
/// two of them never write the same rows or image files at once.
pub static LIBRARY_WORK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The Steam install root: a manual root in Settings wins over discovery.
pub async fn steam_root(db: &Db) -> anyhow::Result<Option<PathBuf>> {
    Ok(db
        .setting::<String>(SETTING_STEAM_ROOT)
        .await?
        // Empty: chosen in Settings to be found automatically.
        .filter(|p| !p.trim().is_empty())
        .map(Into::into)
        .or_else(moonglass_platform::steam_root))
}

#[derive(Debug, Default)]
pub struct ScanSummary {
    pub lines: Vec<String>,
}

impl ScanSummary {
    fn push(&mut self, s: String) {
        tracing::info!("{s}");
        self.lines.push(s);
    }
}

pub async fn scan_all(db: &Db, now: i64) -> anyhow::Result<ScanSummary> {
    let _one_at_a_time = LIBRARY_WORK.lock().await;
    let mut summary = ScanSummary::default();

    let steam_root = steam_root(db).await?;
    let deny: HashSet<u32> = db
        .setting::<Vec<u32>>(SETTING_STEAM_DENY)
        .await?
        .unwrap_or_default()
        .into_iter()
        .collect();
    match &steam_root {
        Some(root) => {
            apply(
                db,
                &mut summary,
                "Steam",
                ScanScope::Steam,
                steam::scan(root, &deny),
                now,
            )
            .await?
        }
        None => summary.push("Steam: not found".into()),
    }
    if let Some(root) = &steam_root {
        import_steam_playtime(db, &mut summary, root).await?;
    }

    // Epic: the client's manifests on Windows, Heroic on Linux.
    match moonglass_platform::epic_source() {
        Some(EpicSource::EpicManifests(dir)) => {
            apply(
                db,
                &mut summary,
                "Epic",
                ScanScope::Epic,
                epic::scan(&dir),
                now,
            )
            .await?
        }
        Some(EpicSource::Heroic { installed_json, .. }) => {
            apply(
                db,
                &mut summary,
                "Epic (Heroic)",
                ScanScope::Epic,
                heroic::scan(&installed_json),
                now,
            )
            .await?
        }
        None => {
            // Not installed: any Epic games we knew of are gone.
            apply(
                db,
                &mut summary,
                "Epic",
                ScanScope::Epic,
                Ok(Vec::new()),
                now,
            )
            .await?;
        }
    }

    // GOG: the registry on Windows (Galaxy or offline installers), Heroic
    // on Linux. Not installed: any GOG games we knew of are gone.
    let (label, result) = match moonglass_platform::gog_source() {
        Some(GogSource::Registry { games, .. }) => (
            "GOG",
            Ok(gog::from_registry(games.into_iter().map(|g| {
                gog::RegistryGame {
                    product_id: g.product_id,
                    name: g.name,
                    path: g.path,
                    exe: g.exe,
                    depends_on: g.depends_on,
                }
            }))),
        ),
        Some(GogSource::Heroic { installed_json, .. }) => {
            ("GOG (Heroic)", gog::scan_heroic(&installed_json))
        }
        None => ("GOG", Ok(Vec::new())),
    };
    apply(db, &mut summary, label, ScanScope::Gog, result, now).await?;

    for dir in db.rom_directories().await? {
        let label = format!("ROMs {} ({})", dir.path.display(), dir.platform);
        let normalize = |p: &Path| moonglass_platform::normalize_path(p);
        let result = roms::scan(&dir, &normalize);
        apply(
            db,
            &mut summary,
            &label,
            ScanScope::RomDirectory(dir.id.clone()),
            result,
            now,
        )
        .await?;
    }

    // Standalone executables are not scanned, but a deleted one hides like
    // any uninstalled game and comes back if the file returns.
    for g in db
        .games()
        .await?
        .into_iter()
        .filter(|g| g.source == Source::Exe)
    {
        let present = g
            .launch_target
            .as_deref()
            .is_some_and(|p| Path::new(p).is_file());
        if present != g.is_installed {
            db.set_installed(&g.id, present).await?;
        }
    }

    Ok(summary)
}

/// Steam's own playtime for the signed-in account (spec 5.1, Playtime).
async fn import_steam_playtime(
    db: &Db,
    summary: &mut ScanSummary,
    root: &Path,
) -> anyhow::Result<()> {
    let root = root.to_path_buf();
    let times = tokio::task::spawn_blocking(move || {
        crate::steam_accounts::signed_in(&root)
            .map(|id| moonglass_scan::steam_accounts::playtimes(&root, &id))
    })
    .await?;
    // No account known: leave the figures as they were.
    if let Some(times) = times {
        let n = db.set_store_playtimes(Source::Steam, &times).await?;
        summary.push(format!("Steam playtime: {n} games"));
    }
    Ok(())
}

async fn apply(
    db: &Db,
    summary: &mut ScanSummary,
    label: &str,
    scope: ScanScope,
    result: Result<Vec<ScannedGame>, ScanError>,
    now: i64,
) -> anyhow::Result<()> {
    match result {
        Ok(games) => {
            let r = db.reconcile(&scope, &games, now).await?;
            summary.push(format!(
                "{label}: {} games ({} new, {} no longer installed)",
                games.len(),
                r.added,
                r.uninstalled
            ));
        }
        // Unreadable is not empty: leave that source's games as they were.
        Err(e) => summary.push(format!("{label}: skipped, {e}")),
    }
    Ok(())
}
