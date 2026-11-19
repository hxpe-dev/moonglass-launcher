//! SQLite persistence (spec section 6 and the reconciliation rules in 5.1).
//!
//! Queries are compile-time checked by `sqlx` in offline mode: the query
//! metadata lives in `.sqlx/` at the workspace root, so a build needs no
//! database. After changing a query or a migration, regenerate it with
//! `scripts/sqlx-prepare.sh` (see README).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use moonglass_core::{text, Game, GameId, Source};
use moonglass_scan::roms::{parse_extensions, RomDirectory};
use moonglass_scan::ScannedGame;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use thiserror::Error;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("setting {key}: {source}")]
    Json {
        key: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("stored game {id} has unknown source {found}")]
    BadSource { id: String, found: String },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, DbError>;

mod health;
pub use health::Opened;

impl DbError {
    /// SQLite reported the file as damaged or not a database at all
    /// (`SQLITE_CORRUPT`, `SQLITE_NOTADB`).
    pub fn is_damage(&self) -> bool {
        let e = match self {
            DbError::Sqlx(e) | DbError::Migrate(sqlx::migrate::MigrateError::Execute(e)) => e,
            _ => return false,
        };
        let sqlx::Error::Database(d) = e else {
            return false;
        };
        d.code()
            .and_then(|c| c.parse::<i32>().ok())
            .is_some_and(|c| matches!(c & 0xff, 11 | 26))
    }
}

/// Which games one scan is authoritative for. Games inside the scope that
/// the scan did not return are marked uninstalled; games outside it are
/// untouched, so one failing ROM directory cannot hide another's games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanScope {
    Steam,
    Epic,
    Gog,
    BattleNet,
    MsStore,
    RomDirectory(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub added: usize,
    pub updated: usize,
    pub uninstalled: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emulator {
    pub id: String,
    pub name: String,
    pub executable_path: String,
    pub launch_template: String,
    pub file_extensions: String,
    pub folder_marker: Option<String>,
}

/// A registered ROM directory as Settings lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomFolder {
    pub id: String,
    pub path: String,
    pub emulator_id: String,
    pub platform: String,
}

#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Opens (creating if needed) the database file and applies migrations.
    /// Prefer [`Db::open_checked`], which also checks the file and recovers
    /// from damage.
    pub async fn open(path: &Path) -> Result<Self> {
        let db = Self::connect_file(path).await?;
        MIGRATOR.run(&db.pool).await?;
        Ok(db)
    }

    /// Connects without migrating, so the file can be checked first.
    ///
    /// One connection per process: every read and write goes through it,
    /// one at a time. The library is small, and two connections of one
    /// process racing a checkpoint against a write is what SQLite's
    /// WAL-reset bug needs (fixed in SQLite 3.51.3, which is bundled; one
    /// connection keeps the race away regardless).
    pub(crate) async fn connect_file(path: &Path) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }

    /// A private in-memory database, for tests and tools.
    pub async fn open_in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        // One connection: each in-memory connection is its own database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Closes the connection. SQLite then copies the write-ahead log into
    /// the database file and deletes it, so the next start begins from one
    /// complete file. Call it before the process exits.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// `PRAGMA integrity_check`: `Ok(vec![])` when the file is sound,
    /// otherwise SQLite's description of each problem.
    pub async fn integrity_problems(&self) -> Result<Vec<String>> {
        let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().filter(|r| r != "ok").collect())
    }

    /// Copies the write-ahead log into the database file and empties it.
    pub async fn checkpoint(&self) -> Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Writes a consistent copy of the whole database to `dest`, which must
    /// not exist yet.
    pub async fn copy_to(&self, dest: &Path) -> Result<()> {
        sqlx::query("VACUUM INTO ?")
            .bind(dest.to_string_lossy().into_owned())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The SQLite library in use, such as "3.51.3".
    pub async fn sqlite_version(&self) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await?)
    }

    // ---------------------------------------------------------------- games

    pub async fn games(&self) -> Result<Vec<Game>> {
        let rows = sqlx::query_as!(
            GameRow,
            r#"SELECT id, title, sort_title, search_title, source, source_id,
                      launch_target, launch_args, working_dir, rom_directory_id,
                      platform, install_dir, watch_process,
                      track_by_folder AS "track_by_folder: bool",
                      cover_path, hero_path, logo_path, dominant_color, accent_color,
                      artwork_locked AS "artwork_locked: bool",
                      date_added, last_played, playtime_seconds, store_playtime_seconds,
                      is_favorite AS "is_favorite: bool",
                      is_hidden AS "is_hidden: bool",
                      is_installed AS "is_installed: bool"
               FROM games"#
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(GameRow::into_game).collect()
    }

    pub async fn game(&self, id: &GameId) -> Result<Option<Game>> {
        let id = id.as_str();
        let row = sqlx::query_as!(
            GameRow,
            r#"SELECT id, title, sort_title, search_title, source, source_id,
                      launch_target, launch_args, working_dir, rom_directory_id,
                      platform, install_dir, watch_process,
                      track_by_folder AS "track_by_folder: bool",
                      cover_path, hero_path, logo_path, dominant_color, accent_color,
                      artwork_locked AS "artwork_locked: bool",
                      date_added, last_played, playtime_seconds, store_playtime_seconds,
                      is_favorite AS "is_favorite: bool",
                      is_hidden AS "is_hidden: bool",
                      is_installed AS "is_installed: bool"
               FROM games WHERE id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(GameRow::into_game).transpose()
    }

    /// Applies one scan result (spec 5.1): rows are updated in place by
    /// their deterministic ID, games missing from the scan are marked
    /// `is_installed = 0` (never deleted), and Steam's `LastPlayed` seeds
    /// `last_played` only when Moonglass has no value of its own.
    pub async fn reconcile(
        &self,
        scope: &ScanScope,
        scanned: &[ScannedGame],
        now: i64,
    ) -> Result<ReconcileReport> {
        let mut tx = self.pool.begin().await?;

        let (source, rom_dir): (&str, Option<&str>) = match scope {
            ScanScope::Steam => ("steam", None),
            ScanScope::Epic => ("epic", None),
            ScanScope::Gog => ("gog", None),
            ScanScope::BattleNet => ("battlenet", None),
            ScanScope::MsStore => ("msstore", None),
            ScanScope::RomDirectory(id) => ("rom", Some(id.as_str())),
        };

        let before: HashSet<String> = sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM games
               WHERE source = ?1 AND (?2 IS NULL OR rom_directory_id = ?2)"#,
            source,
            rom_dir
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        let installed_before: HashSet<String> = sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM games
               WHERE source = ?1 AND (?2 IS NULL OR rom_directory_id = ?2) AND is_installed = 1"#,
            source,
            rom_dir
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();

        sqlx::query!(
            "UPDATE games SET is_installed = 0
             WHERE source = ?1 AND (?2 IS NULL OR rom_directory_id = ?2)",
            source,
            rom_dir
        )
        .execute(&mut *tx)
        .await?;

        let mut report = ReconcileReport::default();
        let mut seen = HashSet::new();
        for g in scanned {
            let id = g.id.as_str();
            if !seen.insert(id.to_string()) {
                continue;
            }
            let sort_title = text::sort_title(&g.title);
            let search_title = text::search_title(&g.title);
            let src = g.source.as_str();
            sqlx::query!(
                "INSERT INTO games (id, title, sort_title, search_title, source, source_id,
                                    launch_target, install_dir, rom_directory_id, platform,
                                    date_added, last_played, is_installed)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
                 ON CONFLICT (id) DO UPDATE SET
                     title            = excluded.title,
                     sort_title       = excluded.sort_title,
                     search_title     = excluded.search_title,
                     source_id        = excluded.source_id,
                     launch_target    = excluded.launch_target,
                     install_dir      = excluded.install_dir,
                     rom_directory_id = excluded.rom_directory_id,
                     platform         = excluded.platform,
                     last_played      = COALESCE(games.last_played, excluded.last_played),
                     is_installed     = 1",
                id,
                g.title,
                sort_title,
                search_title,
                src,
                g.source_id,
                g.launch_target,
                g.install_dir,
                g.rom_directory_id,
                g.platform,
                now,
                g.last_played_hint,
            )
            .execute(&mut *tx)
            .await?;
            if before.contains(id) {
                report.updated += 1;
            } else {
                report.added += 1;
            }
        }
        report.uninstalled = installed_before.difference(&seen).count();
        tx.commit().await?;
        Ok(report)
    }

    /// Adds a standalone executable chosen by the user. `normalized_path`
    /// comes from the platform layer and determines the ID.
    pub async fn add_executable(
        &self,
        normalized_path: &str,
        path: &str,
        title: &str,
        now: i64,
    ) -> Result<GameId> {
        let id = GameId::exe(normalized_path);
        let id_str = id.as_str();
        let sort_title = text::sort_title(title);
        let search_title = text::search_title(title);
        sqlx::query!(
            "INSERT INTO games (id, title, sort_title, search_title, source, launch_target, date_added)
             VALUES (?, ?, ?, ?, 'exe', ?, ?)
             ON CONFLICT (id) DO UPDATE SET is_installed = 1, launch_target = excluded.launch_target",
            id_str,
            title,
            sort_title,
            search_title,
            path,
            now
        )
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Marks a standalone executable present or missing after checking its
    /// file, so it hides like any other uninstalled game.
    pub async fn set_installed(&self, id: &GameId, installed: bool) -> Result<()> {
        let id = id.as_str();
        sqlx::query!(
            "UPDATE games SET is_installed = ? WHERE id = ?",
            installed,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Removes a standalone executable or ROM entry (per-game option). A
    /// removed ROM reappears if its directory still contains it.
    pub async fn remove_game(&self, id: &GameId) -> Result<()> {
        let id = id.as_str();
        sqlx::query!(
            "DELETE FROM games WHERE id = ? AND source IN ('exe', 'rom')",
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_favorite(&self, id: &GameId, favorite: bool) -> Result<()> {
        let id = id.as_str();
        sqlx::query!(
            "UPDATE games SET is_favorite = ? WHERE id = ?",
            favorite,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_hidden(&self, id: &GameId, hidden: bool) -> Result<()> {
        let id = id.as_str();
        sqlx::query!("UPDATE games SET is_hidden = ? WHERE id = ?", hidden, id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Per-game launch options. `None` clears a value.
    pub async fn set_launch_options(
        &self,
        id: &GameId,
        launch_args: Option<&str>,
        working_dir: Option<&str>,
        watch_process: Option<&str>,
        track_by_folder: bool,
    ) -> Result<()> {
        let id = id.as_str();
        sqlx::query!(
            "UPDATE games SET launch_args = ?, working_dir = ?, watch_process = ?, track_by_folder = ?
             WHERE id = ?",
            launch_args,
            working_dir,
            watch_process,
            track_by_folder,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// End of a session (spec 5.3, Exiting → Idle step 5).
    pub async fn record_session(&self, id: &GameId, ended_at: i64, seconds: i64) -> Result<()> {
        let id = id.as_str();
        let seconds = seconds.max(0);
        sqlx::query!(
            "UPDATE games SET last_played = ?, playtime_seconds = playtime_seconds + ? WHERE id = ?",
            ended_at,
            seconds,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Replaces the store-reported playtime of every game from `source`
    /// with `seconds` (by `source_id`); games missing from it get 0. Used
    /// for Steam, whose count belongs to the signed-in account, so a
    /// switch of account replaces the figures. Returns how many games
    /// have a figure.
    pub async fn set_store_playtimes(
        &self,
        source: Source,
        seconds: &std::collections::HashMap<String, i64>,
    ) -> Result<usize> {
        let src = source.as_str();
        let mut tx = self.pool.begin().await?;
        let ids = sqlx::query!(
            r#"SELECT id AS "id!", source_id FROM games WHERE source = ?"#,
            src
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut with_time = 0;
        for row in ids {
            let s = row
                .source_id
                .as_deref()
                .and_then(|k| seconds.get(k))
                .copied()
                .unwrap_or(0)
                .max(0);
            if s > 0 {
                with_time += 1;
            }
            sqlx::query!(
                "UPDATE games SET store_playtime_seconds = ? WHERE id = ? AND store_playtime_seconds != ?",
                s,
                row.id,
                s
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(with_time)
    }

    /// Stores ingested artwork. Ignored for a game whose artwork the user
    /// chose (`artwork_locked`) unless `user_override` is set, which also
    /// locks it so rescans never replace it (spec 5.2).
    #[allow(clippy::too_many_arguments)]
    pub async fn set_artwork(
        &self,
        id: &GameId,
        cover: &str,
        hero: &str,
        logo: Option<&str>,
        dominant: &str,
        accent: &str,
        user_override: bool,
    ) -> Result<bool> {
        let id = id.as_str();
        let done = sqlx::query!(
            "UPDATE games SET cover_path = ?, hero_path = ?, logo_path = ?,
                              dominant_color = ?, accent_color = ?,
                              artwork_locked = (artwork_locked OR ?)
             WHERE id = ? AND (artwork_locked = 0 OR ?)",
            cover,
            hero,
            logo,
            dominant,
            accent,
            user_override,
            id,
            user_override
        )
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Clears a user override so the automatic sources apply again.
    pub async fn unlock_artwork(&self, id: &GameId) -> Result<()> {
        let id = id.as_str();
        sqlx::query!(
            "UPDATE games SET artwork_locked = 0, cover_path = NULL WHERE id = ?",
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ------------------------------------------------- emulators and ROM dirs

    pub async fn upsert_emulator(&self, e: &Emulator) -> Result<()> {
        sqlx::query!(
            "INSERT INTO emulators (id, name, executable_path, launch_template, file_extensions, folder_marker)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 name = excluded.name, executable_path = excluded.executable_path,
                 launch_template = excluded.launch_template,
                 file_extensions = excluded.file_extensions,
                 folder_marker = excluded.folder_marker",
            e.id,
            e.name,
            e.executable_path,
            e.launch_template,
            e.file_extensions,
            e.folder_marker
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn emulator(&self, id: &str) -> Result<Option<Emulator>> {
        Ok(sqlx::query_as!(
            Emulator,
            "SELECT id, name, executable_path, launch_template, file_extensions, folder_marker
             FROM emulators WHERE id = ?",
            id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn emulators(&self) -> Result<Vec<Emulator>> {
        Ok(sqlx::query_as!(
            Emulator,
            "SELECT id, name, executable_path, launch_template, file_extensions, folder_marker
             FROM emulators ORDER BY name COLLATE NOCASE, id"
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Removes a profile with its ROM directories and their games.
    pub async fn remove_emulator(&self, id: &str) -> Result<()> {
        sqlx::query!("DELETE FROM emulators WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn rom_folders(&self) -> Result<Vec<RomFolder>> {
        Ok(sqlx::query_as!(
            RomFolder,
            "SELECT id, path, emulator_id, platform FROM rom_directories ORDER BY path"
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Removes a ROM directory and the games found in it.
    pub async fn remove_rom_directory(&self, id: &str) -> Result<()> {
        sqlx::query!("DELETE FROM rom_directories WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn add_rom_directory(
        &self,
        id: &str,
        path: &str,
        emulator_id: &str,
        platform: &str,
        recursive: bool,
    ) -> Result<()> {
        sqlx::query!(
            "INSERT INTO rom_directories (id, path, emulator_id, platform, recursive)
             VALUES (?, ?, ?, ?, ?)",
            id,
            path,
            emulator_id,
            platform,
            recursive
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every ROM directory joined with its profile's scanning rules.
    pub async fn rom_directories(&self) -> Result<Vec<RomDirectory>> {
        let rows = sqlx::query!(
            r#"SELECT d.id, d.path, d.platform, d.recursive AS "recursive: bool",
                      e.file_extensions, e.folder_marker
               FROM rom_directories d JOIN emulators e ON e.id = d.emulator_id"#
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| RomDirectory {
                id: r.id,
                path: PathBuf::from(r.path),
                platform: r.platform,
                recursive: r.recursive,
                extensions: parse_extensions(&r.file_extensions),
                folder_marker: r.folder_marker,
            })
            .collect())
    }

    /// The emulator that runs a ROM, reached through its directory.
    pub async fn emulator_for_rom_directory(
        &self,
        rom_directory_id: &str,
    ) -> Result<Option<Emulator>> {
        Ok(sqlx::query_as!(
            Emulator,
            "SELECT e.id, e.name, e.executable_path, e.launch_template, e.file_extensions, e.folder_marker
             FROM emulators e JOIN rom_directories d ON d.emulator_id = e.id
             WHERE d.id = ?",
            rom_directory_id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    // ------------------------------------------------------------- settings

    pub async fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw = sqlx::query_scalar!("SELECT value FROM settings WHERE key = ?", key)
            .fetch_optional(&self.pool)
            .await?;
        raw.map(|v| {
            serde_json::from_str(&v).map_err(|source| DbError::Json {
                key: key.to_string(),
                source,
            })
        })
        .transpose()
    }

    pub async fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value).map_err(|source| DbError::Json {
            key: key.to_string(),
            source,
        })?;
        sqlx::query!(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            key,
            json
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

struct GameRow {
    id: String,
    title: String,
    sort_title: String,
    search_title: String,
    source: String,
    source_id: Option<String>,
    launch_target: Option<String>,
    launch_args: Option<String>,
    working_dir: Option<String>,
    rom_directory_id: Option<String>,
    platform: Option<String>,
    install_dir: Option<String>,
    watch_process: Option<String>,
    track_by_folder: bool,
    cover_path: Option<String>,
    hero_path: Option<String>,
    logo_path: Option<String>,
    dominant_color: Option<String>,
    accent_color: Option<String>,
    artwork_locked: bool,
    date_added: i64,
    last_played: Option<i64>,
    playtime_seconds: i64,
    store_playtime_seconds: i64,
    is_favorite: bool,
    is_hidden: bool,
    is_installed: bool,
}

impl GameRow {
    fn into_game(self) -> Result<Game> {
        let source = Source::parse(&self.source).ok_or_else(|| DbError::BadSource {
            id: self.id.clone(),
            found: self.source.clone(),
        })?;
        Ok(Game {
            id: GameId::from_stored(self.id),
            title: self.title,
            sort_title: self.sort_title,
            search_title: self.search_title,
            source,
            source_id: self.source_id,
            launch_target: self.launch_target,
            launch_args: self.launch_args,
            working_dir: self.working_dir,
            rom_directory_id: self.rom_directory_id,
            platform: self.platform,
            install_dir: self.install_dir,
            watch_process: self.watch_process,
            track_by_folder: self.track_by_folder,
            cover_path: self.cover_path,
            hero_path: self.hero_path,
            logo_path: self.logo_path,
            dominant_color: self.dominant_color,
            accent_color: self.accent_color,
            artwork_locked: self.artwork_locked,
            date_added: self.date_added,
            last_played: self.last_played,
            playtime_seconds: self.playtime_seconds,
            store_playtime_seconds: self.store_playtime_seconds,
            is_favorite: self.is_favorite,
            is_hidden: self.is_hidden,
            is_installed: self.is_installed,
        })
    }
}

#[cfg(test)]
mod tests;
