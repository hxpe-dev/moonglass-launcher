//! File-system watchers (spec 5.1): every Steam `steamapps` folder, the Epic
//! manifests folder (or Heroic's config folders on Linux, Epic and GOG), and every ROM
//! directory. Events are debounced by 2 seconds and collapsed into a single
//! "rescan" signal.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use moonglass_db::Db;
use moonglass_platform::{EpicSource, GogSource};
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};

pub const DEBOUNCE: Duration = Duration::from_secs(2);
/// The debouncer reports each path once it has been quiet for [`DEBOUNCE`],
/// checking every quarter of that, so one burst over several files can
/// arrive as two batches. The signal waits until no batch has come for this
/// long.
const SETTLE: Duration = Duration::from_millis(750);

/// Keeps the watchers alive; drop it to stop watching.
pub struct LibraryWatcher {
    _debouncer: Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>,
    pub watched: Vec<PathBuf>,
}

/// Folders to watch and whether to recurse into them.
pub async fn watch_targets(db: &Db) -> anyhow::Result<Vec<(PathBuf, RecursiveMode)>> {
    let mut out = Vec::new();
    if let Some(root) = crate::library::steam_root(db).await? {
        for lib in moonglass_scan::steam::library_paths(&root).unwrap_or_default() {
            // Manifests sit directly in steamapps; recursing would also
            // watch every game's files while it downloads.
            out.push((lib.join("steamapps"), RecursiveMode::NonRecursive));
        }
    }
    match moonglass_platform::epic_source() {
        Some(EpicSource::EpicManifests(dir)) => out.push((dir, RecursiveMode::NonRecursive)),
        Some(EpicSource::Heroic { installed_json, .. }) => {
            if let Some(dir) = installed_json.parent() {
                out.push((dir.to_path_buf(), RecursiveMode::NonRecursive));
            }
        }
        None => {}
    }
    // GOG on Windows, Battle.net and the Microsoft Store live in the
    // registry, which is not watched: those games are found at startup,
    // after each game and from Settings.
    if let Some(GogSource::Heroic { installed_json, .. }) = moonglass_platform::gog_source() {
        if let Some(dir) = installed_json.parent() {
            out.push((dir.to_path_buf(), RecursiveMode::NonRecursive));
        }
    }
    for d in db.rom_directories().await? {
        let mode = if d.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        out.push((d.path, mode));
    }
    out.retain(|(p, _)| p.is_dir());
    Ok(out)
}

/// Starts watching; `on_change` runs (on its own thread) once per burst of
/// changes.
pub fn start(
    targets: Vec<(PathBuf, RecursiveMode)>,
    on_change: impl Fn() + Send + 'static,
) -> anyhow::Result<LibraryWatcher> {
    let (batch_tx, batch_rx) = mpsc::channel::<()>();
    // Ends when the debouncer, which owns the sender, is dropped.
    std::thread::Builder::new()
        .name("moonglass-watch".into())
        .spawn(move || {
            while batch_rx.recv().is_ok() {
                loop {
                    match batch_rx.recv_timeout(SETTLE) {
                        Ok(()) => {}
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
            }
        })?;
    let mut debouncer = new_debouncer(DEBOUNCE, move |res: DebounceEventResult| match res {
        Ok(events) if !events.is_empty() => {
            let _ = batch_tx.send(());
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "file watcher error"),
    })?;
    let mut watched = Vec::new();
    for (path, mode) in targets {
        match debouncer.watcher().watch(&path, mode) {
            Ok(()) => watched.push(path),
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "cannot watch folder"),
        }
    }
    Ok(LibraryWatcher {
        _debouncer: debouncer,
        watched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounces_bursts_into_one_signal() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = start(
            vec![(tmp.path().to_path_buf(), RecursiveMode::Recursive)],
            move || {
                let _ = tx.send(());
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        for i in 0..5 {
            std::fs::write(tmp.path().join(format!("appmanifest_{i}.acf")), "x").unwrap();
        }
        rx.recv_timeout(Duration::from_secs(10))
            .expect("one signal");
        assert!(
            rx.recv_timeout(Duration::from_millis(2500)).is_err(),
            "burst collapsed"
        );
    }
}
