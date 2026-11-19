//! Windows implementation of the platform layer.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE,
    WAIT_OBJECT_0,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    QueryInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};
use windows::Win32::UI::Shell::{SHLoadIndirectString, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::{
    name_matches, within_normalized, AppPaths, BattleNetEntry, BattleNetSource, EpicSource,
    GogRegistryGame, GogSource, PlatformError, Prepared, ProcessInfo, Result, StorePackage,
};

/// Windows reads the background restore combination with its own XInput
/// poller, so the gamepad thread does not need to.
pub const GAMEPAD_RESTORE_VIA_GILRS: bool = false;

pub const PLATFORM_NAME: &str = "Windows";

pub const WINDOWS_STORES: bool = true;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn last_error(what: &str) -> PlatformError {
    PlatformError::Os(format!("{what}: {}", windows::core::Error::from_win32()))
}

// ------------------------------------------------------------------ paths

pub fn app_paths() -> Result<AppPaths> {
    let base = directories::BaseDirs::new().ok_or(PlatformError::NoHome)?;
    // %APPDATA%\Moonglass and %LOCALAPPDATA%\Moonglass (spec 4.4).
    let local = base.data_local_dir().join("Moonglass");
    Ok(AppPaths {
        data_dir: base.config_dir().join("Moonglass"),
        cache_dir: local.join("cache"),
        log_dir: local.join("logs"),
    })
}

/// Strips the `\\?\` verbatim prefix `canonicalize` adds, unifies
/// separators and lowercases (Windows paths are case-insensitive).
pub fn finish_normalize(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    };
    s.to_lowercase()
}

/// Folders that hold many unrelated programs (see
/// [`crate::is_trackable_folder`]).
pub fn broad_folders() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
        "LOCALAPPDATA",
        "APPDATA",
        "PUBLIC",
    ]
    .iter()
    .filter_map(std::env::var_os)
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

/// Windows itself: nothing inside it is a game folder.
pub fn system_folders() -> Vec<PathBuf> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    vec![PathBuf::from(root)]
}

// ---------------------------------------------------------------- registry

fn reg_get_string(subkey: &str, value: &str) -> Option<String> {
    reg_get_string_in(HKEY_CURRENT_USER, subkey, value)
}

fn reg_get_string_in(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);
    let mut len: u32 = 0;
    unsafe {
        let rc = RegGetValueW(
            root,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut len),
        );
        if rc != ERROR_SUCCESS || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        let rc = RegGetValueW(
            root,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        );
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

/// Names of a registry key's subkeys.
fn reg_subkeys(root: HKEY, subkey: &str) -> Vec<String> {
    let subkey = HSTRING::from(subkey);
    let mut key = HKEY::default();
    let mut out = Vec::new();
    unsafe {
        if RegOpenKeyExW(root, &subkey, None, KEY_READ, &mut key) != ERROR_SUCCESS {
            return out;
        }
        for i in 0.. {
            // Registry key names are at most 255 characters.
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            let rc = RegEnumKeyExW(
                key,
                i,
                Some(windows::core::PWSTR(buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            );
            if rc != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        let _ = RegCloseKey(key);
    }
    out
}

// ------------------------------------------------------ launcher discovery

pub fn steam_root() -> Option<PathBuf> {
    let p = PathBuf::from(reg_get_string(r"Software\Valve\Steam", "SteamPath")?);
    p.is_dir().then_some(p)
}

pub fn epic_source() -> Option<EpicSource> {
    let program_data = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
    let dir = PathBuf::from(program_data).join(r"Epic\EpicGamesLauncher\Data\Manifests");
    dir.is_dir().then_some(EpicSource::EpicManifests(dir))
}

const GOG_KEY: &str = r"SOFTWARE\WOW6432Node\GOG.com";

/// GOG Galaxy's program: from its registry key, else its default folder.
fn gog_galaxy() -> Option<PathBuf> {
    reg_get_string_in(
        HKEY_LOCAL_MACHINE,
        &format!(r"{GOG_KEY}\GalaxyClient\paths"),
        "client",
    )
    .map(|dir| PathBuf::from(dir).join("GalaxyClient.exe"))
    .filter(|p| p.is_file())
    .or_else(|| {
        let pf = std::env::var_os("ProgramFiles(x86)")
            .unwrap_or_else(|| r"C:\Program Files (x86)".into());
        Some(PathBuf::from(pf).join(r"GOG Galaxy\GalaxyClient.exe")).filter(|p| p.is_file())
    })
}

pub fn gog_source() -> Option<GogSource> {
    let games_key = format!(r"{GOG_KEY}\Games");
    let games: Vec<GogRegistryGame> = reg_subkeys(HKEY_LOCAL_MACHINE, &games_key)
        .into_iter()
        .map(|id| {
            let key = format!(r"{games_key}\{id}");
            let get = |v: &str| reg_get_string_in(HKEY_LOCAL_MACHINE, &key, v).unwrap_or_default();
            GogRegistryGame {
                name: get("gameName"),
                path: get("path"),
                exe: get("exe"),
                depends_on: get("dependsOn"),
                product_id: id,
            }
        })
        .collect();
    let galaxy = gog_galaxy();
    (!games.is_empty() || galaxy.is_some()).then_some(GogSource::Registry { games, galaxy })
}

const UNINSTALL_KEYS: [&str; 2] = [
    r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
];

/// `Battle.net.exe`: from Battle.net's registered capabilities, else its
/// default folder.
fn battlenet_client() -> Option<PathBuf> {
    reg_get_string_in(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Blizzard Entertainment\Battle.net\Capabilities",
        "ApplicationIcon",
    )
    // `"C:\…\Battle.net.exe",0`
    .and_then(|v| {
        let v = v.trim().trim_start_matches('"');
        let end = v.to_ascii_lowercase().find(".exe")? + 4;
        Some(PathBuf::from(&v[..end]))
    })
    .filter(|p| p.is_file())
    .or_else(|| {
        let pf = std::env::var_os("ProgramFiles(x86)")
            .unwrap_or_else(|| r"C:\Program Files (x86)".into());
        Some(PathBuf::from(pf).join(r"Battle.net\Battle.net.exe")).filter(|p| p.is_file())
    })
}

pub fn battlenet_source() -> Option<BattleNetSource> {
    let games: Vec<BattleNetEntry> = UNINSTALL_KEYS
        .iter()
        .flat_map(|root| {
            reg_subkeys(HKEY_LOCAL_MACHINE, root)
                .into_iter()
                .map(move |k| format!(r"{root}\{k}"))
        })
        .filter_map(|key| {
            let get = |v: &str| reg_get_string_in(HKEY_LOCAL_MACHINE, &key, v).unwrap_or_default();
            let uninstall_string = get("UninstallString");
            // Only Battle.net's own uninstaller carries a product UID.
            (uninstall_string.contains("Blizzard Uninstaller")
                && uninstall_string.contains("--uid="))
            .then(|| BattleNetEntry {
                name: get("DisplayName"),
                install_location: get("InstallLocation"),
                uninstall_string,
            })
        })
        .collect();
    let client = battlenet_client();
    (!games.is_empty() || client.is_some()).then_some(BattleNetSource { games, client })
}

/// Resolves an `@{Package?ms-resource://…}` display name. Plain names are
/// returned as they are; one that cannot be resolved becomes empty.
fn resolve_indirect(s: &str) -> String {
    if !s.starts_with('@') {
        return s.to_string();
    }
    let source = HSTRING::from(s);
    let mut buf = [0u16; 512];
    match unsafe { SHLoadIndirectString(&source, &mut buf, None) } {
        Ok(()) => {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..end])
        }
        Err(_) => String::new(),
    }
}

/// The packages registered for the current user, from the AppX
/// repository in the registry: their full name, display name and folder.
pub fn msstore_packages() -> Option<Vec<StorePackage>> {
    const REPO: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
    Some(
        reg_subkeys(HKEY_CURRENT_USER, REPO)
            .into_iter()
            .filter_map(|full_name| {
                let key = format!(r"{REPO}\{full_name}");
                let root = reg_get_string_in(HKEY_CURRENT_USER, &key, "PackageRootFolder")?;
                if !Path::new(&root).is_dir() {
                    return None;
                }
                let display_name = reg_get_string_in(HKEY_CURRENT_USER, &key, "DisplayName")
                    .map(|n| resolve_indirect(&n))
                    .unwrap_or_default();
                Some(StorePackage {
                    full_name,
                    display_name,
                    root,
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------- launching

pub fn open_uri(uri: &str) -> Result<()> {
    let uri = HSTRING::from(uri);
    let rc = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &uri,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW reports success as a value greater than 32.
    if rc.0 as isize > 32 {
        Ok(())
    } else {
        Err(PlatformError::Os(format!(
            "ShellExecuteW failed with code {}",
            rc.0 as isize
        )))
    }
}

fn steam_exe() -> Option<PathBuf> {
    reg_get_string(r"Software\Valve\Steam", "SteamExe")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            steam_root()
                .map(|r| r.join("steam.exe"))
                .filter(|p| p.is_file())
        })
}

pub fn launch_steam(app_id: &str, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return open_uri(&format!("steam://rungameid/{app_id}"));
    }
    let exe = steam_exe().ok_or_else(|| {
        PlatformError::Os("steam.exe was not found, and launch options need it".into())
    })?;
    // Steam is already running: this hands the request over and exits.
    Command::new(exe)
        .arg("-applaunch")
        .arg(app_id)
        .args(args)
        .spawn()?;
    Ok(())
}

pub fn launch_epic(source: &EpicSource, app_name: &str, fallback_uri: Option<&str>) -> Result<()> {
    match source {
        EpicSource::EpicManifests(_) => match fallback_uri {
            Some(uri) => open_uri(uri),
            None => open_uri(&format!(
                "com.epicgames.launcher://apps/{app_name}?action=launch&silent=true"
            )),
        },
        EpicSource::Heroic { .. } => Err(PlatformError::Os("Heroic is not used on Windows".into())),
    }
}

pub fn launch_gog(source: &GogSource, product_id: &str, install_dir: &Path) -> Result<()> {
    match source {
        GogSource::Registry {
            galaxy: Some(galaxy),
            ..
        } => {
            // Galaxy is usually running already: this hands the request
            // over and exits.
            Command::new(galaxy)
                .arg("/command=runGame")
                .arg(format!("/gameId={product_id}"))
                .arg(format!("/path={}", install_dir.display()))
                .spawn()?;
            Ok(())
        }
        GogSource::Registry { galaxy: None, .. } => {
            Err(PlatformError::Os("GOG Galaxy is not installed".into()))
        }
        GogSource::Heroic { .. } => Err(PlatformError::Os("Heroic is not used on Windows".into())),
    }
}

pub fn launch_battlenet(client: &Path, code: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    // Battle.net is usually running already: this hands the request over
    // and exits. The quotes are Battle.net's own syntax; `code` was checked
    // to be letters and digits.
    Command::new(client)
        .raw_arg(format!("--exec=\"launch {code}\""))
        .spawn()?;
    Ok(())
}

pub fn launch_msstore(aumid: &str) -> Result<()> {
    open_uri(&format!(r"shell:AppsFolder\{aumid}"))
}

/// A child process inside a Job Object. The job is not kill-on-close, so a
/// crash of Moonglass never takes the game down with it.
pub struct Tracked {
    child: Child,
    job: HANDLE,
    /// False when the child could not be placed in the job; only the direct
    /// child is watched and terminated then.
    in_job: bool,
}

// The job handle is only used from the owning thread at a time.
unsafe impl Send for Tracked {}

impl Tracked {
    pub fn spawn(program: &Path, args: &[String], cwd: Option<&Path>) -> Result<Self> {
        let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|e| PlatformError::Os(format!("CreateJobObjectW: {e}")))?;
        let mut cmd = Command::new(program);
        cmd.args(args);
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                unsafe { CloseHandle(job).ok() };
                return Err(e.into());
            }
        };
        let handle = HANDLE(child.as_raw_handle());
        let in_job = match unsafe { AssignProcessToJobObject(job, handle) } {
            Ok(()) => true,
            Err(e) => {
                // Still usable: fall back to watching the direct child only.
                tracing::warn!(error = %e, "could not place game in a Job Object");
                false
            }
        };
        Ok(Self { child, job, in_job })
    }

    fn active_processes(&self) -> Option<u32> {
        if !self.in_job {
            return None;
        }
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.job),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )
        }
        .ok()?;
        Some(info.ActiveProcesses)
    }

    pub fn is_alive(&mut self) -> bool {
        // Reap the direct child either way so its handle state is current.
        let direct_alive = matches!(self.child.try_wait(), Ok(None));
        match self.active_processes() {
            // The job counts the child and everything it started.
            Some(n) => n > 0,
            None => direct_alive,
        }
    }

    pub fn terminate(&mut self) -> Result<()> {
        let job = if self.in_job {
            unsafe { TerminateJobObject(self.job, 1) }
        } else {
            Err(windows::core::Error::empty())
        };
        job.or_else(|_| self.child.kill().map_err(windows::core::Error::from))
            .map_err(|e| PlatformError::Os(format!("terminate: {e}")))
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.job).ok() };
    }
}

// --------------------------------------------------------- game detection

pub(crate) fn process_matches(p: &ProcessInfo<'_>, m: &Prepared<'_>) -> bool {
    match m {
        // Windows has no reliable view of another process's environment, so
        // Steam and Epic games are both found by their executable's folder.
        Prepared::Steam { dir, .. } | Prepared::Folder { dir } => match (dir, p.exe) {
            (Some(dir), Some(exe)) => {
                // A program started through an 8.3 short path
                // (`C:\Users\RUNNER~1\…`) reports that path, while `dir` is
                // in its long form. Short names are rare, so only those
                // paths pay for the lookup.
                //
                // A Store game's folder under `WindowsApps` is often a
                // junction to `C:\XboxGames\<Game>\Content`: `dir` is
                // resolved, and the process may report either path.
                if within_normalized(exe, dir) {
                    return true;
                }
                let s = exe.as_os_str().to_string_lossy();
                (s.contains('~') || s.to_ascii_lowercase().contains(r"\windowsapps\"))
                    && std::fs::canonicalize(exe).is_ok_and(|long| within_normalized(&long, dir))
            }
            _ => false,
        },
        Prepared::Name(n) => name_matches(p.name, n),
    }
}

pub fn browse_roots() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
        out.push(("Home".to_string(), home));
    }
    for letter in b'A'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", letter as char));
        if root.is_dir() {
            out.push((format!("Drive {}:", letter as char), root));
        }
    }
    out
}

pub fn is_launchable(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

fn reg_get_dword(subkey: &str, value: &str) -> Option<u32> {
    use windows::Win32::System::Registry::RRF_RT_REG_DWORD;
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);
    let mut data: u32 = 0;
    let mut len: u32 = 4;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &subkey,
            &value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut len),
        )
    };
    (rc == ERROR_SUCCESS).then_some(data)
}

/// Flatpak is Linux only.
pub fn flatpak_app_installed(_app_id: &str) -> bool {
    false
}

pub const STEAM_PROCESS: &str = "steam.exe";

pub fn steam_active_account() -> Option<u32> {
    reg_get_dword(r"Software\Valve\Steam\ActiveProcess", "ActiveUser")
}

/// Runs `steam.exe` with `args`: a running Steam takes the request over.
pub fn run_steam(args: &[&str]) -> Result<()> {
    let exe = steam_exe().ok_or_else(|| PlatformError::Os("steam.exe was not found".into()))?;
    // A Steam started here outlives Moonglass: it must not hold on to
    // Moonglass's console or pipes.
    Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// The account Steam signs in with when it starts.
pub fn set_steam_auto_login(account_name: &str) -> Result<()> {
    let key = HSTRING::from(r"Software\Valve\Steam");
    let name = HSTRING::from("AutoLoginUser");
    let data = wide(OsStr::new(account_name));
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(PlatformError::Os(format!("registry write failed: {rc:?}")))
    }
}

pub fn steam_app_running(app_id: &str) -> Option<bool> {
    reg_get_dword(&format!(r"Software\Valve\Steam\Apps\{app_id}"), "Running").map(|v| v != 0)
}

pub fn xinput_power() -> Option<crate::PadPower> {
    use windows::Win32::UI::Input::XboxController::{
        XInputGetBatteryInformation, XInputGetState, BATTERY_DEVTYPE_GAMEPAD,
        BATTERY_TYPE_DISCONNECTED, BATTERY_TYPE_UNKNOWN, BATTERY_TYPE_WIRED,
        XINPUT_BATTERY_INFORMATION, XINPUT_STATE,
    };
    for i in 0..4 {
        let mut st = XINPUT_STATE::default();
        if unsafe { XInputGetState(i, &mut st) } != 0 {
            continue;
        }
        let mut info = XINPUT_BATTERY_INFORMATION::default();
        if unsafe { XInputGetBatteryInformation(i, BATTERY_DEVTYPE_GAMEPAD, &mut info) } != 0 {
            continue;
        }
        return match info.BatteryType {
            BATTERY_TYPE_WIRED => Some(crate::PadPower::Wired),
            BATTERY_TYPE_DISCONNECTED | BATTERY_TYPE_UNKNOWN => None,
            _ => Some(crate::PadPower::Level(info.BatteryLevel.0.min(3))),
        };
    }
    None
}

// ------------------------------------------------------- Quick Menu

pub const CAPTURE_TOOLS: &[crate::CaptureTool] = &[
    crate::CaptureTool::GameBar,
    crate::CaptureTool::Nvidia,
    crate::CaptureTool::Amd,
];
pub const CAPTURE_DEFAULT: Option<crate::CaptureTool> = Some(crate::CaptureTool::GameBar);

/// Presses the tool's default shortcut. The tools register these as
/// system-wide hotkeys and act on the game in front.
pub fn capture(tool: crate::CaptureTool, kind: crate::CaptureKind) -> Result<()> {
    use crate::{CaptureKind as K, CaptureTool as T};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_F1, VK_F10, VK_LWIN, VK_MENU, VK_SHIFT,
        VK_SNAPSHOT,
    };
    let chord: &[VIRTUAL_KEY] = match (tool, kind) {
        (T::GameBar, K::Clip) => &[VK_LWIN, VK_MENU, VIRTUAL_KEY(b'G' as u16)],
        (T::GameBar, K::Screenshot) => &[VK_LWIN, VK_MENU, VK_SNAPSHOT],
        (T::Nvidia, K::Clip) => &[VK_MENU, VK_F10],
        (T::Nvidia, K::Screenshot) => &[VK_MENU, VK_F1],
        (T::Amd, K::Clip) => &[VK_CONTROL, VK_SHIFT, VIRTUAL_KEY(b'S' as u16)],
        (T::Amd, K::Screenshot) => &[VK_CONTROL, VK_SHIFT, VIRTUAL_KEY(b'I' as u16)],
        (T::GpuScreenRecorder, _) => {
            return Err(PlatformError::Os(
                "gpu-screen-recorder is not available on Windows".into(),
            ))
        }
    };
    let key = |vk: VIRTUAL_KEY, up: bool| {
        let mut flags = if up {
            KEYEVENTF_KEYUP
        } else {
            Default::default()
        };
        // Print Screen and the Windows key are extended keys.
        if vk == VK_SNAPSHOT || vk == VK_LWIN {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        }
    };
    // Every key down in order, then up in reverse.
    let inputs: Vec<INPUT> = chord
        .iter()
        .map(|&vk| key(vk, false))
        .chain(chord.iter().rev().map(|&vk| key(vk, true)))
        .collect();
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        // Windows drops synthetic input aimed at a program running as
        // administrator when Moonglass is not.
        Err(PlatformError::Os(format!(
            "Windows did not pass the {} shortcut on",
            tool.name()
        )))
    }
}

/// Runs `f` on the volume control of every audio session, on every active
/// output device, that belongs to one of `pids`. Returns how many it ran on.
fn for_game_sessions(
    pids: &[u32],
    mut f: impl FnMut(&windows::Win32::Media::Audio::ISimpleAudioVolume),
) -> usize {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    // Balanced with the uninitialize below when this call initialized COM
    // on the thread (S_OK or S_FALSE); a thread already in another mode
    // can still use it.
    let inited = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let mut n = 0;
    let mut run = || -> windows::core::Result<()> {
        unsafe {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let list = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for d in 0..list.GetCount()? {
                let Ok(manager) = list
                    .Item(d)
                    .and_then(|dev| dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None))
                else {
                    continue;
                };
                let sessions = manager.GetSessionEnumerator()?;
                for s in 0..sessions.GetCount()? {
                    let Ok(control) = sessions.GetSession(s) else {
                        continue;
                    };
                    if control
                        .GetState()
                        .is_ok_and(|st| st == AudioSessionStateExpired)
                    {
                        continue;
                    }
                    let Ok(c2) = control.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    if !c2.GetProcessId().is_ok_and(|pid| pids.contains(&pid)) {
                        continue;
                    }
                    if let Ok(v) = control.cast::<ISimpleAudioVolume>() {
                        f(&v);
                        n += 1;
                    }
                }
            }
        }
        Ok(())
    };
    if let Err(e) = run() {
        tracing::debug!("audio sessions: {e}");
    }
    if inited {
        unsafe { CoUninitialize() };
    }
    n
}

pub fn game_volume(pids: &[u32]) -> Option<crate::GameVolume> {
    // The loudest stream stands for the game; its streams normally agree.
    let mut out: Option<crate::GameVolume> = None;
    for_game_sessions(pids, |v| unsafe {
        let (Ok(level), Ok(muted)) = (v.GetMasterVolume(), v.GetMute()) else {
            return;
        };
        if out.is_none_or(|o| level > o.level) {
            out = Some(crate::GameVolume {
                level,
                muted: muted.as_bool(),
            });
        }
    });
    out
}

pub fn set_game_volume(pids: &[u32], vol: crate::GameVolume) -> bool {
    let n = for_game_sessions(pids, |v| unsafe {
        let _ = v.SetMasterVolume(vol.level, std::ptr::null());
        let _ = v.SetMute(vol.muted, std::ptr::null());
    });
    n > 0
}

/// Posts a close request to every visible top-level window owned by one
/// of `pids` (the same as clicking the window's close button).
pub fn request_close(pids: &[u32]) {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    };
    unsafe extern "system" fn visit(h: HWND, lparam: LPARAM) -> BOOL {
        let pids = &*(lparam.0 as *const Vec<u32>);
        if IsWindowVisible(h).as_bool() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pids.contains(&pid) {
                let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        true.into()
    }
    let list: Vec<u32> = pids.to_vec();
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&list as *const Vec<u32> as isize));
    }
}

// ------------------------------------------------------- start at login

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Moonglass";

pub fn start_at_login() -> Result<bool> {
    Ok(reg_get_string(RUN_KEY, RUN_VALUE).is_some())
}

pub fn set_start_at_login(enabled: bool) -> Result<()> {
    let key = HSTRING::from(RUN_KEY);
    let name = HSTRING::from(RUN_VALUE);
    let rc = if enabled {
        let exe = std::env::current_exe()?;
        let data = wide(OsStr::new(&format!("\"{}\"", exe.display())));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
        }
    } else {
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name) };
        if rc == ERROR_FILE_NOT_FOUND {
            ERROR_SUCCESS
        } else {
            rc
        }
    };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(PlatformError::Os(format!("registry write failed: {rc:?}")))
    }
}

// ------------------------------------------------------- single instance

const MUTEX_NAME: PCWSTR = w!("Local\\Moonglass.SingleInstance");
const ACTIVATE_EVENT: PCWSTR = w!("Local\\Moonglass.Activate");

/// Named mutex plus a named auto-reset event the second instance sets.
pub struct Instance {
    mutex: HANDLE,
    activate: HANDLE,
}

unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}

impl Instance {
    pub fn acquire() -> Result<Option<Self>> {
        unsafe {
            let mutex = CreateMutexW(None, true, MUTEX_NAME)
                .map_err(|e| PlatformError::Os(format!("CreateMutexW: {e}")))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex).ok();
                if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, ACTIVATE_EVENT) {
                    SetEvent(ev).ok();
                    CloseHandle(ev).ok();
                }
                return Ok(None);
            }
            let activate = CreateEventW(None, false, false, ACTIVATE_EVENT)
                .map_err(|_| last_error("CreateEventW"))?;
            Ok(Some(Self { mutex, activate }))
        }
    }

    pub fn take_activation_request(&self) -> bool {
        unsafe { WaitForSingleObject(self.activate, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.activate).ok();
            CloseHandle(self.mutex).ok();
        }
    }
}

// ------------------------------------------------------------------ window

/// Hiding Moonglass, regaining focus and dismissing launcher windows
/// (spec 5.3). Window handles are raw `HWND` values as exposed by the UI
/// toolkit's raw-window-handle.
pub mod window {
    use std::time::Duration;

    use windows::core::{w, BOOL};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetStockObject, UpdateWindow, BLACK_BRUSH, HBRUSH};
    use windows::Win32::System::Threading::{
        AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_MENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, EnumWindows,
        FlashWindowEx, GetForegroundWindow, GetSystemMetrics, GetWindow, GetWindowLongW,
        GetWindowPlacement, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        PostMessageW, RegisterClassW, SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL,
        FLASHW_TIMERNOFG, GWL_EXSTYLE, GW_OWNER, SM_CXSCREEN, SM_CYSCREEN, SW_MINIMIZE, SW_RESTORE,
        WINDOWPLACEMENT, WM_CLOSE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };

    // ------------------------------------------------- display resolution

    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        ChangeDisplaySettingsExW, EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow,
        CDS_FULLSCREEN, CDS_TYPE, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY,
        DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_DISPLAY_SETTINGS_MODE, ENUM_REGISTRY_SETTINGS,
        MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
    };

    pub const CAN_SET_RESOLUTION: bool = true;

    /// One of the primary display's modes; `None` past the last one.
    fn display_mode(i: ENUM_DISPLAY_SETTINGS_MODE) -> Option<DEVMODEW> {
        let mut dm = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        unsafe { EnumDisplaySettingsW(PCWSTR::null(), i, &mut dm) }
            .as_bool()
            .then_some(dm)
    }

    /// The primary display's resolution on the desktop (its saved mode,
    /// not one Moonglass set for itself).
    pub fn desktop_resolution() -> Option<(u32, u32)> {
        display_mode(ENUM_REGISTRY_SETTINGS).map(|d| (d.dmPelsWidth, d.dmPelsHeight))
    }

    /// The primary display's resolutions in its desktop orientation,
    /// largest first, each once.
    pub fn resolutions() -> Vec<(u32, u32)> {
        let landscape = desktop_resolution().is_none_or(|(w, h)| w >= h);
        let mut out: Vec<(u32, u32)> = (0..)
            .map_while(|i| display_mode(ENUM_DISPLAY_SETTINGS_MODE(i)))
            .filter(|d| d.dmBitsPerPel >= 32)
            .map(|d| (d.dmPelsWidth, d.dmPelsHeight))
            .filter(|&(w, h)| w > 0 && h > 0 && (w >= h) == landscape)
            .collect();
        out.sort_unstable_by_key(|&(w, h)| std::cmp::Reverse((w * h, w)));
        out.dedup();
        out
    }

    /// The best mode at `w`×`h`: the desktop's refresh rate if the display
    /// has it at that size, else the highest.
    fn mode_for(w: u32, h: u32) -> Option<DEVMODEW> {
        let desktop_hz = display_mode(ENUM_REGISTRY_SETTINGS).map(|d| d.dmDisplayFrequency);
        (0..)
            .map_while(|i| display_mode(ENUM_DISPLAY_SETTINGS_MODE(i)))
            .filter(|d| d.dmPelsWidth == w && d.dmPelsHeight == h && d.dmBitsPerPel >= 32)
            .max_by_key(|d| {
                (
                    Some(d.dmDisplayFrequency) == desktop_hz,
                    d.dmDisplayFrequency,
                )
            })
    }

    /// Switches the primary display to `size` for as long as Moonglass
    /// runs (`CDS_FULLSCREEN`: never saved, and undone by Windows when
    /// Moonglass exits), or back to the desktop's own resolution with
    /// `None`.
    pub fn set_resolution(size: Option<(u32, u32)>) -> crate::Result<()> {
        let rc = match size {
            Some((w, h)) => {
                let mut dm = mode_for(w, h).ok_or_else(|| {
                    crate::PlatformError::Os(format!("the display has no {w} × {h} mode"))
                })?;
                dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
                unsafe {
                    ChangeDisplaySettingsExW(PCWSTR::null(), Some(&dm), None, CDS_FULLSCREEN, None)
                }
            }
            None => unsafe {
                ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None)
            },
        };
        if rc == DISP_CHANGE_SUCCESSFUL {
            Ok(())
        } else {
            Err(crate::PlatformError::Os(format!(
                "the display mode could not be changed ({})",
                rc.0
            )))
        }
    }

    /// Resizes a fullscreen window to cover its display. The toolkit keeps
    /// the display size it saw when the window opened, which is wrong once
    /// the resolution changed. Meant for another thread than the window's:
    /// the window handles the resize in its own turn.
    pub fn fit_to_display(raw: isize) {
        if raw == 0 {
            return;
        }
        let h = hwnd(raw);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let monitor = MonitorFromWindow(h, MONITOR_DEFAULTTOPRIMARY);
            if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                let r = mi.rcMonitor;
                let _ = SetWindowPos(
                    h,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_ASYNCWINDOWPOS,
                );
            }
        }
    }

    /// A desktop notification: a tray balloon, which Windows 10 and 11 show
    /// as a regular notification. The tray icon is removed after a while.
    pub fn notify(raw: isize, title: &str, body: &str) {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
            NOTIFYICONDATAW,
        };
        use windows::Win32::UI::WindowsAndMessaging::{LoadIconW, IDI_APPLICATION};

        fn copy(dst: &mut [u16], s: &str) {
            let n = dst.len() - 1;
            for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(n)) {
                *d = c;
            }
        }
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd(raw),
            uID: 0x4D47,
            uFlags: NIF_INFO | NIF_ICON | NIF_TIP,
            dwInfoFlags: NIIF_INFO,
            ..Default::default()
        };
        unsafe {
            // The program's own icon (resource 1), else the generic one.
            let module = GetModuleHandleW(None).ok();
            data.hIcon = module
                .and_then(|m| LoadIconW(Some(m.into()), windows::core::PCWSTR(1 as _)).ok())
                .or_else(|| LoadIconW(None, IDI_APPLICATION).ok())
                .unwrap_or_default();
        }
        copy(&mut data.szTip, "Moonglass");
        copy(&mut data.szInfoTitle, title);
        copy(&mut data.szInfo, body);
        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                tracing::warn!("desktop notification unavailable");
                return;
            }
        }
        let handle = raw;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            let data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd(handle),
                uID: 0x4D47,
                ..Default::default()
            };
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
        });
    }

    fn hwnd(raw: isize) -> HWND {
        HWND(raw as *mut _)
    }

    /// A plain black window over the main screen, up the moment Moonglass
    /// starts and until its real window has drawn. The UI toolkit needs
    /// most of a second (graphics device, fonts) before it can open one;
    /// the startup animation begins on black, so one runs into the other.
    pub struct Splash(isize);

    unsafe extern "system" fn splash_proc(w: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        DefWindowProcW(w, msg, wp, lp)
    }

    impl Splash {
        /// Closes the splash window (dropping it does the same).
        pub fn close(self) {}

        /// Call on the thread that will run the UI, and drop it there: the
        /// window belongs to that thread.
        pub fn show() -> Option<Self> {
            unsafe {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(splash_proc),
                    hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                    lpszClassName: w!("MoonglassSplash"),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let window = CreateWindowExW(
                    // No taskbar button of its own.
                    WS_EX_TOOLWINDOW,
                    class.lpszClassName,
                    None,
                    WS_POPUP | WS_VISIBLE,
                    0,
                    0,
                    GetSystemMetrics(SM_CXSCREEN),
                    GetSystemMetrics(SM_CYSCREEN),
                    None,
                    None,
                    None,
                    None,
                )
                .ok()?;
                // Painted now: nothing runs this thread's messages for a while.
                let _ = UpdateWindow(window);
                Some(Self(window.0 as isize))
            }
        }
    }

    impl Drop for Splash {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(hwnd(self.0));
            }
        }
    }

    /// Minimizes Moonglass: the fallback when the game's own window cannot
    /// be found to bring in front ([`show_game`]).
    pub fn hide_for_game(raw: isize) {
        unsafe {
            let _ = ShowWindow(hwnd(raw), SW_MINIMIZE);
        }
    }

    struct GameSearch<'a> {
        pids: &'a [u32],
        /// The largest window found, by its normal (not minimized) size.
        best: Option<(HWND, i64)>,
    }

    unsafe extern "system" fn visit_game(h: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut GameSearch);
        if !IsWindowVisible(h).as_bool() || GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return true.into();
        }
        if GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if !search.pids.contains(&pid) {
            return true.into();
        }
        let mut place = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if GetWindowPlacement(h, &mut place).is_err() {
            return true.into();
        }
        let r = place.rcNormalPosition;
        let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
        if search.best.is_none_or(|(_, a)| area > a) {
            search.best = Some((h, area));
        }
        true.into()
    }

    /// Brings the game's main window (the largest visible top-level window
    /// of `pids`) in front, restoring it first if it is minimized, as an
    /// exclusive-fullscreen game is when it loses focus. Call while
    /// Moonglass is in front, which is what lets it hand the foreground
    /// over. Returns whether the game is now in front.
    pub fn show_game(pids: &[u32]) -> bool {
        if pids.is_empty() {
            return false;
        }
        let mut search = GameSearch { pids, best: None };
        unsafe {
            let _ = EnumWindows(
                Some(visit_game),
                LPARAM(&mut search as *mut GameSearch as isize),
            );
            let Some((h, _)) = search.best else {
                return false;
            };
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let _ = SetForegroundWindow(h);
            is_foreground(h)
        }
    }

    fn is_foreground(h: HWND) -> bool {
        unsafe { GetForegroundWindow() == h }
    }

    fn tap_alt() {
        let key = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_MENU,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }

    /// Restore + `SetForegroundWindow`, then the `AttachThreadInput`
    /// technique, then a synthetic Alt tap: three attempts in total, each
    /// confirmed with `GetForegroundWindow`. On failure the taskbar button
    /// flashes and `NotFocused` is returned so the caller can notify and log.
    pub fn regain_focus(raw: isize) -> FocusResult {
        let h = hwnd(raw);
        unsafe {
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
        if is_foreground(h) {
            return FocusResult::Focused;
        }
        std::thread::sleep(Duration::from_millis(50));

        unsafe {
            let fg = GetForegroundWindow();
            let fg_thread = GetWindowThreadProcessId(fg, None);
            let me = GetCurrentThreadId();
            let attached = fg_thread != 0
                && fg_thread != me
                && AttachThreadInput(me, fg_thread, true).as_bool();
            let _ = BringWindowToTop(h);
            let _ = SetForegroundWindow(h);
            if attached {
                let _ = AttachThreadInput(me, fg_thread, false);
            }
        }
        if is_foreground(h) {
            return FocusResult::Focused;
        }
        std::thread::sleep(Duration::from_millis(50));

        tap_alt();
        unsafe {
            let _ = SetForegroundWindow(h);
        }
        if is_foreground(h) {
            return FocusResult::Focused;
        }

        request_attention(raw);
        FocusResult::NotFocused
    }

    /// Taskbar flash until Moonglass is focused.
    fn request_attention(raw: isize) {
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: hwnd(raw),
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        unsafe {
            let _ = FlashWindowEx(&info);
        }
    }

    use crate::FocusResult;
    pub use crate::Launcher;

    /// (owning process names, exact main-window title). Only the main
    /// client window matches; dialogs such as cloud-save conflicts have
    /// other titles and are never touched.
    fn identity(l: Launcher) -> (&'static [&'static str], &'static str) {
        match l {
            Launcher::Steam => (&["steamwebhelper.exe", "steam.exe"], "Steam"),
            Launcher::Epic => (&["epicgameslauncher.exe"], "Epic Games Launcher"),
            Launcher::Gog => (&["galaxyclient.exe"], "GOG Galaxy"),
            Launcher::BattleNet => (&["battle.net.exe"], "Battle.net"),
        }
    }

    fn process_file_name(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                h,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            CloseHandle(h).ok();
            ok.ok()?;
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit('\\').next().map(str::to_lowercase)
        }
    }

    struct Search {
        names: &'static [&'static str],
        title: &'static str,
        found: Vec<HWND>,
    }

    unsafe extern "system" fn visit(h: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        if !IsWindowVisible(h).as_bool() || GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return true.into();
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(h, &mut buf);
        if n <= 0 || String::from_utf16_lossy(&buf[..n as usize]) != search.title {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if process_file_name(pid).is_some_and(|n| search.names.contains(&n.as_str())) {
            search.found.push(h);
        }
        true.into()
    }

    /// If the client's main window is in front (the foreground window),
    /// sends it a normal close request, which sends Steam and the Epic
    /// launcher to the system tray. A client window the user left open
    /// behind something else is not touched. The client keeps running so
    /// cloud saves finish syncing. Call this before taking focus back.
    /// Returns how many windows were asked to close.
    pub fn dismiss_launcher_window(launcher: Launcher) -> usize {
        let (names, title) = identity(launcher);
        let mut search = Search {
            names,
            title,
            found: Vec::new(),
        };
        let mut closed = 0;
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
            let front = GetForegroundWindow();
            for h in search.found.iter().filter(|h| **h == front) {
                let _ = PostMessageW(Some(*h), WM_CLOSE, WPARAM(0), LPARAM(0));
                closed += 1;
            }
        }
        closed
    }
}

// ------------------------------------------------------------------ hotkey

/// The background restore hotkey (spec 5.4): an XInput poller for the
/// controller hold ([`crate::RestoreConfig`]: View + Menu or both sticks),
/// and `Ctrl + Alt + Home` as a global keyboard hotkey.
/// Both deliver to the same callback and keep working while Moonglass is not
/// focused. Arm and disarm with [`hotkey::RestoreHotkey::set_armed`]: it is only
/// active while Launching or Running, and the keyboard combination is only
/// registered with Windows for that time, so other programs can use it the
/// rest of the time.
pub mod hotkey {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_HOME,
    };
    use windows::Win32::UI::Input::XboxController::{
        XInputGetState, XINPUT_GAMEPAD_BACK, XINPUT_GAMEPAD_LEFT_THUMB, XINPUT_GAMEPAD_RIGHT_THUMB,
        XINPUT_GAMEPAD_START, XINPUT_STATE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY, WM_USER,
    };

    const HOTKEY_ID: i32 = 1;
    /// Thread messages from [`RestoreHotkey::set_armed`] to the keyboard thread.
    const WM_ARM: u32 = WM_APP + 1;
    const WM_DISARM: u32 = WM_APP + 2;

    /// What is available on this machine, shown in Settings.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Availability {
        pub gamepad: bool,
        pub keyboard: bool,
    }

    pub struct RestoreHotkey {
        armed: Arc<AtomicBool>,
        availability: Availability,
        /// The keyboard thread, when the combination is available.
        keyboard_thread: Option<u32>,
    }

    impl RestoreHotkey {
        /// Starts the two background threads. `on_restore` runs on one of
        /// them; it should only post a message to the UI.
        pub fn start(
            config: Arc<crate::RestoreConfig>,
            on_restore: impl Fn() + Send + Sync + 'static,
        ) -> Self {
            let armed = Arc::new(AtomicBool::new(false));
            let cb: Arc<dyn Fn() + Send + Sync> = Arc::new(on_restore);

            {
                let armed = armed.clone();
                let cb = cb.clone();
                std::thread::Builder::new()
                    .name("moonglass-xinput".into())
                    .spawn(move || xinput_loop(&config, &armed, &*cb))
                    .ok();
            }

            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("moonglass-hotkey".into())
                .spawn(move || keyboard_loop(&*cb, tx))
                .ok();
            let keyboard_thread = rx.recv_timeout(Duration::from_secs(1)).ok().flatten();
            Self {
                armed,
                availability: Availability {
                    gamepad: true,
                    keyboard: keyboard_thread.is_some(),
                },
                keyboard_thread,
            }
        }

        pub fn set_armed(&self, armed: bool) {
            if self.armed.swap(armed, Ordering::Relaxed) == armed {
                return;
            }
            if let Some(thread) = self.keyboard_thread {
                let msg = if armed { WM_ARM } else { WM_DISARM };
                if let Err(e) = unsafe { PostThreadMessageW(thread, msg, WPARAM(0), LPARAM(0)) } {
                    tracing::warn!(error = %e, "could not reach the hotkey thread");
                }
            }
        }

        pub fn availability(&self) -> Availability {
            self.availability
        }
    }

    fn combo_down(state: &XINPUT_STATE, combo: crate::RestoreCombo) -> bool {
        let want = match combo {
            crate::RestoreCombo::ViewMenu => XINPUT_GAMEPAD_BACK.0 | XINPUT_GAMEPAD_START.0,
            crate::RestoreCombo::StickClicks => {
                XINPUT_GAMEPAD_LEFT_THUMB.0 | XINPUT_GAMEPAD_RIGHT_THUMB.0
            }
        };
        state.Gamepad.wButtons.0 & want == want
    }

    fn xinput_loop(
        config: &crate::RestoreConfig,
        armed: &AtomicBool,
        cb: &(dyn Fn() + Send + Sync),
    ) {
        let mut held_since: Option<Instant> = None;
        let mut fired = false;
        // Asking an empty XInput slot is slow, so only connected slots are
        // polled; empty ones are looked at again every two seconds.
        let mut connected = [false; 4];
        let mut last_probe: Option<Instant> = None;
        loop {
            if !armed.load(Ordering::Relaxed) {
                held_since = None;
                fired = false;
                last_probe = None;
                // Idle cost while no game runs.
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            let probe = last_probe.is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
            if probe {
                last_probe = Some(Instant::now());
            }
            let mut any = false;
            for (i, slot) in connected.iter_mut().enumerate() {
                if !*slot && !probe {
                    continue;
                }
                let mut st = XINPUT_STATE::default();
                *slot = unsafe { XInputGetState(i as u32, &mut st) } == 0;
                if *slot && combo_down(&st, config.combo()) {
                    any = true;
                }
            }
            match (any, held_since) {
                (true, None) => held_since = Some(Instant::now()),
                (true, Some(t)) if !fired && t.elapsed() >= config.hold() => {
                    fired = true;
                    cb();
                }
                (false, _) => {
                    held_since = None;
                    fired = false;
                }
                _ => {}
            }
            // About ten times per second.
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn register() -> bool {
        unsafe {
            RegisterHotKey(
                None,
                HOTKEY_ID,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                VK_HOME.0 as u32,
            )
        }
        .is_ok()
    }

    fn unregister() {
        unsafe {
            let _ = UnregisterHotKey(None, HOTKEY_ID);
        }
    }

    /// Reports this thread's ID through `ready` (`None` if the combination
    /// is taken), then registers the hotkey while armed and unregisters it
    /// otherwise.
    fn keyboard_loop(cb: &(dyn Fn() + Send + Sync), ready: std::sync::mpsc::Sender<Option<u32>>) {
        let mut msg = MSG::default();
        // Creates this thread's message queue, so `set_armed` can post to it.
        unsafe {
            let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        }
        // Tried once now so that Settings can say whether it is available.
        let free = register();
        if free {
            unregister();
        } else {
            tracing::warn!(
                "Ctrl+Alt+Home is taken by another program; keyboard restore hotkey unavailable"
            );
        }
        let _ = ready.send(free.then(|| unsafe { GetCurrentThreadId() }));
        if !free {
            return;
        }
        let mut registered = false;
        // GetMessageW returns 0 on WM_QUIT and -1 on failure.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            match msg.message {
                WM_ARM if !registered => {
                    registered = register();
                    if !registered {
                        tracing::warn!("Ctrl+Alt+Home could not be registered for this game");
                    }
                }
                WM_DISARM if registered => {
                    unregister();
                    registered = false;
                }
                WM_HOTKEY => cb(),
                _ => {}
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Whether another thread could register the combination right now.
        fn combination_is_free() -> bool {
            let free = register();
            if free {
                unregister();
            }
            free
        }

        fn wait_until(want_free: bool) -> bool {
            for _ in 0..40 {
                if combination_is_free() == want_free {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        }

        #[test]
        fn keyboard_hotkey_is_held_only_while_armed() {
            let config =
                crate::RestoreConfig::new(crate::RestoreCombo::ViewMenu, Duration::from_secs(2));
            let hotkey = RestoreHotkey::start(config, || {});
            if !hotkey.availability().keyboard {
                // Another program owns Ctrl+Alt+Home on this machine.
                return;
            }
            assert!(combination_is_free(), "free before a game");
            hotkey.set_armed(true);
            assert!(wait_until(false), "held while a game runs");
            hotkey.set_armed(false);
            assert!(wait_until(true), "released after the game");
        }
    }
}
