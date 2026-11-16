//! Library scanners (spec 2 and 5.1).
//!
//! Each scanner reads one launcher's files from paths it is given and
//! returns [`ScannedGame`]s. Where those files live on each OS is decided by
//! the platform layer, and how results are merged into the database is
//! decided by `moonglass-db`. A scanner that returns `Err` means "could not
//! read this source", which is different from "this source has no games":
//! the caller must not mark that source's games as uninstalled on error.

use std::path::{Path, PathBuf};

use moonglass_core::{GameId, Source};
use thiserror::Error;

pub mod epic;
pub mod heroic;
pub mod roms;
pub mod steam;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

impl ScanError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn parse(path: &Path, message: impl ToString) -> Self {
        Self::Parse {
            path: path.to_path_buf(),
            message: message.to_string(),
        }
    }
}

/// What a scanner knows about one game. User-owned fields (favorites,
/// artwork, launch arguments, playtime…) are never produced here, so a
/// rescan cannot overwrite them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedGame {
    pub id: GameId,
    pub title: String,
    pub source: Source,
    /// Steam AppID or Epic AppName.
    pub source_id: Option<String>,
    /// ROM path (ROMs only; executables are added by the user, not scanned).
    pub launch_target: Option<String>,
    pub install_dir: Option<String>,
    pub rom_directory_id: Option<String>,
    pub platform: Option<String>,
    /// Steam's own `LastPlayed` (Unix seconds), used to seed `last_played`
    /// when Moonglass has no value of its own.
    pub last_played_hint: Option<i64>,
}

/// Turns a path into the canonical string used for IDs. Supplied by the
/// platform layer (symlinks, relative segments, case on Windows).
pub type Normalizer<'a> = &'a dyn Fn(&Path) -> String;

pub(crate) fn read_to_string(path: &Path) -> Result<String, ScanError> {
    let bytes = std::fs::read(path).map_err(|e| ScanError::io(path, e))?;
    // Launcher files are UTF-8 in practice; tolerate stray bytes rather than
    // dropping a whole library.
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
