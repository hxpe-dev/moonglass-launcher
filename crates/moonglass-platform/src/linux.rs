//! Linux implementation of the platform layer (X11 and Wayland sessions).

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use rustix::process::{kill_process_group, test_kill_process_group, Pid, Signal};

use crate::{
    name_matches, within_normalized, AppPaths, EpicSource, PlatformError, Prepared, ProcessInfo,
    Result,
};

/// evdev delivers controller input without focus, so the gamepad thread
/// also watches for the restore combination.
pub const GAMEPAD_RESTORE_VIA_GILRS: bool = true;

pub const PLATFORM_NAME: &str = "Linux";

const STEAM_FLATPAK_ID: &str = "com.valvesoftware.Steam";
const HEROIC_FLATPAK_ID: &str = "com.heroicgameslauncher.hgl";

fn home() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or(PlatformError::NoHome)
}

// ------------------------------------------------------------------ paths

pub fn app_paths() -> Result<AppPaths> {
    // `directories` honors the XDG_* variables when they are set.
    let base = directories::BaseDirs::new().ok_or(PlatformError::NoHome)?;
    let state = base
        .state_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| base.home_dir().join(".local/state"));
    Ok(AppPaths {
        data_dir: base.data_dir().join("moonglass"),
        cache_dir: base.cache_dir().join("moonglass"),
        log_dir: state.join("moonglass"),
    })
}

/// Linux paths are case-sensitive; `canonicalize` already did the rest.
pub fn finish_normalize(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Folders that hold many unrelated programs (see
/// [`crate::is_trackable_folder`]).
pub fn broad_folders() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [
        "/usr/bin",
        "/usr/sbin",
        "/usr/lib",
        "/usr/libexec",
        "/usr/share",
        "/usr/local/bin",
        "/bin",
        "/sbin",
        "/lib",
        "/opt",
        "/etc",
        "/var",
        "/tmp",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(user) = directories::UserDirs::new() {
        out.push(user.home_dir().to_path_buf());
        out.extend(
            [user.desktop_dir(), user.document_dir(), user.download_dir()]
                .into_iter()
                .flatten()
                .map(Path::to_path_buf),
        );
    }
    out
}

/// Linux has no single system folder that games never live under; the
/// shared folders above cover it.
pub fn system_folders() -> Vec<PathBuf> {
    Vec::new()
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn flatpak_installed(app_id: &str, home: &Path) -> bool {
    [
        home.join(".local/share/flatpak/app").join(app_id),
        PathBuf::from("/var/lib/flatpak/app").join(app_id),
    ]
    .iter()
    .any(|p| p.is_dir())
}

// ------------------------------------------------------ launcher discovery

fn steam_candidates(home: &Path) -> [PathBuf; 4] {
    [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app")
            .join(STEAM_FLATPAK_ID)
            .join(".local/share/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
    ]
}

pub fn steam_root() -> Option<PathBuf> {
    let home = home().ok()?;
    steam_candidates(&home)
        .into_iter()
        .find(|p| p.join("steamapps").is_dir())
}

pub fn epic_source() -> Option<EpicSource> {
    let home = home().ok()?;
    let config = directories::BaseDirs::new()?.config_dir().to_path_buf();
    let legendary = "heroic/legendaryConfig/legendary/installed.json";

    let native_cmd = on_path("heroic")
        .or_else(|| Some(PathBuf::from("/opt/Heroic/heroic")).filter(|p| p.is_file()));
    if let Some(cmd) = native_cmd {
        return Some(EpicSource::Heroic {
            installed_json: config.join(legendary),
            command: vec![cmd.to_string_lossy().into_owned()],
        });
    }
    if flatpak_installed(HEROIC_FLATPAK_ID, &home) {
        return Some(EpicSource::Heroic {
            installed_json: home
                .join(".var/app")
                .join(HEROIC_FLATPAK_ID)
                .join("config")
                .join(legendary),
            command: vec!["flatpak".into(), "run".into(), HEROIC_FLATPAK_ID.into()],
        });
    }
    None
}

// ---------------------------------------------------------------- launching

/// Starts a helper program detached from Moonglass's lifecycle, reaping it in
/// the background so it never lingers as a zombie.
fn spawn_detached(cmd: &mut Command) -> Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn open_uri(uri: &str) -> Result<()> {
    spawn_detached(Command::new("xdg-open").arg(uri))
}

/// Uses Steam's own command when available so that `-silent` keeps Steam's
/// window in the tray if Moonglass is the one starting it (Moonglass cannot close
/// another app's window on Wayland). Falls back to `xdg-open`.
pub fn launch_steam(app_id: &str, args: &[String]) -> Result<()> {
    let uri = format!("steam://rungameid/{app_id}");
    // What Steam is asked to do: the URI, or `-applaunch` with the game's
    // launch options.
    let mut request = vec!["-silent".to_string()];
    if args.is_empty() {
        request.push(uri.clone());
    } else {
        request.extend(["-applaunch".to_string(), app_id.to_string()]);
        request.extend(args.iter().cloned());
    }
    let home = home()?;
    let root = steam_root();
    let is_flatpak = root
        .as_deref()
        .is_some_and(|r| r.starts_with(home.join(".var/app").join(STEAM_FLATPAK_ID)));
    let is_snap = root
        .as_deref()
        .is_some_and(|r| r.starts_with(home.join("snap")));

    if is_flatpak {
        spawn_detached(
            Command::new("flatpak")
                .args(["run", STEAM_FLATPAK_ID])
                .args(&request),
        )
    } else if is_snap {
        spawn_detached(Command::new("snap").args(["run", "steam"]).args(&request))
    } else if let Some(steam) = on_path("steam") {
        spawn_detached(Command::new(steam).args(&request))
    } else {
        if !args.is_empty() {
            tracing::warn!("the steam command was not found; starting without launch options");
        }
        open_uri(&uri)
    }
}

pub fn launch_epic(source: &EpicSource, app_name: &str, _fallback_uri: Option<&str>) -> Result<()> {
    match source {
        EpicSource::Heroic { command, .. } => {
            let (program, pre) = command
                .split_first()
                .ok_or_else(|| PlatformError::Os("empty Heroic command".into()))?;
            spawn_detached(
                Command::new(program)
                    .args(pre)
                    // `--no-gui` keeps Heroic's window from appearing.
                    .arg("--no-gui")
                    .arg(format!(
                        "heroic://launch?appName={app_name}&runner=legendary"
                    )),
            )
        }
        EpicSource::EpicManifests(_) => Err(PlatformError::Os(
            "the Epic Games Launcher is not available on Linux".into(),
        )),
    }
}

/// A child started in its own process group, so the group can be checked
/// and signalled as a whole.
pub struct Tracked {
    child: Child,
    pgid: Pid,
}

impl Tracked {
    pub fn spawn(program: &Path, args: &[String], cwd: Option<&Path>) -> Result<Self> {
        let mut cmd = Command::new(program);
        cmd.args(args).process_group(0).stdin(Stdio::null());
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        let child = cmd.spawn()?;
        let pgid = Pid::from_child(&child);
        Ok(Self { child, pgid })
    }

    pub fn is_alive(&mut self) -> bool {
        // Reap the leader so a zombie does not keep the group "alive".
        let _ = self.child.try_wait();
        test_kill_process_group(self.pgid).is_ok()
    }

    pub fn terminate(&mut self) -> Result<()> {
        match kill_process_group(self.pgid, Signal::TERM) {
            Ok(()) => {}
            // Already gone.
            Err(rustix::io::Errno::SRCH) => return Ok(()),
            Err(e) => return Err(std::io::Error::from(e).into()),
        }
        // Escalate if the game ignores SIGTERM.
        let pgid = self.pgid;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            if test_kill_process_group(pgid).is_ok() {
                let _ = kill_process_group(pgid, Signal::KILL);
            }
        });
        Ok(())
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

// --------------------------------------------------------- game detection

fn any_path_within(p: &ProcessInfo<'_>, dir: &str) -> bool {
    p.exe.is_some_and(|e| within_normalized(e, dir))
        || p.cwd.is_some_and(|c| within_normalized(c, dir))
        || p.cmd.iter().any(|a| {
            let a = Path::new(a);
            a.is_absolute() && within_normalized(a, dir)
        })
}

pub(crate) fn process_matches(p: &ProcessInfo<'_>, m: &Prepared<'_>) -> bool {
    match m {
        Prepared::Steam { app_id, dir } => {
            // Steam sets SteamAppId on the game's own processes, native or
            // Proton. Path matching is the fallback for sandboxed setups
            // that hide the environment.
            let wanted = format!("SteamAppId={app_id}");
            p.environ
                .iter()
                .any(|e| e.as_encoded_bytes() == wanted.as_bytes())
                || dir.as_deref().is_some_and(|d| any_path_within(p, d))
        }
        Prepared::Folder { dir } => dir.as_deref().is_some_and(|d| any_path_within(p, d)),
        Prepared::Name(n) => name_matches(p.name, n),
    }
}

pub fn browse_roots() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(home) = home() {
        out.push(("Home".to_string(), home));
    }
    out.push(("Computer".to_string(), PathBuf::from("/")));
    let user = std::env::var("USER").unwrap_or_default();
    for base in [
        PathBuf::from("/media").join(&user),
        PathBuf::from("/run/media").join(&user),
        PathBuf::from("/mnt"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    out.push((name, p));
                }
            }
        }
    }
    out
}

pub fn is_launchable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Steam on Linux has no registry; process detection is the only signal.
pub fn steam_app_running(_app_id: &str) -> Option<bool> {
    None
}

/// `gilrs` reports battery percentages on Linux itself.
pub fn xinput_power() -> Option<crate::PadPower> {
    None
}

/// SIGTERM to each process: the polite request to quit.
pub fn request_close(pids: &[u32]) {
    for pid in pids {
        if let Some(p) = Pid::from_raw(*pid as i32) {
            let _ = rustix::process::kill_process(p, Signal::TERM);
        }
    }
}

// ------------------------------------------------------- start at login

fn autostart_file() -> Result<PathBuf> {
    let config = directories::BaseDirs::new()
        .ok_or(PlatformError::NoHome)?
        .config_dir()
        .to_path_buf();
    Ok(config.join("autostart").join("moonglass.desktop"))
}

pub fn start_at_login() -> Result<bool> {
    Ok(autostart_file()?.is_file())
}

pub fn set_start_at_login(enabled: bool) -> Result<()> {
    let file = autostart_file()?;
    if !enabled {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    // Inside an AppImage, current_exe is a temporary mount; $APPIMAGE is
    // the stable file.
    let exe = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .map_or_else(std::env::current_exe, Ok)?;
    let exec = exe
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Moonglass\nComment=Console-style game launcher\nExec=\"{exec}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    );
    std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(&file, entry)?;
    Ok(())
}

// ------------------------------------------------------- single instance

/// The per-user runtime directory, or a per-user folder in the temporary
/// directory (which is shared between users) when there is none.
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| {
            let uid = rustix::process::getuid().as_raw();
            std::env::temp_dir().join(format!("moonglass-{uid}"))
        })
}

/// Lock file plus a Unix socket in the runtime directory. The second
/// instance connects and writes `activate`.
pub struct Instance {
    _lock: std::fs::File,
    listener: UnixListener,
}

impl Instance {
    pub fn acquire() -> Result<Option<Self>> {
        use rustix::fs::{flock, FlockOperation};
        let dir = runtime_dir();
        std::fs::create_dir_all(&dir)?;
        let lock_path = dir.join("moonglass.lock");
        let sock_path = dir.join("moonglass.sock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        if flock(&lock, FlockOperation::NonBlockingLockExclusive).is_err() {
            if let Ok(mut s) = UnixStream::connect(&sock_path) {
                let _ = s.write_all(b"activate");
            }
            return Ok(None);
        }
        // We hold the lock, so any socket file left behind is stale.
        let _ = std::fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path)?;
        listener.set_nonblocking(true)?;
        Ok(Some(Self {
            _lock: lock,
            listener,
        }))
    }

    pub fn take_activation_request(&self) -> bool {
        let mut requested = false;
        while let Ok((mut s, _)) = self.listener.accept() {
            let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
            let mut buf = [0u8; 16];
            if let Ok(n) = s.read(&mut buf) {
                requested |= &buf[..n] == b"activate";
            }
        }
        requested
    }
}

// ------------------------------------------------------------------ window

/// Window behavior on Linux (spec 4.3, 5.3). Moonglass never minimizes: it
/// stays fullscreen behind the game and renders nothing, so that when the
/// game's window closes the compositor hands focus back to it. Activation
/// requests go through the UI toolkit (`_NET_ACTIVE_WINDOW` on X11, the
/// activation protocol on Wayland), not through this crate.
pub mod window {
    use crate::FocusResult;
    pub use crate::Launcher;

    /// Windows only for now: nothing is shown before the real window.
    pub struct Splash;

    impl Splash {
        pub fn show() -> Option<Self> {
            None
        }
    }

    /// No-op: Moonglass stays fullscreen underneath the game.
    pub fn hide_for_game(_raw: isize) {}

    /// Focus is requested through the toolkit on Linux.
    pub fn regain_focus(_raw: isize) -> FocusResult {
        FocusResult::UseToolkit
    }

    /// Launcher windows are kept from appearing instead (`heroic --no-gui`,
    /// `steam -silent`), because Wayland gives no way to close another
    /// application's window. The X11 close request is not implemented yet.
    pub fn dismiss_launcher_window(launcher: Launcher) -> usize {
        tracing::debug!(
            ?launcher,
            "launcher window dismissal relies on --no-gui / -silent on Linux"
        );
        0
    }
}

// ------------------------------------------------------------------ hotkey

/// On Linux the gamepad restore combination is read by the same `gilrs`
/// thread that handles navigation (evdev does not depend on focus), so this
/// module only reports keyboard availability. The X11 grab and the Wayland
/// global-shortcuts portal are not implemented yet; until they are, only the
/// gamepad combination is offered and Settings says so.
pub mod hotkey {
    use std::time::Duration;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Availability {
        pub gamepad: bool,
        pub keyboard: bool,
    }

    pub struct RestoreHotkey {
        availability: Availability,
    }

    impl RestoreHotkey {
        pub fn start(_hold: Duration, _on_restore: impl Fn() + Send + Sync + 'static) -> Self {
            Self {
                availability: Availability {
                    gamepad: true,
                    keyboard: false,
                },
            }
        }

        pub fn set_armed(&self, _armed: bool) {}

        pub fn availability(&self) -> Availability {
            self.availability
        }
    }
}
