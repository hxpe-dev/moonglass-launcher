//! Opening the library safely (spec 6, Keeping the library sound).
//!
//! On every start the file is checked with `PRAGMA integrity_check`
//! before any migration touches it. A sound file is then copied to
//! `moonglass.backup.db` next to it, so there is always a last good copy.
//! A damaged file (and its `-wal` / `-shm`) is moved aside, never deleted,
//! and the backup takes its place; with no usable backup, Moonglass starts
//! from an empty library and the next scan finds the games again.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Db, DbError, Result};

/// How [`Db::open_checked`] found the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    Healthy,
    /// The file was damaged; the last good copy was put back. Changes made
    /// since the previous start are lost.
    Restored {
        damaged: PathBuf,
        problems: Vec<String>,
    },
    /// The file was damaged and there was no usable copy: the library
    /// starts empty (favorites, playtime and settings are reset).
    Fresh {
        damaged: PathBuf,
        problems: Vec<String>,
    },
}

enum Fail {
    Damage(Vec<String>),
    Other(DbError),
}

impl From<DbError> for Fail {
    fn from(e: DbError) -> Self {
        if e.is_damage() {
            Fail::Damage(vec![e.to_string()])
        } else {
            Fail::Other(e)
        }
    }
}

/// `moonglass.db` → `moonglass.backup.db`.
pub fn backup_path(db: &Path) -> PathBuf {
    sibling(db, "backup", "")
}

/// `<stem>.<tag>.db<suffix>` in the same folder.
fn sibling(db: &Path, tag: &str, suffix: &str) -> PathBuf {
    let stem = db
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "moonglass".into());
    db.with_file_name(format!("{stem}.{tag}.db{suffix}"))
}

/// The database file with SQLite's `-wal` / `-shm` suffix.
fn with_suffix(db: &Path, suffix: &str) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn io(path: &Path, source: std::io::Error) -> DbError {
    DbError::Io {
        path: path.to_path_buf(),
        source,
    }
}

impl Db {
    /// Opens the library, checking it first and recovering from damage
    /// (see the module docs). Errors other than damage are returned as is.
    pub async fn open_checked(path: &Path) -> Result<(Db, Opened)> {
        let problems = match open_sound(path).await {
            Ok(db) => {
                if let Err(e) = keep_backup(&db, path).await {
                    tracing::warn!(error = %e, "library backup not made");
                }
                return Ok((db, Opened::Healthy));
            }
            Err(Fail::Other(e)) => return Err(e),
            Err(Fail::Damage(p)) => p,
        };
        tracing::error!(?problems, "the library database is damaged");
        let damaged = set_aside(path)?;
        tracing::warn!(to = %damaged.display(), "damaged library moved aside");

        let backup = backup_path(path);
        if backup.is_file() {
            std::fs::copy(&backup, path).map_err(|e| io(path, e))?;
            match open_sound(path).await {
                Ok(db) => {
                    tracing::warn!("library restored from its last good copy");
                    return Ok((db, Opened::Restored { damaged, problems }));
                }
                Err(Fail::Other(e)) => return Err(e),
                Err(Fail::Damage(more)) => {
                    tracing::error!(problems = ?more, "the library backup is damaged too");
                    set_aside(path)?;
                }
            }
        }
        let db = Db::open(path).await?;
        Ok((db, Opened::Fresh { damaged, problems }))
    }
}

/// Connects, checks the file, then migrates. Closes the connection again
/// when the file is damaged, so it can be moved.
async fn open_sound(path: &Path) -> std::result::Result<Db, Fail> {
    let db = Db::connect_file(path).await?;
    let checked = async {
        let problems = db.integrity_problems().await?;
        if !problems.is_empty() {
            return Err(Fail::Damage(problems));
        }
        crate::migrate(&db.pool).await.map_err(Fail::from)?;
        // A log left by a process that did not close the library (killed,
        // crashed) goes into the file now rather than growing further.
        db.checkpoint().await?;
        Ok(())
    }
    .await;
    match checked {
        Ok(()) => Ok(db),
        Err(e) => {
            db.close().await;
            Err(e)
        }
    }
}

/// Replaces `moonglass.backup.db` with a copy of the (sound) library.
async fn keep_backup(db: &Db, path: &Path) -> Result<()> {
    let backup = backup_path(path);
    let tmp = sibling(path, "backup", ".tmp");
    let _ = std::fs::remove_file(&tmp);
    db.copy_to(&tmp).await?;
    std::fs::rename(&tmp, &backup).map_err(|e| io(&backup, e))
}

/// Moves the database and its `-wal` / `-shm` to
/// `<stem>.damaged-<unix time>.db…`, and returns the new database path.
fn set_aside(path: &Path) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut n = 0;
    let target = loop {
        let tag = match n {
            0 => format!("damaged-{now}"),
            n => format!("damaged-{now}-{n}"),
        };
        let t = sibling(path, &tag, "");
        if !t.exists() {
            break t;
        }
        n += 1;
    };
    for suffix in ["", "-wal", "-shm"] {
        let from = with_suffix(path, suffix);
        if from.exists() {
            let to = with_suffix(&target, suffix);
            std::fs::rename(&from, &to).map_err(|e| io(&from, e))?;
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moonglass_core::GameId;

    async fn library(path: &Path) -> Db {
        let (db, opened) = Db::open_checked(path).await.unwrap();
        assert_eq!(opened, Opened::Healthy);
        db
    }

    #[test]
    fn names() {
        let p = Path::new("/d/moonglass.db");
        assert_eq!(backup_path(p), Path::new("/d/moonglass.backup.db"));
        assert_eq!(with_suffix(p, "-wal"), PathBuf::from("/d/moonglass.db-wal"));
    }

    #[tokio::test]
    async fn bundled_sqlite_has_the_wal_reset_fix() {
        let db = Db::open_in_memory().await.unwrap();
        let v = db.sqlite_version().await.unwrap();
        let parts: Vec<u32> = v.split('.').map(|p| p.parse().unwrap()).collect();
        assert!(
            parts >= vec![3, 51, 3],
            "SQLite {v} predates the WAL-reset fix"
        );
    }

    #[tokio::test]
    async fn healthy_start_keeps_a_backup_and_closes_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("moonglass.db");
        let db = library(&path).await;
        db.set_setting("k", &1).await.unwrap();
        db.close().await;
        // Closing merged the log into the file and removed it.
        assert!(!with_suffix(&path, "-wal").exists());
        let db = library(&path).await;
        assert!(backup_path(&path).is_file());
        assert_eq!(db.setting::<i32>("k").await.unwrap(), Some(1));
        db.close().await;
    }

    /// Breaks the file the way it broke in practice: the settings table
    /// and its index disagree.
    async fn damage(path: &Path) {
        let db = Db::open(path).await.unwrap();
        sqlx::query("PRAGMA writable_schema = ON")
            .execute(&db.pool)
            .await
            .unwrap();
        // Point the settings index at the games table's root page.
        sqlx::query(
            "UPDATE sqlite_master SET rootpage = (SELECT rootpage FROM sqlite_master WHERE name = 'games')
             WHERE name = 'sqlite_autoindex_settings_1'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        db.close().await;
    }

    #[tokio::test]
    async fn damaged_library_is_set_aside_and_restored() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("moonglass.db");
        let db = library(&path).await;
        db.set_favorite(&GameId::steam("1"), true).await.unwrap();
        db.set_setting("kept", &true).await.unwrap();
        db.close().await;
        // The next start makes the backup that holds "kept".
        library(&path).await.close().await;

        damage(&path).await;
        let (db, opened) = Db::open_checked(&path).await.unwrap();
        let Opened::Restored { damaged, problems } = opened else {
            panic!("expected a restore, got {opened:?}");
        };
        assert!(!problems.is_empty());
        assert!(damaged.is_file(), "the damaged file is kept");
        assert_eq!(db.setting::<bool>("kept").await.unwrap(), Some(true));
        assert!(db.integrity_problems().await.unwrap().is_empty());
        db.close().await;
    }

    #[tokio::test]
    async fn damaged_library_without_backup_starts_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("moonglass.db");
        let db = Db::open(&path).await.unwrap();
        db.set_setting("lost", &true).await.unwrap();
        db.close().await;
        damage(&path).await;
        let (db, opened) = Db::open_checked(&path).await.unwrap();
        assert!(matches!(opened, Opened::Fresh { .. }), "{opened:?}");
        assert_eq!(db.setting::<bool>("lost").await.unwrap(), None);
        db.close().await;
    }
}
