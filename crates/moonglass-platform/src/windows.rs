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
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::{
    name_matches, within_normalized, AppPaths, EpicSource, PlatformError, Prepared, ProcessInfo,
    Result,
};

/// Windows reads the background restore combination with its own XInput
/// poller, so the gamepad thread does not need to.
pub const GAMEPAD_RESTORE_VIA_GILRS: bool = false;

pub const PLATFORM_NAME: &str = "Windows";

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
    let roaming = base.config_dir().join("Moonglass");
    let local = base.data_local_dir().join("Moonglass");
    Ok(AppPaths {
        data_dir: roaming.clone(),
        config_dir: roaming,
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

// ---------------------------------------------------------------- registry

fn reg_get_string(subkey: &str, value: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);
    let mut len: u32 = 0;
    unsafe {
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
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
            HKEY_CURRENT_USER,
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

pub fn launch_steam(app_id: &str) -> Result<()> {
    open_uri(&format!("steam://rungameid/{app_id}"))
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

/// A child process inside a Job Object. The job is not kill-on-close, so a
/// crash of Moonglass never takes the game down with it.
pub struct Tracked {
    child: Child,
    job: HANDLE,
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
        if let Err(e) = unsafe { AssignProcessToJobObject(job, handle) } {
            // Still usable: fall back to watching the direct child only.
            tracing::warn!(error = %e, "could not place game in a Job Object");
        }
        Ok(Self { child, job })
    }

    fn active_processes(&self) -> Option<u32> {
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
        unsafe { TerminateJobObject(self.job, 1) }
            .or_else(|_| self.child.kill().map_err(windows::core::Error::from))
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
        Prepared::Steam { dir, .. } | Prepared::Folder { dir } => {
            p.exe.is_some_and(|exe| within_normalized(exe, dir))
        }
        Prepared::Name(n) => name_matches(p.name, n),
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

    use windows::core::BOOL;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::{
        AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_MENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, FlashWindowEx, GetForegroundWindow, GetWindow,
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
        SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, GW_OWNER,
        SW_MINIMIZE, SW_RESTORE, WM_CLOSE,
    };

    fn hwnd(raw: isize) -> HWND {
        HWND(raw as *mut _)
    }

    /// While a game runs, Moonglass's window is minimized.
    pub fn hide_for_game(raw: isize) {
        unsafe {
            let _ = ShowWindow(hwnd(raw), SW_MINIMIZE);
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
    pub fn request_attention(raw: isize) {
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

    /// Sends a normal close request to the client's main window, which sends
    /// Steam and the Epic launcher to the system tray. The client keeps
    /// running so cloud saves finish syncing. Returns how many windows were
    /// asked to close.
    pub fn dismiss_launcher_window(launcher: Launcher) -> usize {
        let (names, title) = identity(launcher);
        let mut search = Search {
            names,
            title,
            found: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
            for h in &search.found {
                let _ = PostMessageW(Some(*h), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        search.found.len()
    }
}

// ------------------------------------------------------------------ hotkey

/// The background restore hotkey (spec 5.4): an XInput poller for the
/// `View + Menu` hold, and `Ctrl + Alt + Home` as a global keyboard hotkey.
/// Both deliver to the same callback and keep working while Moonglass is not
/// focused. Arm and disarm with [`RestoreHotkey::set_armed`]: it is only
/// active while Launching or Running.
pub mod hotkey {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_HOME,
    };
    use windows::Win32::UI::Input::XboxController::{
        XInputGetState, XINPUT_GAMEPAD_BACK, XINPUT_GAMEPAD_START, XINPUT_STATE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    /// What is available on this machine, shown in Settings.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Availability {
        pub gamepad: bool,
        pub keyboard: bool,
    }

    pub struct RestoreHotkey {
        armed: Arc<AtomicBool>,
        availability: Availability,
    }

    impl RestoreHotkey {
        /// Starts the two background threads. `on_restore` runs on one of
        /// them; it should only post a message to the UI.
        pub fn start(hold: Duration, on_restore: impl Fn() + Send + Sync + 'static) -> Self {
            let armed = Arc::new(AtomicBool::new(false));
            let cb: Arc<dyn Fn() + Send + Sync> = Arc::new(on_restore);

            {
                let armed = armed.clone();
                let cb = cb.clone();
                std::thread::Builder::new()
                    .name("moonglass-xinput".into())
                    .spawn(move || xinput_loop(hold, &armed, &*cb))
                    .ok();
            }

            let (tx, rx) = std::sync::mpsc::channel();
            {
                let armed = armed.clone();
                std::thread::Builder::new()
                    .name("moonglass-hotkey".into())
                    .spawn(move || keyboard_loop(&armed, &*cb, tx))
                    .ok();
            }
            let keyboard = rx.recv_timeout(Duration::from_secs(1)).unwrap_or(false);
            Self {
                armed,
                availability: Availability {
                    gamepad: true,
                    keyboard,
                },
            }
        }

        pub fn set_armed(&self, armed: bool) {
            self.armed.store(armed, Ordering::Relaxed);
        }

        pub fn availability(&self) -> Availability {
            self.availability
        }
    }

    fn combo_down(state: &XINPUT_STATE) -> bool {
        let want = XINPUT_GAMEPAD_BACK.0 | XINPUT_GAMEPAD_START.0;
        state.Gamepad.wButtons.0 & want == want
    }

    fn xinput_loop(hold: Duration, armed: &AtomicBool, cb: &(dyn Fn() + Send + Sync)) {
        let mut held_since: Option<Instant> = None;
        let mut fired = false;
        loop {
            if !armed.load(Ordering::Relaxed) {
                held_since = None;
                fired = false;
                // Idle cost while no game runs.
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            let mut any = false;
            for i in 0..4 {
                let mut st = XINPUT_STATE::default();
                if unsafe { XInputGetState(i, &mut st) } == 0 && combo_down(&st) {
                    any = true;
                }
            }
            match (any, held_since) {
                (true, None) => held_since = Some(Instant::now()),
                (true, Some(t)) if !fired && t.elapsed() >= hold => {
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

    fn keyboard_loop(
        armed: &AtomicBool,
        cb: &(dyn Fn() + Send + Sync),
        ready: std::sync::mpsc::Sender<bool>,
    ) {
        let ok = unsafe {
            RegisterHotKey(
                None,
                1,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                VK_HOME.0 as u32,
            )
        }
        .is_ok();
        let _ = ready.send(ok);
        if !ok {
            tracing::warn!(
                "Ctrl+Alt+Home is taken by another program; keyboard restore hotkey unavailable"
            );
            return;
        }
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            if msg.message == WM_HOTKEY && armed.load(Ordering::Relaxed) {
                cb();
            }
        }
    }
}
