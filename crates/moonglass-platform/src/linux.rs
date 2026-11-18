//! Linux implementation of the platform layer (X11 and Wayland sessions).

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use rustix::process::{kill_process_group, test_kill_process_group, Pid, Signal};

use crate::{
    name_matches, within_normalized, AppPaths, EpicSource, GogSource, PlatformError, Prepared,
    ProcessInfo, Result,
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

pub fn flatpak_app_installed(app_id: &str) -> bool {
    home().is_ok_and(|h| flatpak_installed(app_id, &h))
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

/// Heroic's config folder and the command that runs it: the native
/// install first, then the Flatpak.
fn heroic() -> Option<(PathBuf, Vec<String>)> {
    let home = home().ok()?;
    let config = directories::BaseDirs::new()?.config_dir().to_path_buf();
    let native_cmd = on_path("heroic")
        .or_else(|| Some(PathBuf::from("/opt/Heroic/heroic")).filter(|p| p.is_file()));
    if let Some(cmd) = native_cmd {
        return Some((
            config.join("heroic"),
            vec![cmd.to_string_lossy().into_owned()],
        ));
    }
    if flatpak_installed(HEROIC_FLATPAK_ID, &home) {
        return Some((
            home.join(".var/app")
                .join(HEROIC_FLATPAK_ID)
                .join("config/heroic"),
            vec!["flatpak".into(), "run".into(), HEROIC_FLATPAK_ID.into()],
        ));
    }
    None
}

pub fn epic_source() -> Option<EpicSource> {
    let (config, command) = heroic()?;
    Some(EpicSource::Heroic {
        installed_json: config.join("legendaryConfig/legendary/installed.json"),
        command,
    })
}

pub fn gog_source() -> Option<GogSource> {
    let (config, command) = heroic()?;
    Some(GogSource::Heroic {
        installed_json: config.join("gog_store/installed.json"),
        command,
    })
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

/// Asks Heroic to start a game. `--no-gui` keeps Heroic's window from
/// appearing, which Moonglass could not close on Wayland.
fn launch_heroic(command: &[String], app_name: &str, runner: &str) -> Result<()> {
    let (program, pre) = command
        .split_first()
        .ok_or_else(|| PlatformError::Os("empty Heroic command".into()))?;
    spawn_detached(Command::new(program).args(pre).arg("--no-gui").arg(format!(
        "heroic://launch?appName={app_name}&runner={runner}"
    )))
}

pub fn launch_epic(source: &EpicSource, app_name: &str, _fallback_uri: Option<&str>) -> Result<()> {
    match source {
        EpicSource::Heroic { command, .. } => launch_heroic(command, app_name, "legendary"),
        EpicSource::EpicManifests(_) => Err(PlatformError::Os(
            "the Epic Games Launcher is not available on Linux".into(),
        )),
    }
}

pub fn launch_gog(source: &GogSource, product_id: &str, _install_dir: &Path) -> Result<()> {
    match source {
        GogSource::Heroic { command, .. } => launch_heroic(command, product_id, "gog"),
        GogSource::Registry { .. } => Err(PlatformError::Os(
            "GOG Galaxy is not available on Linux".into(),
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
pub const STEAM_PROCESS: &str = "steam";

/// Steam's `registry.vdf`, which holds on Linux what the registry holds
/// on Windows.
fn steam_registry() -> Option<PathBuf> {
    let home = home().ok()?;
    [
        home.join(".steam/registry.vdf"),
        home.join(".var/app")
            .join(STEAM_FLATPAK_ID)
            .join(".steam/registry.vdf"),
        home.join("snap/steam/common/.steam/registry.vdf"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// The quoted value after `"key"` in a VDF text, with its byte range.
fn vdf_value(text: &str, key: &str) -> Option<(usize, usize)> {
    let needle = format!("\"{key}\"");
    let at = text.find(&needle)? + needle.len();
    let open = at + text[at..].find('"')? + 1;
    let close = open + text[open..].find('"')?;
    // Only whitespace between the key and its value.
    text[at..open - 1]
        .trim()
        .is_empty()
        .then_some((open, close))
}

pub fn steam_active_account() -> Option<u32> {
    let text = std::fs::read_to_string(steam_registry()?).ok()?;
    let (a, b) = vdf_value(&text, "ActiveUser")?;
    text[a..b].parse().ok()
}

/// The `steam` command for this install (Flatpak, Snap or native).
fn steam_command() -> Option<Command> {
    let home = home().ok()?;
    let root = steam_root();
    let under = |p: PathBuf| root.as_deref().is_some_and(|r| r.starts_with(&p));
    if under(home.join(".var/app").join(STEAM_FLATPAK_ID)) {
        let mut c = Command::new("flatpak");
        c.args(["run", STEAM_FLATPAK_ID]);
        Some(c)
    } else if under(home.join("snap")) {
        let mut c = Command::new("snap");
        c.args(["run", "steam"]);
        Some(c)
    } else {
        on_path("steam").map(Command::new)
    }
}

pub fn run_steam(args: &[&str]) -> Result<()> {
    let mut c = steam_command()
        .ok_or_else(|| PlatformError::Os("the steam command was not found".into()))?;
    spawn_detached(c.args(args))
}

pub fn set_steam_auto_login(account_name: &str) -> Result<()> {
    let path = steam_registry()
        .ok_or_else(|| PlatformError::Os("Steam's registry.vdf was not found".into()))?;
    let mut text = std::fs::read_to_string(&path)?;
    let (a, b) = vdf_value(&text, "AutoLoginUser")
        .ok_or_else(|| PlatformError::Os("registry.vdf has no AutoLoginUser".into()))?;
    text.replace_range(a..b, account_name);
    std::fs::write(&path, text)?;
    Ok(())
}

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

// ------------------------------------------------------- Quick Menu

pub const CAPTURE_TOOLS: &[crate::CaptureTool] = &[crate::CaptureTool::GpuScreenRecorder];
/// Nothing ships with the desktop that can be asked to save a clip.
pub const CAPTURE_DEFAULT: Option<crate::CaptureTool> = None;

/// gpu-screen-recorder saves its replay buffer on `SIGUSR1`.
pub fn capture(tool: crate::CaptureTool, kind: crate::CaptureKind) -> Result<()> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    if tool != crate::CaptureTool::GpuScreenRecorder || kind != crate::CaptureKind::Clip {
        return Err(PlatformError::Os(format!(
            "{} is not available on Linux",
            tool.name()
        )));
    }
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut found = false;
    for p in sys.processes().values() {
        if p.thread_kind().is_some() || p.name() != "gpu-screen-recorder" {
            continue;
        }
        if let Some(pid) = Pid::from_raw(p.pid().as_u32() as i32) {
            found |= rustix::process::kill_process(pid, Signal::USR1).is_ok();
        }
    }
    if found {
        Ok(())
    } else {
        Err(PlatformError::Os(
            "gpu-screen-recorder isn't running. Start it in replay mode (-r) first.".into(),
        ))
    }
}

/// One PulseAudio / PipeWire playback stream, from `pactl list sink-inputs`.
#[derive(Debug, PartialEq)]
struct SinkInput {
    index: u32,
    pid: Option<u32>,
    /// The first channel's volume, as a fraction of 100%.
    level: f32,
    muted: bool,
}

fn parse_sink_inputs(text: &str) -> Vec<SinkInput> {
    let mut out = Vec::new();
    let mut cur: Option<SinkInput> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = t.strip_prefix("Sink Input #") {
            out.extend(cur.take());
            cur = n.trim().parse().ok().map(|index| SinkInput {
                index,
                pid: None,
                level: 1.0,
                muted: false,
            });
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if let Some(m) = t.strip_prefix("Mute:") {
            c.muted = m.trim() == "yes";
        } else if let Some(v) = t.strip_prefix("Volume:") {
            // "front-left: 65536 /  100% / 0.00 dB,   front-right: ..."
            if let Some(pct) = v
                .split('/')
                .nth(1)
                .and_then(|s| s.trim().trim_end_matches('%').parse::<f32>().ok())
            {
                c.level = pct / 100.0;
            }
        } else if let Some(p) = t.strip_prefix("application.process.id = ") {
            c.pid = p.trim_matches('"').parse().ok();
        }
    }
    out.extend(cur);
    out
}

fn game_sink_inputs(pids: &[u32]) -> Vec<SinkInput> {
    let out = Command::new("pactl")
        .args(["list", "sink-inputs"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => parse_sink_inputs(&String::from_utf8_lossy(&o.stdout))
            .into_iter()
            .filter(|s| s.pid.is_some_and(|p| pids.contains(&p)))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn game_volume(pids: &[u32]) -> Option<crate::GameVolume> {
    game_sink_inputs(pids)
        .into_iter()
        .map(|s| crate::GameVolume {
            level: s.level.min(1.0),
            muted: s.muted,
        })
        .max_by(|a, b| a.level.total_cmp(&b.level))
}

pub fn set_game_volume(pids: &[u32], v: crate::GameVolume) -> bool {
    let streams = game_sink_inputs(pids);
    let pactl = |args: &[&str]| {
        Command::new("pactl")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let level = format!("{}%", (v.level * 100.0).round() as u32);
    let mute = if v.muted { "1" } else { "0" };
    let mut any = false;
    for s in &streams {
        let i = s.index.to_string();
        any |= pactl(&["set-sink-input-volume", &i, &level]);
        pactl(&["set-sink-input-mute", &i, mute]);
    }
    any
}

#[cfg(test)]
mod quick_menu_tests {
    use super::*;

    #[test]
    fn sink_inputs_parse() {
        let text = "Sink Input #42\n\tDriver: PipeWire\n\tMute: no\n\tVolume: front-left: 52429 /  80% / -5.81 dB,   front-right: 52429 /  80% / -5.81 dB\n\tProperties:\n\t\tapplication.name = \"Game\"\n\t\tapplication.process.id = \"1234\"\nSink Input #43\n\tMute: yes\n\tVolume: mono: 65536 / 100% / 0.00 dB\n";
        assert_eq!(
            parse_sink_inputs(text),
            [
                SinkInput {
                    index: 42,
                    pid: Some(1234),
                    level: 0.8,
                    muted: false
                },
                SinkInput {
                    index: 43,
                    pid: None,
                    level: 1.0,
                    muted: true
                },
            ]
        );
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

    /// A desktop notification through `notify-send`, where it is installed.
    pub fn notify(_raw: isize, title: &str, body: &str) {
        let sent = std::process::Command::new("notify-send")
            .args(["--app-name=Moonglass", title, body])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Err(e) = sent {
            tracing::warn!(error = %e, "desktop notification unavailable");
        }
    }

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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Availability {
        pub gamepad: bool,
        pub keyboard: bool,
    }

    pub struct RestoreHotkey {
        availability: Availability,
    }

    impl RestoreHotkey {
        pub fn start(
            _config: std::sync::Arc<crate::RestoreConfig>,
            _on_restore: impl Fn() + Send + Sync + 'static,
        ) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_values() {
        let text = "\"Registry\"\n{\n\t\"AutoLoginUser\"\t\t\"old_name\"\n\t\"ActiveProcess\"\n\t{\n\t\t\"pid\"\t\t\"42\"\n\t\t\"ActiveUser\"\t\t\"1259678735\"\n\t}\n}\n";
        let (a, b) = vdf_value(text, "ActiveUser").unwrap();
        assert_eq!(&text[a..b], "1259678735");
        let (a, b) = vdf_value(text, "AutoLoginUser").unwrap();
        assert_eq!(&text[a..b], "old_name");
        // A key followed by a block has no value.
        assert_eq!(vdf_value(text, "ActiveProcess"), None);
        assert_eq!(vdf_value(text, "Missing"), None);
    }
}
