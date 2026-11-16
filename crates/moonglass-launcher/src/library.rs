//! Scan orchestration (spec 5.1): runs every scanner, reconciles each one
//! independently, and refreshes standalone executables' installed flag.
//! Shared by the debug CLI now and the carousel UI later.

use std::collections::HashSet;
use std::path::Path;

use moonglass_core::Source;
use moonglass_db::{Db, ScanScope};
use moonglass_platform::EpicSource;
use moonglass_scan::{epic, heroic, roms, steam, ScanError, ScannedGame};

/// Settings keys.
pub const SETTING_STEAM_ROOT: &str = "steam_root";
pub const SETTING_STEAM_DENY: &str = "steam_extra_deny_app_ids";

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
    let mut summary = ScanSummary::default();

    // Steam: a manual root in Settings wins over discovery.
    let steam_root = db
        .setting::<String>(SETTING_STEAM_ROOT)
        .await?
        .map(Into::into)
        .or_else(moonglass_platform::steam_root);
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
