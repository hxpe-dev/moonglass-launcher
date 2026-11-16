//! The platform layer (spec 4.3).
//!
//! Every OS difference in Moonglass lives in this crate: the public API below is
//! the one interface, and `windows.rs` / `linux.rs` are its two
//! implementations. Other crates call these functions and never use
//! `cfg(windows)` themselves.
//!
//! | Capability | Here |
//! | --- | --- |
//! | Open a launcher URI / start a store game | [`launch_steam`], [`launch_epic`], [`open_uri`] |
//! | Track and kill a spawned game | [`TrackedChild`] |
//! | Detect a launcher-started game | [`ProcessWatcher`] + [`GameMatcher`] |
//! | Hide Moonglass / regain focus / dismiss the launcher window | [`window`] |
//! | Background restore hotkey | [`hotkey`] |
//! | Start at login | [`start_at_login`], [`set_start_at_login`] |
//! | Single-instance guard | [`InstanceGuard`] |
//! | Steam and Epic discovery | [`steam_root`], [`epic_source`] |
//! | File locations | [`AppPaths`] |

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use thiserror::Error;

#[cfg(unix)]
#[path = "linux.rs"]
mod imp;
#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

pub use imp::{hotkey, window};

/// Which platform this build is for, for logs and Settings text only.
pub const PLATFORM_NAME: &str = imp::PLATFORM_NAME;

/// Whether the foreground gamepad thread must also watch for the background
/// restore combination (true where its input does not depend on focus).
pub const GAMEPAD_RESTORE_VIA_GILRS: bool = imp::GAMEPAD_RESTORE_VIA_GILRS;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{0}")]
    Os(String),
    #[error("could not determine the user's home or app-data folders")]
    NoHome,
}

pub type Result<T> = std::result::Result<T, PlatformError>;

// ------------------------------------------------------------------ paths

/// Where Moonglass keeps its files (spec 4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// Holds `moonglass.db`.
    pub data_dir: PathBuf,
    /// Settings-adjacent files (the DB stores settings itself).
    pub config_dir: PathBuf,
    /// Artwork and derived textures.
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        imp::app_paths()
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("moonglass.db")
    }

    pub fn create_all(&self) -> io::Result<()> {
        for d in [
            &self.data_dir,
            &self.config_dir,
            &self.cache_dir,
            &self.log_dir,
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------- path identity

/// Canonical string for a path, used for `exe:` and `rom:` IDs and for
/// comparing paths (spec 6): symlinks and `.`/`..` resolved, and lowercased
/// on Windows where paths are case-insensitive. Works for paths that no
/// longer exist by falling back to lexical cleanup.
pub fn normalize_path(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| lexical_clean(path));
    imp::finish_normalize(&resolved)
}

fn lexical_clean(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `child` is `dir` or inside it, using platform path identity.
pub fn path_is_within(child: &Path, dir: &Path) -> bool {
    within_normalized(child, &normalize_path(dir))
}

/// `dir` must already be a [`normalize_path`] result. `child` is a path
/// reported by the OS (already absolute and resolved), so only case is
/// folded, which keeps per-process checks free of file-system calls.
pub(crate) fn within_normalized(child: &Path, dir: &str) -> bool {
    let c = imp::finish_normalize(child);
    let d = dir.trim_end_matches(['/', '\\']);
    c == d
        || (c.len() > d.len() && c.starts_with(d) && matches!(c.as_bytes()[d.len()], b'/' | b'\\'))
}

// ------------------------------------------------------ launcher discovery

/// The Steam install root (spec 2): the registry on Windows; the first
/// existing known directory on Linux (native, legacy symlink, Flatpak, Snap).
pub fn steam_root() -> Option<PathBuf> {
    imp::steam_root()
}

/// Where Epic games are read from and how they are started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpicSource {
    /// Windows: the Epic Games Launcher's `.item` manifests folder.
    EpicManifests(PathBuf),
    /// Linux: Heroic's Legendary `installed.json`, and the command that
    /// runs Heroic (`["heroic"]` or `["flatpak", "run", "com.heroicgameslauncher.hgl"]`).
    Heroic {
        installed_json: PathBuf,
        command: Vec<String>,
    },
}

/// `None` when Epic is not available on this machine (on Linux: Heroic is
/// not installed), in which case the Epic source and pill do not appear.
pub fn epic_source() -> Option<EpicSource> {
    imp::epic_source()
}

/// A launcher client whose window Moonglass may dismiss after a game exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    Steam,
    Epic,
}

/// Outcome of [`window::regain_focus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusResult {
    Focused,
    /// All attempts failed; attention was requested. The caller shows the
    /// "how to return" notification and logs it.
    NotFocused,
    /// This platform requests activation through the UI toolkit (Linux).
    UseToolkit,
}

// ---------------------------------------------------------------- launching

/// Hands a URI to the OS handler (`ShellExecuteW` / `xdg-open`).
pub fn open_uri(uri: &str) -> Result<()> {
    imp::open_uri(uri)
}

/// Starts a Steam game through the Steam client.
pub fn launch_steam(app_id: &str) -> Result<()> {
    imp::launch_steam(app_id)
}

/// Starts an Epic game through the Epic client (Windows) or Heroic (Linux).
/// `fallback` selects the namespaced URI form on Windows, for titles that
/// do not launch from the bare `AppName`.
pub fn launch_epic(source: &EpicSource, app_name: &str, fallback_uri: Option<&str>) -> Result<()> {
    imp::launch_epic(source, app_name, fallback_uri)
}

/// A standalone executable or emulator started by Moonglass, placed in a Job
/// Object (Windows) or its own process group (Linux) so that it and its
/// children can be observed and terminated together.
pub struct TrackedChild {
    inner: imp::Tracked,
}

impl TrackedChild {
    /// Spawns `program` with an argument vector; nothing goes through a
    /// shell. `cwd` defaults to the program's folder.
    pub fn spawn(program: &Path, args: &[String], cwd: Option<&Path>) -> Result<Self> {
        let default_cwd = program.parent().filter(|p| !p.as_os_str().is_empty());
        let cwd = cwd.or(default_cwd);
        Ok(Self {
            inner: imp::Tracked::spawn(program, args, cwd)?,
        })
    }

    /// Whether any process of the group or job is still running.
    pub fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }

    /// Force quit: terminates the whole job or process group.
    pub fn terminate(&mut self) -> Result<()> {
        self.inner.terminate()
    }

    pub fn pid(&self) -> u32 {
        self.inner.pid()
    }
}

// --------------------------------------------------------- game detection

/// Helper processes never counted as "the game" (spec 5.3). Matched
/// case-insensitively against the executable's file name. Users can extend
/// this list.
pub const IGNORED_PROCESSES: &[&str] = &[
    "UnityCrashHandler64.exe",
    "UnityCrashHandler32.exe",
    "CrashReportClient.exe",
    "CrashReportClientEditor.exe",
    "crashpad_handler",
    "crashpad_handler.exe",
    "CrashSender1403.exe",
    "BsSndRpt.exe",
    "BsSndRpt64.exe",
    "vc_redist.x64.exe",
    "vc_redist.x86.exe",
    "vcredist_x64.exe",
    "vcredist_x86.exe",
    "DXSETUP.exe",
    "dotNetFx40_Full_setup.exe",
    "EasyAntiCheat_Setup.exe",
    "EasyAntiCheat_EOS_Setup.exe",
    "UE4PrereqSetup_x64.exe",
    "UEPrereqSetup_x64.exe",
];

/// What identifies a running game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameMatcher {
    /// A Steam game. Linux checks `SteamAppId` in the environment first
    /// (covers Proton, where the executable is Wine); both platforms fall
    /// back to the install folder.
    Steam {
        app_id: String,
        install_dir: PathBuf,
    },
    /// Epic, or a standalone executable with folder tracking: any process
    /// whose executable (Windows) or executable, working directory or
    /// command line (Linux) is inside the folder.
    Folder { install_dir: PathBuf },
    /// Per-game override: an exact process name.
    ProcessName(String),
}

/// Polls the process table (once per second while a game runs).
pub struct ProcessWatcher {
    sys: System,
    ignored: Vec<String>,
}

impl ProcessWatcher {
    pub fn new(extra_ignored: &[String]) -> Self {
        let ignored = IGNORED_PROCESSES
            .iter()
            .map(|s| s.to_lowercase())
            .chain(extra_ignored.iter().map(|s| s.to_lowercase()))
            .collect();
        Self {
            sys: System::new(),
            ignored,
        }
    }

    /// Refreshes the process list. Path details are read once per process
    /// (`OnlyIfNotSet`) to keep each poll cheap.
    pub fn refresh(&mut self) {
        let kind = ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_environ(UpdateKind::OnlyIfNotSet);
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    }

    /// Whether any non-ignored process matches. Call [`refresh`](Self::refresh) first.
    pub fn any_matching(&self, matcher: &GameMatcher) -> bool {
        let prepared = Prepared::new(matcher);
        let own = std::process::id();
        self.sys.processes().values().any(|p| {
            if p.pid().as_u32() == own || p.thread_kind().is_some() {
                return false;
            }
            let name = p
                .exe()
                .and_then(Path::file_name)
                .unwrap_or_else(|| p.name());
            if self.is_ignored(name) {
                return false;
            }
            let info = ProcessInfo {
                name,
                exe: p.exe(),
                cwd: p.cwd(),
                cmd: p.cmd(),
                environ: p.environ(),
            };
            imp::process_matches(&info, &prepared)
        })
    }

    fn is_ignored(&self, name: &OsStr) -> bool {
        let n = name.to_string_lossy().to_lowercase();
        self.ignored.contains(&n)
    }
}

/// A matcher with its folder normalized once per poll.
pub(crate) enum Prepared<'a> {
    #[cfg_attr(windows, allow(dead_code))]
    Steam {
        app_id: &'a str,
        dir: String,
    },
    Folder {
        dir: String,
    },
    Name(&'a str),
}

impl<'a> Prepared<'a> {
    fn new(m: &'a GameMatcher) -> Self {
        match m {
            GameMatcher::Steam {
                app_id,
                install_dir,
            } => Prepared::Steam {
                app_id,
                dir: normalize_path(install_dir),
            },
            GameMatcher::Folder { install_dir } => Prepared::Folder {
                dir: normalize_path(install_dir),
            },
            GameMatcher::ProcessName(n) => Prepared::Name(n),
        }
    }
}

/// The parts of a process the matchers look at.
pub(crate) struct ProcessInfo<'a> {
    pub name: &'a OsStr,
    pub exe: Option<&'a Path>,
    #[cfg_attr(windows, allow(dead_code))]
    pub cwd: Option<&'a Path>,
    #[cfg_attr(windows, allow(dead_code))]
    pub cmd: &'a [std::ffi::OsString],
    #[cfg_attr(windows, allow(dead_code))]
    pub environ: &'a [std::ffi::OsString],
}

pub(crate) fn name_matches(name: &OsStr, wanted: &str) -> bool {
    let n = name.to_string_lossy();
    let w = wanted.trim();
    n.eq_ignore_ascii_case(w)
        || n.strip_suffix(".exe")
            .is_some_and(|s| s.eq_ignore_ascii_case(w))
        || n.strip_suffix(".EXE")
            .is_some_and(|s| s.eq_ignore_ascii_case(w))
}

// ------------------------------------------------------- start at login

pub fn start_at_login() -> Result<bool> {
    imp::start_at_login()
}

pub fn set_start_at_login(enabled: bool) -> Result<()> {
    imp::set_start_at_login(enabled)
}

// ------------------------------------------------------- single instance

/// Held for the life of the process. A second launch signals the first one
/// to come forward and then exits.
pub struct InstanceGuard {
    #[allow(dead_code)]
    inner: imp::Instance,
}

pub enum InstanceStatus {
    /// This process is the only instance.
    Primary(InstanceGuard),
    /// Another instance is running and has been asked to come forward.
    AlreadyRunning,
}

impl InstanceGuard {
    pub fn acquire() -> Result<InstanceStatus> {
        Ok(match imp::Instance::acquire()? {
            Some(inner) => InstanceStatus::Primary(Self { inner }),
            None => InstanceStatus::AlreadyRunning,
        })
    }

    /// True once per activation request from a second launch since the
    /// last call. Poll this from the UI loop.
    pub fn take_activation_request(&self) -> bool {
        self.inner.take_activation_request()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within() {
        let base = std::env::temp_dir().join("moonglass-within");
        assert!(path_is_within(&base.join("a").join("b.exe"), &base));
        assert!(path_is_within(&base, &base));
        assert!(!path_is_within(
            &PathBuf::from(format!("{}x", base.display())),
            &base
        ));
        assert!(!path_is_within(&std::env::temp_dir(), &base));
    }

    #[test]
    fn normalize_is_stable_for_missing_paths() {
        let p = std::env::temp_dir()
            .join("moonglass-missing")
            .join(".")
            .join("x")
            .join("..")
            .join("game.bin");
        let n = normalize_path(&p);
        assert_eq!(
            n,
            normalize_path(
                &std::env::temp_dir()
                    .join("moonglass-missing")
                    .join("game.bin")
            )
        );
        assert!(!n.contains(".."));
    }

    #[test]
    fn normalize_resolves_existing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("Game.EXE");
        std::fs::write(&f, "").unwrap();
        let n = normalize_path(&f);
        assert!(!n.starts_with(r"\\?\"));
        assert_eq!(n, normalize_path(&dir.path().join(".").join("Game.EXE")));
    }

    #[test]
    fn process_names() {
        assert!(name_matches(OsStr::new("Game.exe"), "game.exe"));
        assert!(name_matches(OsStr::new("Game.exe"), "Game"));
        assert!(name_matches(OsStr::new("game"), "game"));
        assert!(!name_matches(OsStr::new("Gamer.exe"), "game"));
    }

    #[test]
    fn tracked_child_lifecycle() {
        let mut child = spawn_sleeper();
        assert!(child.is_alive());
        child.terminate().unwrap();
        for _ in 0..50 {
            if !child.is_alive() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("child still alive after terminate");
    }

    #[test]
    fn watcher_finds_child_by_folder_and_name() {
        let mut child = spawn_sleeper();
        let exe = sleeper_program();
        let mut w = ProcessWatcher::new(&[]);
        w.refresh();
        let folder = GameMatcher::Folder {
            install_dir: exe.parent().unwrap().to_path_buf(),
        };
        assert!(w.any_matching(&folder));
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert!(w.any_matching(&GameMatcher::ProcessName(name.clone())));
        assert!(!w.any_matching(&GameMatcher::ProcessName(
            "definitely-not-running-xyz".into()
        )));
        let mut ignoring = ProcessWatcher::new(std::slice::from_ref(&name));
        ignoring.refresh();
        assert!(!ignoring.any_matching(&GameMatcher::ProcessName(name)));
        child.terminate().unwrap();
    }

    #[cfg(windows)]
    fn sleeper_program() -> PathBuf {
        let sys = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(sys).join("System32").join("PING.EXE")
    }

    #[cfg(windows)]
    fn spawn_sleeper() -> TrackedChild {
        TrackedChild::spawn(
            &sleeper_program(),
            &["-n".into(), "30".into(), "127.0.0.1".into()],
            None,
        )
        .unwrap()
    }

    #[cfg(unix)]
    fn sleeper_program() -> PathBuf {
        PathBuf::from("/bin/sleep")
    }

    #[cfg(unix)]
    fn spawn_sleeper() -> TrackedChild {
        TrackedChild::spawn(&sleeper_program(), &["30".into()], None).unwrap()
    }
}
