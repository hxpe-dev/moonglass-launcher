//! The platform layer (spec 4.3).
//!
//! Every OS difference in Moonglass lives in this crate: the public API below is
//! the one interface, and `windows.rs` / `linux.rs` are its two
//! implementations. Other crates call these functions and never use
//! `cfg(windows)` themselves.
//!
//! | Capability | Here |
//! | --- | --- |
//! | Start a store game | [`launch_steam`], [`launch_epic`] |
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

use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use thiserror::Error;

#[cfg(unix)]
#[path = "linux.rs"]
mod imp;
#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

pub use imp::{hotkey, window};

/// The controller buttons held to return to Moonglass during a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestoreCombo {
    /// View + Menu (Back + Start).
    #[default]
    ViewMenu,
    /// Both sticks pressed in (L3 + R3).
    StickClicks,
}

/// The restore hotkey's buttons and hold time, shared with the threads
/// that watch for it so a change in Settings applies at once.
#[derive(Debug)]
pub struct RestoreConfig {
    combo: std::sync::atomic::AtomicU8,
    hold_ms: std::sync::atomic::AtomicU32,
}

impl RestoreConfig {
    pub fn new(combo: RestoreCombo, hold: Duration) -> std::sync::Arc<Self> {
        let c = Self {
            combo: Default::default(),
            hold_ms: Default::default(),
        };
        c.set(combo, hold);
        std::sync::Arc::new(c)
    }

    pub fn set(&self, combo: RestoreCombo, hold: Duration) {
        use std::sync::atomic::Ordering::Relaxed;
        self.combo.store(combo as u8, Relaxed);
        self.hold_ms
            .store(hold.as_millis().min(u32::MAX as u128) as u32, Relaxed);
    }

    pub fn combo(&self) -> RestoreCombo {
        match self.combo.load(std::sync::atomic::Ordering::Relaxed) {
            1 => RestoreCombo::StickClicks,
            _ => RestoreCombo::ViewMenu,
        }
    }

    pub fn hold(&self) -> Duration {
        Duration::from_millis(self.hold_ms.load(std::sync::atomic::Ordering::Relaxed) as u64)
    }
}

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
    #[error("Steam did not sign in to that account; it may be asking for its password")]
    SteamSignIn,
}

pub type Result<T> = std::result::Result<T, PlatformError>;

// ------------------------------------------------------------------ paths

/// Where Moonglass keeps its files (spec 4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// Holds `moonglass.db`, which also stores the settings.
    pub data_dir: PathBuf,
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
        for d in [&self.data_dir, &self.cache_dir, &self.log_dir] {
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
/// `dir` must already be a [`normalize_path`] result. `child` is a path
/// reported by the OS (already absolute and resolved), so only case is
/// folded, which keeps per-process checks free of file-system calls.
pub(crate) fn within_normalized(child: &Path, dir: &str) -> bool {
    let c = imp::finish_normalize(child);
    let d = dir.trim_end_matches(['/', '\\']);
    c == d
        || (c.len() > d.len() && c.starts_with(d) && matches!(c.as_bytes()[d.len()], b'/' | b'\\'))
}

/// Whether `dir` is narrow enough to stand for one game when matching "any
/// process under this folder". A drive root, the home folder, `Program
/// Files`, the system folder and the like would match unrelated processes,
/// and Stop / Force stop act on every match, so such folders never match
/// anything.
pub fn is_trackable_folder(dir: &Path) -> bool {
    trackable_dir(dir).is_some()
}

/// The [`normalize_path`] form of `dir` if it is [trackable](is_trackable_folder).
fn trackable_dir(dir: &Path) -> Option<String> {
    if !dir.is_absolute() {
        return None;
    }
    let d = normalize_path(dir);
    let d_path = Path::new(&d);
    d_path.parent()?;
    // `dir` is one of the shared folders, or a parent of one.
    if imp::broad_folders()
        .iter()
        .any(|b| within_normalized(Path::new(&normalize_path(b)), &d))
    {
        return None;
    }
    // `dir` is inside the operating system's own folder.
    if imp::system_folders()
        .iter()
        .any(|s| within_normalized(d_path, &normalize_path(s)))
    {
        return None;
    }
    Some(d)
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

/// Starts a Steam game through the Steam client. `args` are extra launch
/// options for this start, passed on Steam's command line (`-applaunch`),
/// because the `steam://` URI cannot carry them without Steam asking the
/// player to confirm.
pub fn launch_steam(app_id: &str, args: &[String]) -> Result<()> {
    imp::launch_steam(app_id, args)
}

/// Joins a friend's Steam session: `lobby` and `friend` are the lobby's and
/// the friend's SteamID64, as Steam's Web API reports them. Steam starts
/// the game if needed and connects it.
pub fn join_steam_lobby(app_id: &str, lobby: &str, friend: &str) -> Result<()> {
    // The values come from the network and end up in a URI: digits only.
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !(numeric(app_id) && numeric(lobby) && numeric(friend)) {
        return Err(PlatformError::Os("not a Steam lobby".into()));
    }
    imp::open_uri(&format!("steam://joinlobby/{app_id}/{lobby}/{friend}"))
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
    /// back to the install folder, if it is [trackable](is_trackable_folder).
    Steam {
        app_id: String,
        install_dir: PathBuf,
    },
    /// Epic, or a standalone executable with folder tracking: any process
    /// whose executable (Windows) or executable, working directory or
    /// command line (Linux) is inside the folder. A folder that is not
    /// [trackable](is_trackable_folder) matches nothing.
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
        !self.matching_pids(matcher).is_empty()
    }

    /// Process IDs of every non-ignored process matching the game.
    pub fn matching_pids(&self, matcher: &GameMatcher) -> Vec<u32> {
        let prepared = Prepared::new(matcher);
        let own = std::process::id();
        let mut out: Vec<u32> = self
            .sys
            .processes()
            .values()
            .filter(|p| {
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
            .map(|p| p.pid().as_u32())
            .collect();
        out.sort_unstable();
        out
    }

    /// Stop button, first press: asks the game to close normally (window
    /// close request on Windows, SIGTERM on Linux) so it can save. Returns
    /// how many processes were asked.
    pub fn request_close(&self, matcher: &GameMatcher) -> usize {
        let pids = self.matching_pids(matcher);
        imp::request_close(&pids);
        pids.len()
    }

    /// Stop button, when the game ignored the close request: terminates
    /// every matching process.
    pub fn force_stop(&self, matcher: &GameMatcher) -> usize {
        let pids = self.matching_pids(matcher);
        let mut n = 0;
        for pid in &pids {
            if let Some(p) = self.sys.process(sysinfo::Pid::from_u32(*pid)) {
                if p.kill() {
                    n += 1;
                }
            }
        }
        n
    }

    fn is_ignored(&self, name: &OsStr) -> bool {
        let n = name.to_string_lossy().to_lowercase();
        self.ignored.contains(&n)
    }
}

/// A matcher with its folder normalized once per poll. `dir` is `None` for
/// a folder too broad to identify one game.
pub(crate) enum Prepared<'a> {
    Steam {
        #[cfg_attr(windows, allow(dead_code))]
        app_id: &'a str,
        dir: Option<String>,
    },
    Folder {
        dir: Option<String>,
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
                dir: trackable_dir(install_dir),
            },
            GameMatcher::Folder { install_dir } => Prepared::Folder {
                dir: trackable_dir(install_dir),
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

// ------------------------------------------------------- file browser

/// Starting points for the built-in file browser (spec 5.5): the drive
/// list on Windows; home plus mounted volumes on Linux.
pub fn browse_roots() -> Vec<(String, PathBuf)> {
    imp::browse_roots()
}

/// Whether a file can be added as a standalone executable: `.exe` on
/// Windows, the executable bit on Linux (binaries, AppImages, scripts).
pub fn is_launchable(path: &Path) -> bool {
    imp::is_launchable(path)
}

/// Polite close request for specific processes (see
/// [`ProcessWatcher::request_close`]).
pub fn request_close_pids(pids: &[u32]) {
    imp::request_close(pids)
}

// ------------------------------------------------------- controllers

/// Controller power as the OS reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadPower {
    Wired,
    /// Coarse level 0 (empty) to 3 (full), as XInput reports it.
    Level(u8),
}

/// Power of the first connected XInput controller, where the OS has that
/// (Windows). `gilrs` cannot read it there, so the UI asks here instead.
pub fn xinput_power() -> Option<PadPower> {
    imp::xinput_power()
}

// ------------------------------------------------------- Steam signal

/// Steam's own per-game running flag
/// (`HKCU\Software\Valve\Steam\Apps\<AppID>\Running` on Windows).
/// `None` where it does not exist (Linux) or Steam has no entry for the
/// game. Spec 5.3: logged next to process detection until it is shown to
/// be dependable.
pub fn steam_app_running(app_id: &str) -> Option<bool> {
    imp::steam_app_running(app_id)
}

/// The first of `app_ids` installed as a Flatpak (Linux; always `None` on
/// Windows).
pub fn installed_flatpak(app_ids: &[&str]) -> Option<String> {
    app_ids
        .iter()
        .find(|id| imp::flatpak_app_installed(id))
        .map(|id| id.to_string())
}

// ------------------------------------------------- Steam accounts

/// The 32-bit account ID Steam is signed in with right now, `None` when
/// Steam is not running or no one is signed in
/// (`HKCU\Software\Valve\Steam\ActiveProcess\ActiveUser` on Windows,
/// the same value in `registry.vdf` on Linux).
pub fn steam_active_account() -> Option<u32> {
    imp::steam_active_account().filter(|&id| id != 0)
}

fn steam_running() -> bool {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .values()
        .any(|p| p.name().eq_ignore_ascii_case(imp::STEAM_PROCESS))
}

/// Polls `done` every quarter second until it holds or `timeout` passes.
fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Signs Steam in as another remembered account (one that saved its
/// password): closes Steam if it runs, makes `account_name` the account it
/// signs in with automatically, starts it again in the background, and
/// waits until that account is signed in. Blocks for up to about
/// `timeout` plus the time Steam takes to close.
///
/// No password is involved: Steam uses the sign-in token it kept. When
/// that token has expired, Steam shows its own sign-in window and this
/// returns [`PlatformError::SteamSignIn`].
pub fn switch_steam_account(account_name: &str, account_id: u32, timeout: Duration) -> Result<()> {
    // Steam login names are letters, digits and underscores; anything else
    // is refused before it reaches the registry or a config file.
    let valid = !account_name.is_empty()
        && account_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !valid {
        return Err(PlatformError::Os("not a Steam account name".into()));
    }
    if steam_active_account() == Some(account_id) {
        return Ok(());
    }
    if steam_running() {
        imp::run_steam(&["-shutdown"])?;
        if !wait_for(Duration::from_secs(40), || !steam_running()) {
            return Err(PlatformError::Os("Steam did not close".into()));
        }
    }
    // After Steam has closed: it saves its settings on the way out.
    imp::set_steam_auto_login(account_name)?;
    imp::run_steam(&["-silent"])?;
    if wait_for(timeout, || steam_active_account() == Some(account_id)) {
        Ok(())
    } else {
        Err(PlatformError::SteamSignIn)
    }
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
        let within = |child: &Path| within_normalized(child, &normalize_path(&base));
        assert!(within(&base.join("a").join("b.exe")));
        assert!(within(&base));
        assert!(!within(&PathBuf::from(format!("{}x", base.display()))));
        assert!(!within(&std::env::temp_dir()));
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
    fn lobby_ids_must_be_numeric() {
        assert!(join_steam_lobby("620", "109775241", "7656119 & calc").is_err());
        assert!(join_steam_lobby("620", "", "76561190000000001").is_err());
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
    fn broad_folders_are_not_trackable() {
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert!(!is_trackable_folder(&home));
        assert!(!is_trackable_folder(Path::new("")));
        assert!(!is_trackable_folder(Path::new("relative/folder")));
        assert!(!is_trackable_folder(home.ancestors().last().unwrap()));
        // The folder of a system program (System32, /bin).
        assert!(!is_trackable_folder(sleeper_program().parent().unwrap()));
        assert!(is_trackable_folder(&home.join("Games").join("Some Game")));
        let tmp = tempfile::tempdir().unwrap();
        assert!(is_trackable_folder(tmp.path()));
    }

    #[test]
    fn watcher_finds_child_by_folder_and_name() {
        // A private copy, so the folder holds nothing but this "game".
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(sleeper_program().file_name().unwrap());
        std::fs::copy(sleeper_program(), &exe).unwrap();
        let mut child = TrackedChild::spawn(&exe, &sleeper_args(), None).unwrap();
        let mut w = ProcessWatcher::new(&[]);
        w.refresh();
        let folder = GameMatcher::Folder {
            install_dir: tmp.path().to_path_buf(),
        };
        assert_eq!(w.matching_pids(&folder), [child.pid()]);
        // A system folder is too broad to stand for a game.
        assert!(!w.any_matching(&GameMatcher::Folder {
            install_dir: sleeper_program().parent().unwrap().to_path_buf(),
        }));
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert!(w.any_matching(&GameMatcher::ProcessName(name.clone())));
        assert!(!w.any_matching(&GameMatcher::ProcessName(
            "definitely-not-running-xyz".into()
        )));
        let mut ignoring = ProcessWatcher::new(std::slice::from_ref(&name));
        ignoring.refresh();
        assert!(!ignoring.any_matching(&GameMatcher::ProcessName(name)));
        child.terminate().unwrap();
        // Let it go before the folder is deleted.
        for _ in 0..50 {
            if !child.is_alive() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    #[cfg(windows)]
    fn sleeper_program() -> PathBuf {
        let sys = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(sys).join("System32").join("PING.EXE")
    }

    #[cfg(windows)]
    fn sleeper_args() -> Vec<String> {
        vec!["-n".into(), "30".into(), "127.0.0.1".into()]
    }

    #[cfg(unix)]
    fn sleeper_program() -> PathBuf {
        PathBuf::from("/bin/sleep")
    }

    #[cfg(unix)]
    fn sleeper_args() -> Vec<String> {
        vec!["30".into()]
    }

    fn spawn_sleeper() -> TrackedChild {
        TrackedChild::spawn(&sleeper_program(), &sleeper_args(), None).unwrap()
    }
}
