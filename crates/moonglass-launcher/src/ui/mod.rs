//! The GPUI front end (spec 3, 4.1, 5.3, 5.4, 5.5).
//!
//! Threading: UI state lives on GPUI's foreground executor. Database,
//! scanning, artwork and process polling run on a small tokio runtime and
//! report back through one channel ([`UiEvent`]), the same bridge pattern as
//! Zed's `gpui_tokio`.
//!
//! Layers, bottom to top: the home screen (carousel) or the blurred
//! backdrop it crossfades into, the launch / Now Playing overlay, Settings,
//! a modal (on-screen keyboard, file browser, per-game options or an
//! emulator menu), and a toast. Input goes to the topmost layer.

mod anim;
mod browser;
mod emulators;
mod gamepad;
mod home;
mod icons;
mod menu;
mod options;
mod osk;
pub mod prefs;
mod settings;
pub mod spring;
mod theme;

use std::cell::RefCell;
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, FocusHandle, Hsla, KeyDownEvent,
    KeyUpEvent, MouseMoveEvent, ScrollWheelEvent, Window, WindowBounds, WindowOptions,
};
use moonglass_art::ingest::{blurred_hero_path, motion_hero_path};
use moonglass_core::library::{carousel, search, visible_filters, Filter};
use moonglass_core::lifecycle::{AbandonReason, Effect, Lifecycle, State, Timing};
use moonglass_core::{Game, GameId, Source};
use moonglass_db::Db;
use moonglass_platform::hotkey::RestoreHotkey;
use moonglass_platform::window::Splash;
use moonglass_platform::{
    AppPaths, FocusResult, InstanceGuard, PadPower, ProcessWatcher, TrackedChild,
};

use crate::friends::{self, Friend};
use crate::launch::{self, Running, SteamJoin};
use crate::now_unix;
use crate::steam_accounts::SteamAccounts;
use anim::Anims;
use browser::{Browser, BrowserResult, Entry, Purpose};
use emulators::EmulatorInfo;
use gamepad::{ControllerStatus, NavAction, PadEvent};
use menu::Menu;
use options::{Opt, OptionsMenu};
use osk::{Osk, OskResult, TextTarget};
use prefs::Prefs;
use settings::{Item, SettingsCmd, SettingsInfo, SettingsState};
use spring::Spring;
use theme::{hex_color, on};

/// Mouse-wheel distance that moves the selection by one card.
const WHEEL_STEP_PX: f32 = 60.0;
const CROSSFADE_SECS: f32 = 0.35;
const PULSE_PERIOD_SECS: f32 = 3.2;
/// The glow pulse is a slow opacity change; redrawing it at ~30 Hz is
/// indistinguishable from the display rate and costs a fraction of the
/// CPU/GPU. Motion (scrolling, crossfades) still runs at the refresh rate.
const PULSE_FRAME: Duration = Duration::from_millis(33);
const CARET_BLINK_MS: u128 = 530;
const INTRO_SECS: f32 = 1.6;
const TOAST_SECS: u64 = 4;
/// Holding Esc or B on the home screen this long exits Moonglass.
const QUIT_HOLD: Duration = Duration::from_millis(1400);
/// The exit progress appears after this, so a plain press shows nothing.
const QUIT_HINT_DELAY: Duration = Duration::from_millis(250);
/// After Stop, how long the game gets to close before Force stop is offered.
const STOP_GRACE: Duration = Duration::from_secs(8);

/// Everything the UI needs from the rest of the app.
pub struct Backend {
    pub db: Db,
    pub rt: tokio::runtime::Handle,
    pub paths: AppPaths,
    /// `None` only for a development second instance.
    pub instance: Option<InstanceGuard>,
}

/// What the first frame shows, read while the UI toolkit starts.
pub struct Loaded {
    prefs: Prefs,
    sgdb_key_set: bool,
    emulators: Vec<EmulatorInfo>,
    games: Vec<Game>,
}

impl Loaded {
    /// Anything that cannot be read is left at its default; the scan that
    /// follows the first frame reports real trouble.
    pub async fn read(db: &Db) -> Self {
        let key = db.setting::<String>(crate::artwork::SETTING_SGDB_KEY).await;
        Self {
            prefs: db
                .setting(prefs::PREFS_KEY)
                .await
                .ok()
                .flatten()
                .unwrap_or_default(),
            sgdb_key_set: key.ok().flatten().is_some_and(|k| !k.trim().is_empty()),
            emulators: emulators::load(db).await.unwrap_or_default(),
            games: db.games().await.unwrap_or_default(),
        }
    }
}

enum UiEvent {
    Library(Vec<Game>),
    Emulators(Vec<EmulatorInfo>),
    Friends(friends::Snapshot, friends::Status),
    /// The Steam accounts, or the account Steam is signed in with, changed.
    SteamAccounts(SteamAccounts),
    /// A switch of Steam account finished.
    SteamSwitched(Result<(), String>, SteamAccounts),
    Pad(PadEvent),
    Power(Option<PadPower>),
    Restore,
    /// `session` says which launch an event belongs to, so a late event
    /// from a cancelled launch never reaches the next one.
    LaunchFailed {
        session: u64,
        message: String,
    },
    Observed {
        session: u64,
        alive: bool,
    },
    Tick,
    Toast(String),
    Dir {
        generation: u64,
        dir: Option<PathBuf>,
        result: Result<Vec<Entry>, String>,
    },
}

#[derive(Clone, PartialEq)]
struct Ambient {
    background: Option<PathBuf>,
    fanart: Option<PathBuf>,
    /// The art smeared sideways, for the motion blur background.
    motion: Option<PathBuf>,
    dominant: Hsla,
    accent: Hsla,
}

/// The last input device used; hints follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Gamepad,
    Keyboard,
    Mouse,
}

/// What a button in the action bar does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hint {
    Play,
    Back,
    Search,
    Options,
    Filter,
    Settings,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopState {
    Idle,
    /// A close request was sent at this time.
    Requested(Instant),
    /// The game ignored the request; Force stop is offered.
    CanForce,
    Forcing,
}

enum Overlay {
    None,
    Launching { still_waiting: bool },
    Playing { stop: StopState },
    Message(String),
}

enum Modal {
    None,
    Keyboard(Osk),
    Browser(Browser),
    Options(Box<OptionsMenu>),
    Menu(Menu),
}

enum Screen {
    Home,
    Settings(SettingsState),
}

/// Stop requests passed to the polling task.
const STOP_NONE: u8 = 0;
const STOP_POLITE: u8 = 1;
const STOP_FORCE: u8 = 2;

struct Session {
    id: u64,
    game: Game,
    stop_polling: Arc<AtomicBool>,
    stop_request: Arc<AtomicU8>,
    origin: Instant,
    running_since: Option<Instant>,
}

pub struct LauncherView {
    backend: Arc<Backend>,
    tx: UnboundedSender<UiEvent>,
    focus: FocusHandle,
    prefs: Prefs,

    /// As stored.
    games: Vec<Game>,
    /// Emulator profiles with their ROM folders, for Settings.
    emulators: Vec<EmulatorInfo>,
    /// An emulator whose menu opens once the database has it.
    pending_emulator_menu: Option<String>,
    /// Steam friends in a game right now (empty without an API key).
    friends: friends::Snapshot,
    friends_status: friends::Status,
    /// Wakes the friends poll when the key changes.
    friends_wake: Arc<tokio::sync::Notify>,
    steam: SteamAccounts,
    /// The account Steam is being switched to (its display name).
    steam_switching: Option<String>,
    /// A game whose borrowed copy is in use and whose Play was pressed
    /// once: the second press starts it anyway.
    play_anyway: Option<GameId>,
    /// What the carousel shows: `games`, with hidden games un-hidden when
    /// "Show hidden games" is on.
    shown: Vec<Game>,
    hidden_ids: HashSet<GameId>,
    filters: Vec<Filter>,
    filter: Filter,
    order: Vec<usize>,
    selected: usize,
    query: Option<String>,
    search_focused: bool,
    saved_position: Option<(Filter, usize)>,

    spring: Spring,
    last_frame: Option<Instant>,
    /// When Esc or B started being held on the home screen.
    quit_hold: Option<Instant>,
    started: Instant,
    intro_start: Option<Instant>,
    ambient_from: Option<Ambient>,
    ambient_to: Option<Ambient>,
    crossfade: f32,
    /// Which way the focus last moved (+1 right, -1 left): the motion blur
    /// background slides with it.
    motion_dir: f32,
    caret_epoch: Instant,
    repaint_pending: bool,
    /// Every animated value except the carousel position (see [`anim`]).
    /// In a cell because drawing code only has `&self`.
    anims: RefCell<Anims>,

    screen: Screen,
    /// Settings while it fades out after closing.
    settings_leaving: Option<SettingsState>,
    modal: Modal,
    /// The modal that was just closed, while it fades out.
    modal_leaving: Option<Modal>,
    overlay: Overlay,
    toast: Option<(String, Instant)>,
    browser_generation: u64,

    lifecycle: Lifecycle,
    session: Option<Session>,
    last_session_id: u64,
    controller: ControllerStatus,
    xinput_power: Option<PadPower>,
    restore_armed: Arc<AtomicBool>,
    hotkey: RestoreHotkey,
    sgdb_key_set: bool,
    start_at_login: bool,

    /// The black window shown since the program started, until the first
    /// frame is on screen.
    splash: Option<Splash>,
    drawn: bool,
    loaded: bool,
    /// Logo sizes, so the logo can be laid out left-aligned at its own
    /// aspect ratio (an image box with Contain would center it).
    logo_sizes: std::collections::HashMap<String, Option<(u32, u32)>>,
    /// Until the user moves, rescans keep focus on the first card (the most
    /// recently played game) instead of following the previous one.
    user_moved: bool,
    input: InputMode,
    wheel_accum: f32,
}

fn raw_window(window: &Window) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // GPUI has an inherent `window_handle` too; ask the trait explicitly.
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

impl LauncherView {
    fn new(
        backend: Arc<Backend>,
        loaded: Loaded,
        splash: Option<Splash>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, mut rx) = unbounded::<UiEvent>();

        // Event pump: everything from other threads arrives here.
        cx.spawn_in(window, async move |this, cx| {
            while let Some(ev) = rx.next().await {
                if this
                    .update_in(cx, |view, window, cx| view.handle(ev, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let restore_armed = Arc::new(AtomicBool::new(false));
        let pad_tx = tx.clone();
        let (pad_sender, mut pad_rx) = unbounded::<PadEvent>();
        gamepad::spawn(
            pad_sender,
            restore_armed.clone(),
            moonglass_platform::GAMEPAD_RESTORE_VIA_GILRS,
            Duration::from_secs(2),
        );
        backend.rt.spawn(async move {
            while let Some(e) = pad_rx.next().await {
                let ev = match e {
                    PadEvent::Restore => UiEvent::Restore,
                    other => UiEvent::Pad(other),
                };
                if pad_tx.unbounded_send(ev).is_err() {
                    break;
                }
            }
        });

        let hk_tx = tx.clone();
        let hotkey = RestoreHotkey::start(Duration::from_secs(2), move || {
            let _ = hk_tx.unbounded_send(UiEvent::Restore);
        });

        // Clock, toasts, stop timeouts and second-instance activation.
        let tick_tx = tx.clone();
        backend.rt.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if tick_tx.unbounded_send(UiEvent::Tick).is_err() {
                    break;
                }
            }
        });

        // Steam friends, paused while a game is tracked.
        let friends_wake = Arc::new(tokio::sync::Notify::new());
        let friends_tx = tx.clone();
        friends::spawn_poll(
            &backend.rt,
            backend.db.clone(),
            restore_armed.clone(),
            friends_wake.clone(),
            move |snapshot, status| {
                friends_tx
                    .unbounded_send(UiEvent::Friends(snapshot, status))
                    .is_ok()
            },
        );

        // Steam accounts: which one is signed in (it may change in Steam
        // itself) and what it can play. Reading the registry is cheap; the
        // files are read again only when something changed.
        let steam_tx = tx.clone();
        let steam_db = backend.db.clone();
        backend.rt.spawn(async move {
            let mut last = None;
            loop {
                if let Ok(Some(root)) = crate::library::steam_root(&steam_db).await {
                    let probe = root.clone();
                    let now = tokio::task::spawn_blocking(move || {
                        let users = probe.join("config").join("loginusers.vdf");
                        let changed = std::fs::metadata(users).and_then(|m| m.modified()).ok();
                        (moonglass_platform::steam_active_account(), changed)
                    })
                    .await
                    .ok();
                    if now.is_some() && now != last {
                        last = now;
                        let loaded =
                            tokio::task::spawn_blocking(move || SteamAccounts::load(&root)).await;
                        if let Ok(accounts) = loaded {
                            if steam_tx
                                .unbounded_send(UiEvent::SteamAccounts(accounts))
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(4)).await;
            }
        });

        // Controller battery where gilrs cannot read it (XInput on Windows).
        let power_tx = tx.clone();
        backend.rt.spawn(async move {
            loop {
                let p = tokio::task::spawn_blocking(moonglass_platform::xinput_power)
                    .await
                    .ok()
                    .flatten();
                if power_tx.unbounded_send(UiEvent::Power(p)).is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });

        let mut view = Self {
            backend,
            tx,
            focus: cx.focus_handle(),
            prefs: loaded.prefs,
            games: Vec::new(),
            emulators: loaded.emulators,
            pending_emulator_menu: None,
            friends: friends::Snapshot::default(),
            friends_status: friends::Status::default(),
            friends_wake,
            steam: SteamAccounts::default(),
            steam_switching: None,
            play_anyway: None,
            shown: Vec::new(),
            hidden_ids: HashSet::new(),
            filters: vec![Filter::All],
            filter: Filter::All,
            order: Vec::new(),
            selected: 0,
            query: None,
            search_focused: false,
            saved_position: None,
            spring: Spring::new(0.0, 18.0),
            last_frame: None,
            started: Instant::now(),
            intro_start: None,
            ambient_from: None,
            ambient_to: None,
            crossfade: 1.0,
            motion_dir: 1.0,
            caret_epoch: Instant::now(),
            repaint_pending: false,
            anims: RefCell::new(Anims::new()),
            screen: Screen::Home,
            settings_leaving: None,
            modal: Modal::None,
            modal_leaving: None,
            overlay: Overlay::None,
            toast: None,
            quit_hold: None,
            browser_generation: 0,
            lifecycle: Lifecycle::new(Timing::default()),
            session: None,
            last_session_id: 0,
            controller: ControllerStatus::default(),
            xinput_power: None,
            restore_armed,
            hotkey,
            sgdb_key_set: loaded.sgdb_key_set,
            start_at_login: moonglass_platform::start_at_login().unwrap_or(false),
            splash,
            drawn: false,
            loaded: false,
            logo_sizes: Default::default(),
            user_moved: false,
            input: InputMode::Keyboard,
            wheel_accum: 0.0,
        };
        view.handle(UiEvent::Library(loaded.games), window, cx);
        view.startup();
        window.focus(&view.focus);
        view
    }

    // ------------------------------------------------------------ backend

    /// The library as stored is already on screen ([`Loaded`]); this brings
    /// it up to date behind the startup animation: scan, artwork, then file
    /// watching.
    fn startup(&self) {
        let db = self.backend.db.clone();
        let tx = self.tx.clone();
        let cache = self.backend.paths.cache_dir.clone();
        let rt = self.backend.rt.clone();
        self.backend.rt.spawn(async move {
            let send = |db: Db, tx: UnboundedSender<UiEvent>| async move {
                if let Ok(g) = db.games().await {
                    let _ = tx.unbounded_send(UiEvent::Library(g));
                }
            };
            if let Err(e) = crate::library::scan_all(&db, now_unix()).await {
                tracing::error!("scan failed: {e:#}");
            }
            send(db.clone(), tx.clone()).await;
            if let Err(e) = crate::artwork::refresh(&db, &cache, false).await {
                tracing::error!("artwork failed: {e:#}");
            }
            send(db.clone(), tx.clone()).await;

            // Watchers: debounced rescan + artwork + reload.
            if let Ok(targets) = crate::watch::watch_targets(&db).await {
                let (wtx, mut wrx) = unbounded::<()>();
                match crate::watch::start(targets, move || {
                    let _ = wtx.unbounded_send(());
                }) {
                    Ok(watcher) => {
                        tracing::info!(folders = watcher.watched.len(), "watching library folders");
                        rt.spawn(async move {
                            let _keep = watcher;
                            while wrx.next().await.is_some() {
                                let _ = crate::library::scan_all(&db, now_unix()).await;
                                let _ = crate::artwork::refresh(&db, &cache, false).await;
                                send(db.clone(), tx.clone()).await;
                            }
                        });
                    }
                    Err(e) => tracing::warn!("file watching unavailable: {e:#}"),
                }
            }
        });
    }

    /// Runs a database task, then shows its message (if any) and reloads
    /// the library.
    fn db_task<F, Fut>(&self, f: F)
    where
        F: FnOnce(Db, PathBuf) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<Option<String>>> + Send + 'static,
    {
        let db = self.backend.db.clone();
        let cache = self.backend.paths.cache_dir.clone();
        let tx = self.tx.clone();
        self.backend.rt.spawn(async move {
            match f(db.clone(), cache).await {
                Ok(Some(msg)) => {
                    let _ = tx.unbounded_send(UiEvent::Toast(msg));
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::error!("{e:#}");
                    let _ = tx.unbounded_send(UiEvent::Toast(format!("Something went wrong: {e}")));
                }
            }
            if let Ok(g) = db.games().await {
                let _ = tx.unbounded_send(UiEvent::Library(g));
            }
            if let Ok(e) = emulators::load(&db).await {
                let _ = tx.unbounded_send(UiEvent::Emulators(e));
            }
        });
    }

    fn save_prefs(&self) {
        let db = self.backend.db.clone();
        let p = self.prefs;
        self.backend.rt.spawn(async move {
            if let Err(e) = db.set_setting(prefs::PREFS_KEY, &p).await {
                tracing::error!(error = %e, "saving preferences");
            }
        });
    }

    fn toast(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        self.toast = Some((msg.into(), Instant::now()));
        cx.notify();
    }

    // -------------------------------------------------------------- state

    fn current(&self) -> Option<&Game> {
        self.order.get(self.selected).map(|&i| &self.shown[i])
    }

    /// The stored record of a game (accurate `is_hidden`).
    fn stored(&self, id: &GameId) -> Option<&Game> {
        self.games.iter().find(|g| &g.id == id)
    }

    fn recompute(&mut self, keep_id: Option<String>) {
        let now = now_unix();
        let recent = self.prefs.recent_window();
        self.hidden_ids = self
            .games
            .iter()
            .filter(|g| g.is_hidden)
            .map(|g| g.id.clone())
            .collect();
        self.shown = if self.prefs.show_hidden {
            self.games
                .iter()
                .cloned()
                .map(|mut g| {
                    g.is_hidden = false;
                    g
                })
                .collect()
        } else {
            self.games.clone()
        };
        // Steam games the signed-in account cannot play.
        if self.prefs.steam_playable_only {
            self.shown.retain(|g| self.steam.can_play(g));
        }
        self.filters = visible_filters(&self.shown, now, recent);
        if !self.filters.contains(&self.filter) {
            self.filter = Filter::All;
        }
        self.order = match &self.query {
            Some(q) => search(&self.shown, q),
            None => carousel(&self.shown, self.filter, now, recent),
        };
        let pos = keep_id
            .and_then(|id| {
                self.order
                    .iter()
                    .position(|&i| self.shown[i].id.as_str() == id)
            })
            .unwrap_or(0);
        self.select(pos, true);
    }

    fn select(&mut self, index: usize, snap: bool) {
        let index = index.min(self.order.len().saturating_sub(1));
        if index != self.selected {
            self.motion_dir = if index > self.selected { 1.0 } else { -1.0 };
        }
        self.selected = index;
        if snap || !self.prefs.animate() {
            self.spring.snap(self.selected as f32);
        } else {
            self.spring.target = self.selected as f32;
        }
        let level = self.prefs.blur_level();
        let next = self.current().map(|g| Ambient {
            background: g
                .hero_path
                .as_ref()
                .map(|h| blurred_hero_path(&PathBuf::from(h), level)),
            fanart: g.hero_path.as_ref().map(PathBuf::from),
            motion: g
                .hero_path
                .as_ref()
                .map(|h| motion_hero_path(&PathBuf::from(h))),
            dominant: hex_color(g.dominant_color.as_deref(), 0x2A3550),
            accent: hex_color(g.accent_color.as_deref(), 0x5A78C8),
        });
        if next != self.ambient_to {
            self.ambient_from = self.ambient_to.take();
            self.ambient_to = next;
            self.crossfade = if self.prefs.animate() { 0.0 } else { 1.0 };
        }
    }

    fn change_filter(&mut self, delta: isize) {
        if self.query.is_some() || self.filters.is_empty() {
            return;
        }
        let i = self
            .filters
            .iter()
            .position(|f| *f == self.filter)
            .unwrap_or(0) as isize;
        let n = self.filters.len() as isize;
        self.filter = self.filters[((i + delta).rem_euclid(n)) as usize];
        self.recompute(None);
    }

    fn apply_prefs(&mut self) {
        let keep = self.current().map(|g| g.id.to_string());
        self.recompute(keep);
        // Background blur level may have changed.
        self.ambient_to = None;
        self.select(self.selected, true);
        self.crossfade = 1.0;
    }

    // ------------------------------------------------------------- search

    /// Starts search (remembering the pill and position to come back to).
    fn begin_search(&mut self) {
        if self.query.is_none() {
            self.saved_position = Some((self.filter, self.selected));
            self.query = Some(String::new());
            self.recompute(None);
        }
        self.search_focused = true;
        self.caret_epoch = Instant::now();
    }

    fn set_query(&mut self, q: String) {
        self.query = Some(q);
        self.caret_epoch = Instant::now();
        self.recompute(None);
    }

    /// "Done": focus moves from the field to the results. An empty search
    /// simply ends.
    fn finish_search(&mut self) {
        self.search_focused = false;
        if self.query.as_deref().is_some_and(str::is_empty) {
            self.clear_search();
        }
    }

    fn clear_search(&mut self) {
        self.search_focused = false;
        if self.query.take().is_some() {
            let (filter, pos) = self.saved_position.take().unwrap_or((Filter::All, 0));
            self.filter = filter;
            self.recompute(None);
            self.select(pos, true);
        }
    }

    fn open_search_keyboard(&mut self) {
        self.begin_search();
        let text = self.query.clone().unwrap_or_default();
        self.set_modal(Modal::Keyboard(Osk::new(TextTarget::Search, text)));
    }

    // ------------------------------------------------------------- events

    fn handle(&mut self, ev: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            UiEvent::Library(games) => {
                let keep = self
                    .user_moved
                    .then(|| self.current().map(|g| g.id.to_string()))
                    .flatten();
                let first = !self.loaded;
                self.games = games;
                self.loaded = true;
                self.recompute(keep);
                if let Modal::Options(m) = &mut self.modal {
                    if let Some(g) = self.games.iter().find(|g| g.id == m.game.id) {
                        let sel = m.selected;
                        **m = OptionsMenu::new(g.clone(), std::mem::take(&mut m.joinable));
                        m.selected = sel.min(m.items.len() - 1);
                    }
                }
                if first {
                    if self.prefs.startup_animation && self.prefs.animate() {
                        self.intro_start = Some(Instant::now());
                    }
                    self.started = Instant::now();
                    // Development: start on a given screen.
                    match std::env::var("MOONGLASS_DEV_SCREEN").as_deref() {
                        Ok("settings") => self.open_settings(),
                        Ok("search") => self.open_search_keyboard(),
                        _ => {}
                    }
                }
                cx.notify();
            }
            UiEvent::Emulators(list) => {
                self.emulators_changed(list);
                cx.notify();
            }
            UiEvent::SteamAccounts(accounts) => self.steam_accounts_changed(accounts, cx),
            UiEvent::SteamSwitched(result, accounts) => {
                let name = self.steam_switching.take().unwrap_or_default();
                self.toast(
                    match result {
                        Ok(()) => format!("Signed in to Steam as {name}"),
                        Err(e) => format!("Could not switch to {name}: {e}"),
                    },
                    cx,
                );
                self.steam_accounts_changed(accounts, cx);
                // Friends are per account.
                self.friends_wake.notify_one();
            }
            UiEvent::Friends(snapshot, status) => {
                if self.friends != snapshot || self.friends_status != status {
                    self.friends = snapshot;
                    self.friends_status = status;
                    cx.notify();
                }
            }
            UiEvent::Pad(PadEvent::Status(s)) => {
                self.controller = s;
                cx.notify();
            }
            UiEvent::Power(p) => {
                if p != self.xinput_power {
                    self.xinput_power = p;
                    cx.notify();
                }
            }
            UiEvent::Pad(PadEvent::BackReleased) => self.end_quit_hold(cx),
            UiEvent::Pad(PadEvent::Nav(a)) => {
                // Spec 5.4: dispatch only while focused.
                if window.is_window_active() {
                    self.set_input(InputMode::Gamepad, cx);
                    if a == NavAction::Back {
                        self.start_quit_hold(cx);
                    }
                    self.input_action(a, window, cx);
                }
            }
            UiEvent::Pad(PadEvent::Restore) | UiEvent::Restore => self.restore(window, cx),
            UiEvent::LaunchFailed { session, message } => {
                if self.session.as_ref().is_none_or(|s| s.id != session) {
                    return;
                }
                self.lifecycle.cancel();
                self.end_session();
                self.overlay = Overlay::Message(message);
                cx.notify();
            }
            UiEvent::Observed { session, alive } => {
                let Some(origin) = self
                    .session
                    .as_ref()
                    .filter(|s| s.id == session)
                    .map(|s| s.origin)
                else {
                    return;
                };
                let effects = self.lifecycle.observe(origin.elapsed(), alive);
                for e in effects {
                    self.apply(e, window, cx);
                }
            }
            UiEvent::Toast(msg) => self.toast(msg, cx),
            UiEvent::Dir {
                generation,
                dir,
                result,
            } => {
                if let Modal::Browser(b) = &mut self.modal {
                    if generation == self.browser_generation {
                        b.loading = false;
                        b.dir = dir;
                        b.selected = 0;
                        match result {
                            Ok(e) => {
                                b.entries = e;
                                b.error = None;
                            }
                            Err(e) => {
                                b.entries.clear();
                                b.error = Some(e);
                            }
                        }
                        cx.notify();
                    }
                }
            }
            UiEvent::Tick => self.tick(window, cx),
        }
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .backend
            .instance
            .as_ref()
            .is_some_and(InstanceGuard::take_activation_request)
        {
            window.activate_window();
        }
        if let Some((_, at)) = &self.toast {
            // Past its time the toast fades out (see `render`), then goes.
            let age = at.elapsed();
            if age > Duration::from_secs(TOAST_SECS + 1) {
                self.toast = None;
            }
            if age > Duration::from_secs(TOAST_SECS) {
                cx.notify();
            }
        }
        if let Overlay::Playing { stop } = &mut self.overlay {
            if let StopState::Requested(at) = *stop {
                if at.elapsed() >= STOP_GRACE {
                    *stop = StopState::CanForce;
                    cx.notify();
                }
            }
        }
        // Clock and "playing for" text: once a minute, only when visible.
        if chrono::Local::now().timestamp() % 60 == 0
            && (self.lifecycle.state() == State::Idle || window.is_window_active())
        {
            cx.notify();
        }
    }

    // -------------------------------------------------------------- input

    fn set_input(&mut self, mode: InputMode, cx: &mut Context<Self>) {
        if self.input != mode {
            self.input = mode;
            cx.notify();
        }
    }

    /// One navigation action, routed to the topmost layer.
    fn input_action(&mut self, a: NavAction, window: &mut Window, cx: &mut Context<Self>) {
        match &mut self.modal {
            Modal::Keyboard(_) => self.osk_nav(a, cx),
            Modal::Browser(b) => {
                let r = b.nav(a);
                self.browser_result(r, cx);
            }
            Modal::Options(m) => {
                if a == NavAction::Back || a == NavAction::Options {
                    self.close_modal(cx);
                } else if let Some(o) = m.nav(a) {
                    self.run_option(o, window, cx);
                }
            }
            Modal::Menu(m) => {
                if a == NavAction::Back {
                    self.close_modal(cx);
                } else if let Some(action) = m.nav(a) {
                    self.run_menu_action(action, cx);
                }
            }
            Modal::None => {
                if !matches!(self.overlay, Overlay::None) {
                    self.overlay_nav(a, window, cx);
                } else if matches!(self.screen, Screen::Settings(_)) {
                    self.settings_nav(a, cx);
                } else {
                    self.home_nav(a, window, cx);
                }
            }
        }
        cx.notify();
    }

    fn home_nav(&mut self, a: NavAction, window: &mut Window, cx: &mut Context<Self>) {
        if !self.lifecycle.accepts_navigation() {
            return;
        }
        self.user_moved = true;
        match a {
            NavAction::Left => self.select(self.selected.saturating_sub(1), false),
            NavAction::Right => self.select(self.selected + 1, false),
            NavAction::Up => {}
            NavAction::Down => {
                if self.search_focused {
                    self.finish_search();
                }
            }
            NavAction::FilterPrev => self.change_filter(-1),
            NavAction::FilterNext => self.change_filter(1),
            NavAction::Confirm => {
                if self.search_focused {
                    self.finish_search();
                } else {
                    self.play(window, cx);
                }
            }
            NavAction::Back => self.clear_search(),
            NavAction::Search => {
                if self.input == InputMode::Gamepad {
                    self.open_search_keyboard();
                } else {
                    self.begin_search();
                }
            }
            NavAction::Options => self.open_options(),
            NavAction::OpenSettings => self.open_settings(),
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Keyboard, cx);
        let k = &ev.keystroke;
        let ctrl = k.modifiers.control || k.modifiers.platform;
        let key = k.key.as_str();

        if ctrl && key == "," {
            return self.input_action(NavAction::OpenSettings, window, cx);
        }
        if ctrl && key == "o" {
            return self.input_action(NavAction::Options, window, cx);
        }
        // Exit. Not while a game is tracked: its playtime would be lost.
        if ctrl && key == "q" && self.session.is_none() {
            return cx.quit();
        }
        if key == "escape" {
            if ev.is_held {
                // Key repeat while holding to exit is not more presses.
                if self.quit_hold.is_some() {
                    return;
                }
            } else {
                self.start_quit_hold(cx);
            }
        }
        let typing = self.text_target_active();
        // With a physical keyboard, Enter finishes the on-screen keyboard
        // rather than pressing its highlighted key.
        if key == "enter" {
            if let Modal::Keyboard(_) = self.modal {
                return self.osk_nav(NavAction::OpenSettings, cx);
            }
        }
        let action = match key {
            "left" => Some(NavAction::Left),
            "right" => Some(NavAction::Right),
            "up" => Some(NavAction::Up),
            "down" => Some(NavAction::Down),
            "enter" => Some(NavAction::Confirm),
            "escape" => Some(NavAction::Back),
            "pageup" => Some(NavAction::FilterPrev),
            "pagedown" => Some(NavAction::FilterNext),
            "tab" if k.modifiers.shift => Some(NavAction::FilterPrev),
            "tab" => Some(NavAction::FilterNext),
            "f11" => {
                window.toggle_fullscreen();
                None
            }
            "backspace" if typing => {
                self.text_backspace(cx);
                None
            }
            "backspace" if matches!(self.modal, Modal::Browser(_)) => Some(NavAction::Back),
            "delete" if matches!(self.overlay, Overlay::Playing { .. }) => {
                self.press_stop(cx);
                None
            }
            "space" if typing => {
                self.text_input(" ", cx);
                None
            }
            "space" => Some(NavAction::Confirm),
            "home" | "end" if self.is_home_idle() => {
                self.user_moved = true;
                let last = self.order.len().saturating_sub(1);
                self.select(if key == "home" { 0 } else { last }, false);
                cx.notify();
                None
            }
            _ => {
                if !ctrl && !k.modifiers.alt {
                    if let Some(ch) = k
                        .key_char
                        .as_deref()
                        .filter(|c| !c.is_empty() && c.chars().all(|c| !c.is_control()))
                    {
                        let ch = ch.to_string();
                        self.text_input(&ch, cx);
                    }
                }
                None
            }
        };
        if let Some(a) = action {
            self.input_action(a, window, cx);
        }
    }

    /// Esc or B went down: on the home screen with nothing to back out of
    /// (no search), holding it exits. Not while a game is tracked, as with
    /// Ctrl+Q.
    fn start_quit_hold(&mut self, cx: &mut Context<Self>) {
        if self.quit_hold.is_none()
            && self.is_home_idle()
            && self.query.is_none()
            && self.session.is_none()
        {
            self.quit_hold = Some(Instant::now());
            cx.notify();
        }
    }

    fn end_quit_hold(&mut self, cx: &mut Context<Self>) {
        if self.quit_hold.take().is_some() {
            cx.notify();
        }
    }

    /// Each frame while held: exits once held long enough, or gives up if
    /// the home screen lost focus or something opened.
    fn check_quit_hold(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(t) = self.quit_hold else { return };
        if !self.is_home_idle() || !window.is_window_active() {
            self.quit_hold = None;
        } else if t.elapsed() >= QUIT_HOLD {
            cx.quit();
        }
    }

    /// 0..=1 once the exit progress is shown, `None` before.
    fn quit_progress(&self) -> Option<f32> {
        let held = self.quit_hold?.elapsed();
        (held >= QUIT_HINT_DELAY).then(|| {
            ((held - QUIT_HINT_DELAY).as_secs_f32() / (QUIT_HOLD - QUIT_HINT_DELAY).as_secs_f32())
                .min(1.0)
        })
    }

    fn steam_accounts_changed(&mut self, accounts: SteamAccounts, cx: &mut Context<Self>) {
        if accounts == self.steam {
            return;
        }
        self.steam = accounts;
        let keep = self.current().map(|g| g.id.to_string());
        self.recompute(keep);
        // An open account list shows the new state.
        if matches!(&self.modal, Modal::Menu(m) if m.steam_accounts) {
            self.modal = Modal::Menu(self.steam_account_menu());
        }
        cx.notify();
    }

    /// The account list: choosing one signs Steam in with it.
    fn steam_account_menu(&self) -> Menu {
        let active = self.steam.active.as_deref();
        let rows: Vec<menu::MenuRow> = self
            .steam
            .accounts
            .iter()
            .map(|a| {
                let mut label = a.persona_name.clone();
                if Some(a.steam_id.as_str()) == active {
                    label.push_str("  ·  signed in");
                } else if !a.remembers_password {
                    label.push_str("  ·  Steam will ask for the password");
                }
                menu::MenuRow::new(
                    label,
                    menu::MenuAction::SwitchSteamAccount(a.steam_id.clone()),
                )
            })
            .collect();
        let selected = self
            .steam
            .accounts
            .iter()
            .position(|a| Some(a.steam_id.as_str()) == active)
            .unwrap_or(0);
        Menu {
            title: "Steam account".into(),
            note: Some(
                "Steam restarts to sign in with the account you choose. \
                 Accounts appear once they have signed in to Steam on this computer."
                    .into(),
            ),
            rows,
            selected,
            armed: false,
            emulator: None,
            steam_accounts: true,
        }
    }

    pub(super) fn open_steam_accounts(&mut self, cx: &mut Context<Self>) {
        if self.steam.accounts.is_empty() {
            return self.toast("No Steam accounts found on this computer", cx);
        }
        self.set_modal(Modal::Menu(self.steam_account_menu()));
    }

    /// Signs Steam in with another account, in the background.
    pub(super) fn switch_steam_account(&mut self, steam_id: &str, cx: &mut Context<Self>) {
        let Some(account) = self
            .steam
            .accounts
            .iter()
            .find(|a| a.steam_id == steam_id)
            .cloned()
        else {
            return;
        };
        self.set_modal(Modal::None);
        if self.steam.active.as_deref() == Some(steam_id) || self.steam_switching.is_some() {
            return;
        }
        if self.session.is_some() {
            return self.toast("Close the game first: switching restarts Steam", cx);
        }
        self.steam_switching = Some(account.persona_name.clone());
        self.toast(
            format!(
                "Switching Steam to {}… Steam restarts",
                account.persona_name
            ),
            cx,
        );
        let db = self.backend.db.clone();
        let tx = self.tx.clone();
        self.backend.rt.spawn(async move {
            let root = crate::library::steam_root(&db).await.ok().flatten();
            let (result, accounts) = match root {
                Some(root) => tokio::task::spawn_blocking(move || {
                    crate::steam_accounts::switch(&root, &account)
                })
                .await
                .unwrap_or_else(|e| (Err(e.to_string()), SteamAccounts::default())),
                None => (Err("Steam was not found".into()), SteamAccounts::default()),
            };
            let _ = tx.unbounded_send(UiEvent::SteamSwitched(result, accounts));
        });
    }

    fn is_home_idle(&self) -> bool {
        matches!(self.modal, Modal::None)
            && matches!(self.overlay, Overlay::None)
            && matches!(self.screen, Screen::Home)
            && self.lifecycle.accepts_navigation()
    }

    /// Whether typed characters go into a text field right now.
    fn text_target_active(&self) -> bool {
        matches!(self.modal, Modal::Keyboard(_)) || (self.is_home_idle() && self.search_focused)
    }

    fn text_input(&mut self, s: &str, cx: &mut Context<Self>) {
        match &mut self.modal {
            Modal::Keyboard(k) => {
                let r = k.type_text(s);
                self.osk_result(r, cx);
            }
            Modal::Browser(b) => {
                if let Some(c) = s.chars().next() {
                    b.jump_to(c);
                }
            }
            Modal::Options(_) | Modal::Menu(_) => {}
            Modal::None => {
                // Typing on a physical keyboard searches (spec 3, Search).
                if self.is_home_idle() {
                    self.begin_search();
                    let q = format!("{}{s}", self.query.clone().unwrap_or_default());
                    self.set_query(q);
                }
            }
        }
        cx.notify();
    }

    fn text_backspace(&mut self, cx: &mut Context<Self>) {
        match &mut self.modal {
            Modal::Keyboard(k) => {
                let r = k.backspace();
                self.osk_result(r, cx);
            }
            _ => {
                if let Some(mut q) = self.query.clone() {
                    q.pop();
                    self.set_query(q);
                }
            }
        }
        cx.notify();
    }

    // -------------------------------------------------------------- mouse

    fn on_mouse_move(&mut self, _: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
    }

    /// The wheel (vertical or horizontal) steps through cards.
    fn on_wheel(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        if !self.is_home_idle() {
            return;
        }
        // A mouse wheel reports lines (Windows sends 3 per notch by
        // default): one notch moves one card. Touchpads report pixels.
        let (dx, dy) = match ev.delta {
            gpui::ScrollDelta::Lines(l) => {
                let per_line = WHEEL_STEP_PX / 3.0;
                (l.x * per_line, l.y * per_line)
            }
            gpui::ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
        };
        // Wheel down / right moves to the next card.
        let v = if dx.abs() > dy.abs() { -dx } else { -dy };
        self.wheel_accum += v;
        let mut moved = false;
        while self.wheel_accum >= WHEEL_STEP_PX {
            self.wheel_accum -= WHEEL_STEP_PX;
            self.select(self.selected + 1, false);
            moved = true;
        }
        while self.wheel_accum <= -WHEEL_STEP_PX {
            self.wheel_accum += WHEEL_STEP_PX;
            self.select(self.selected.saturating_sub(1), false);
            moved = true;
        }
        if moved {
            self.user_moved = true;
            cx.notify();
        }
    }

    fn click_card(&mut self, slot: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        if !self.is_home_idle() {
            return;
        }
        self.user_moved = true;
        self.search_focused = false;
        if self.selected == slot {
            self.play(window, cx);
        } else {
            self.select(slot, false);
        }
        cx.notify();
    }

    fn right_click_card(&mut self, slot: usize, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        if !self.is_home_idle() {
            return;
        }
        self.user_moved = true;
        self.select(slot, false);
        self.open_options();
        cx.notify();
    }

    fn click_filter(&mut self, f: Filter, cx: &mut Context<Self>) {
        if !self.is_home_idle() {
            return;
        }
        self.clear_search();
        self.filter = f;
        self.user_moved = true;
        self.recompute(None);
        cx.notify();
    }

    fn click_search_box(&mut self, cx: &mut Context<Self>) {
        if !self.is_home_idle() {
            return;
        }
        self.begin_search();
        cx.notify();
    }

    fn hint_action(&mut self, h: Hint, window: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        let a = match h {
            Hint::Play => NavAction::Confirm,
            Hint::Back => NavAction::Back,
            Hint::Search => NavAction::Search,
            Hint::Options => NavAction::Options,
            Hint::Filter => NavAction::FilterNext,
            Hint::Settings => NavAction::OpenSettings,
            // A click has nothing to hold: it exits at once.
            Hint::Exit => {
                if self.session.is_none() {
                    cx.quit();
                }
                return;
            }
        };
        self.input_action(a, window, cx);
    }

    // ------------------------------------------------------------- modals

    /// Shows `modal` (or none). A modal that closes is kept while it fades
    /// out; one that replaces another kind animates in afresh.
    fn set_modal(&mut self, modal: Modal) {
        let old = std::mem::replace(&mut self.modal, modal);
        match (&old, &self.modal) {
            (Modal::None, Modal::None) => {}
            (_, Modal::None) => self.modal_leaving = Some(old),
            (old, new) => {
                self.modal_leaving = None;
                if std::mem::discriminant(old) != std::mem::discriminant(new) {
                    self.anims.borrow_mut().set("modal", 0.0);
                }
            }
        }
    }

    /// An animated value gliding toward `target` (see [`anim`]).
    fn glide(&self, key: impl std::hash::Hash, target: f32) -> f32 {
        self.anims.borrow_mut().to(key, target)
    }

    /// 0 to 1 from the first frame `key` is drawn.
    fn appear(&self, key: impl std::hash::Hash) -> f32 {
        self.anims.borrow_mut().glide_from(key, 0.0, 1.0)
    }

    fn close_modal(&mut self, cx: &mut Context<Self>) {
        if let Modal::Keyboard(k) = &self.modal {
            if k.target == TextTarget::Search {
                self.finish_search();
            }
        }
        self.set_modal(Modal::None);
        cx.notify();
    }

    fn browser_mut(&mut self) -> Option<&mut Browser> {
        match &mut self.modal {
            Modal::Browser(b) => Some(b),
            _ => None,
        }
    }

    fn osk_nav(&mut self, a: NavAction, cx: &mut Context<Self>) {
        if let Modal::Keyboard(k) = &mut self.modal {
            let r = k.nav(a);
            self.osk_result(r, cx);
        }
    }

    fn osk_click(&mut self, row: usize, col: usize, _: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        if let Modal::Keyboard(k) = &mut self.modal {
            let r = k.click(row, col);
            self.osk_result(r, cx);
        }
    }

    fn osk_result(&mut self, r: OskResult, cx: &mut Context<Self>) {
        let Modal::Keyboard(k) = &self.modal else {
            return;
        };
        let target = k.target.clone();
        let text = k.text.clone();
        match (r, &target) {
            (OskResult::Edited, TextTarget::Search) => self.set_query(text),
            (OskResult::Edited, _) => self.caret_epoch = Instant::now(),
            (OskResult::Done, TextTarget::Search) => {
                self.set_modal(Modal::None);
                self.finish_search();
            }
            (OskResult::Cancel, TextTarget::Search) => {
                self.set_modal(Modal::None);
                self.clear_search();
            }
            (OskResult::Cancel, _) => {
                self.set_modal(Modal::None);
                // Back to the emulator's menu the field was opened from.
                if let Some(id) = target.emulator() {
                    self.open_emulator_menu(id);
                }
            }
            (OskResult::Done, _) => {
                self.set_modal(Modal::None);
                self.apply_text(target, text);
            }
            (OskResult::Nothing, _) => {}
        }
        cx.notify();
    }

    /// Text entered for a field other than search.
    fn apply_text(&mut self, target: TextTarget, text: String) {
        let text = text.trim().to_string();
        match target {
            TextTarget::Search => {}
            TextTarget::EmulatorArguments(ref id) => self.save_emulator_arguments(id, text),
            TextTarget::EmulatorFileTypes(ref id) => self.save_emulator_file_types(id, text),
            TextTarget::RomPlatform {
                ref emulator,
                ref path,
            } => self.add_rom_folder(emulator, path, text),
            TextTarget::SteamApiKey => {
                let wake = self.friends_wake.clone();
                self.db_task(move |db, _| async move {
                    db.set_setting(friends::SETTING_STEAM_API_KEY, &text)
                        .await?;
                    // Asks Steam now rather than at the next minute.
                    wake.notify_one();
                    Ok(Some(if text.is_empty() {
                        "Steam key removed".to_string()
                    } else {
                        "Steam key saved".to_string()
                    }))
                });
            }
            TextTarget::SgdbKey => {
                self.sgdb_key_set = !text.is_empty();
                self.db_task(move |db, cache| async move {
                    db.set_setting(crate::artwork::SETTING_SGDB_KEY, &text)
                        .await?;
                    if text.is_empty() {
                        return Ok(Some("SteamGridDB key removed".to_string()));
                    }
                    let s = crate::artwork::refresh(&db, &cache, true).await?;
                    Ok(Some(format!(
                        "SteamGridDB key saved · {} covers from SteamGridDB",
                        s.from_sgdb
                    )))
                });
            }
            TextTarget::LaunchArgs(ref id) | TextTarget::WatchProcess(ref id) => {
                let Some(g) = self.stored(id).cloned() else {
                    return;
                };
                let is_args = matches!(target, TextTarget::LaunchArgs(_));
                if is_args {
                    if let Err(e) = moonglass_core::template::split_args(&text) {
                        self.toast =
                            Some((format!("Launch arguments not saved: {e}"), Instant::now()));
                        return;
                    }
                }
                let value = (!text.is_empty()).then_some(text);
                let (args, watch) = if is_args {
                    (value, g.watch_process.clone())
                } else {
                    (g.launch_args.clone(), value)
                };
                self.db_task(move |db, _| async move {
                    db.set_launch_options(
                        &g.id,
                        args.as_deref(),
                        g.working_dir.as_deref(),
                        watch.as_deref(),
                        g.track_by_folder,
                    )
                    .await?;
                    Ok(Some(format!("Saved for {}", g.title)))
                });
            }
        }
    }

    fn open_browser(&mut self, purpose: Purpose) {
        self.set_modal(Modal::Browser(Browser::new(purpose)));
        self.load_dir(None);
    }

    fn load_dir(&mut self, dir: Option<PathBuf>) {
        let Modal::Browser(b) = &mut self.modal else {
            return;
        };
        b.loading = true;
        self.browser_generation += 1;
        let generation = self.browser_generation;
        let purpose = b.purpose.clone();
        let tx = self.tx.clone();
        self.backend.rt.spawn(async move {
            let d = dir.clone();
            let result =
                tokio::task::spawn_blocking(move || browser::read_entries(&purpose, d.as_deref()))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
            let _ = tx.unbounded_send(UiEvent::Dir {
                generation,
                dir,
                result,
            });
        });
    }

    fn browser_click(&mut self, i: usize, _: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        if let Some(b) = self.browser_mut() {
            let r = b.click(i);
            self.browser_result(r, cx);
        }
        cx.notify();
    }

    fn browser_result(&mut self, r: BrowserResult, cx: &mut Context<Self>) {
        match r {
            BrowserResult::Nothing => {}
            BrowserResult::Close => self.close_modal(cx),
            BrowserResult::Open(dir) => self.load_dir(dir),
            BrowserResult::Picked(path) => {
                let Modal::Browser(b) = &self.modal else {
                    return;
                };
                let purpose = b.purpose.clone();
                self.set_modal(Modal::None);
                self.picked(purpose, path);
            }
        }
        cx.notify();
    }

    fn picked(&mut self, purpose: Purpose, path: PathBuf) {
        match purpose {
            Purpose::AddExecutable => {
                self.db_task(move |db, cache| async move {
                    let title = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let normalized = moonglass_platform::normalize_path(&path);
                    db.add_executable(&normalized, &path.to_string_lossy(), &title, now_unix())
                        .await?;
                    let _ = crate::artwork::refresh(&db, &cache, false).await;
                    Ok(Some(format!("Added {title}")))
                });
            }
            Purpose::Artwork(id) => {
                let Some(g) = self.stored(&id).cloned() else {
                    return;
                };
                self.db_task(move |db, cache| async move {
                    crate::artwork::set_override(&db, &cache, &g, &path).await?;
                    Ok(Some(format!("New artwork for {}", g.title)))
                });
            }
            Purpose::EmulatorProgram { preset, emulator } => {
                self.picked_emulator_program(preset, emulator, path)
            }
            Purpose::RomFolder(emulator) => self.picked_rom_folder(emulator, path),
            Purpose::WorkingDir(id) => {
                let Some(g) = self.stored(&id).cloned() else {
                    return;
                };
                self.db_task(move |db, _| async move {
                    let dir = path.to_string_lossy().into_owned();
                    db.set_launch_options(
                        &g.id,
                        g.launch_args.as_deref(),
                        Some(&dir),
                        g.watch_process.as_deref(),
                        g.track_by_folder,
                    )
                    .await?;
                    Ok(Some(format!("Working directory set for {}", g.title)))
                });
            }
        }
    }

    fn open_options(&mut self) {
        let Some(id) = self.current().map(|g| g.id.clone()) else {
            return;
        };
        if let Some(g) = self.stored(&id).cloned() {
            let joinable: Vec<Friend> = self
                .friends
                .playing(&g)
                .iter()
                .filter(|f| f.lobby.is_some())
                .cloned()
                .collect();
            self.set_modal(Modal::Options(Box::new(OptionsMenu::new(g, joinable))));
        }
    }

    fn options_click(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        let Modal::Options(m) = &mut self.modal else {
            return;
        };
        m.selected = i;
        if let Some(o) = m.items.get(i).copied() {
            self.run_option(o, window, cx);
        }
        cx.notify();
    }

    fn run_option(&mut self, o: Opt, window: &mut Window, cx: &mut Context<Self>) {
        let Modal::Options(m) = &mut self.modal else {
            return;
        };
        let g = m.game.clone();
        match o {
            Opt::Play => {
                self.set_modal(Modal::None);
                self.play(window, cx);
            }
            Opt::Join(i) => {
                let join = m.joinable.get(i).and_then(|f| {
                    Some(SteamJoin {
                        lobby: f.lobby.clone()?,
                        friend: f.steam_id.clone(),
                    })
                });
                self.set_modal(Modal::None);
                if join.is_some() {
                    self.start_game(join, window, cx);
                }
            }
            Opt::Favorite => {
                let fav = !g.is_favorite;
                self.db_task(move |db, _| async move {
                    db.set_favorite(&g.id, fav).await?;
                    Ok(None)
                });
            }
            Opt::Hide => {
                let hide = !g.is_hidden;
                self.set_modal(Modal::None);
                self.db_task(move |db, _| async move {
                    db.set_hidden(&g.id, hide).await?;
                    Ok(Some(if hide {
                        format!("{} hidden · Settings → Library shows hidden games", g.title)
                    } else {
                        format!("{} is back in the library", g.title)
                    }))
                });
            }
            Opt::ChangeArt => self.open_browser(Purpose::Artwork(g.id)),
            Opt::ResetArt => {
                self.set_modal(Modal::None);
                self.db_task(move |db, cache| async move {
                    db.unlock_artwork(&g.id).await?;
                    crate::artwork::refresh(&db, &cache, false).await?;
                    Ok(Some(format!("Automatic artwork for {}", g.title)))
                });
            }
            Opt::LaunchArgs => {
                let text = g.launch_args.clone().unwrap_or_default();
                self.set_modal(Modal::Keyboard(Osk::new(
                    TextTarget::LaunchArgs(g.id),
                    text,
                )));
            }
            Opt::WatchProcess => {
                let text = g.watch_process.clone().unwrap_or_default();
                self.set_modal(Modal::Keyboard(Osk::new(
                    TextTarget::WatchProcess(g.id),
                    text,
                )));
            }
            Opt::WorkingDir => self.open_browser(Purpose::WorkingDir(g.id)),
            Opt::TrackByFolder => {
                self.db_task(move |db, _| async move {
                    db.set_launch_options(
                        &g.id,
                        g.launch_args.as_deref(),
                        g.working_dir.as_deref(),
                        g.watch_process.as_deref(),
                        !g.track_by_folder,
                    )
                    .await?;
                    Ok(None)
                });
            }
            Opt::Remove => {
                if !m.confirm_remove {
                    m.confirm_remove = true;
                } else {
                    self.set_modal(Modal::None);
                    self.db_task(move |db, _| async move {
                        db.remove_game(&g.id).await?;
                        Ok(Some(format!("Removed {}", g.title)))
                    });
                }
            }
        }
        cx.notify();
    }

    // ----------------------------------------------------------- settings

    fn open_settings(&mut self) {
        self.search_focused = false;
        self.start_at_login = moonglass_platform::start_at_login().unwrap_or(false);
        self.screen = Screen::Settings(SettingsState::new(self.emulators.len()));
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if let Screen::Settings(st) = std::mem::replace(&mut self.screen, Screen::Home) {
            self.settings_leaving = Some(st);
        }
        cx.notify();
    }

    fn settings_info(&self) -> SettingsInfo {
        let installed = self.games.iter().filter(|g| g.is_installed);
        SettingsInfo {
            games: installed.clone().count(),
            hidden: installed.filter(|g| g.is_hidden).count(),
            sgdb_key_set: self.sgdb_key_set,
            steam_key_set: self.friends_status != friends::Status::Off,
            steam_account: match (&self.steam_switching, self.steam.active_account()) {
                (Some(to), _) => format!("Switching to {to}…"),
                (None, Some(a)) if self.steam.can_switch() => {
                    format!("{} · change…", a.persona_name)
                }
                (None, Some(a)) => a.persona_name.clone(),
                (None, None) if self.steam.accounts.is_empty() => "Not found".into(),
                (None, None) => "Not signed in · choose…".into(),
            },
            friends: self.friends_status.text(),
            start_at_login: self.start_at_login,
            emulators: self
                .emulators
                .iter()
                .map(|e| (e.emulator.name.clone(), e.summary()))
                .collect(),
            family: self.controller.family,
            keyboard_hotkey: self.hotkey.availability().keyboard,
            logs: self.backend.paths.log_dir.display().to_string(),
        }
    }

    fn settings_nav(&mut self, a: NavAction, cx: &mut Context<Self>) {
        let Screen::Settings(st) = &mut self.screen else {
            return;
        };
        let cmd = st.nav(a, &mut self.prefs);
        self.settings_cmd(cmd, cx);
    }

    fn settings_click_item(&mut self, i: usize, _: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        let Screen::Settings(st) = &mut self.screen else {
            return;
        };
        let cmd = st.click_item(i, &mut self.prefs);
        self.settings_cmd(cmd, cx);
        cx.notify();
    }

    fn settings_click_section(&mut self, s: usize, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        if let Screen::Settings(st) = &mut self.screen {
            st.click_section(s);
        }
        cx.notify();
    }

    fn settings_cmd(&mut self, cmd: SettingsCmd, cx: &mut Context<Self>) {
        match cmd {
            SettingsCmd::None => {}
            SettingsCmd::Close => self.close_settings(cx),
            SettingsCmd::PrefsChanged => {
                self.apply_prefs();
                self.save_prefs();
            }
            SettingsCmd::ToggleStartAtLogin => {
                let want = !self.start_at_login;
                match moonglass_platform::set_start_at_login(want) {
                    Ok(()) => self.start_at_login = want,
                    Err(e) => self.toast(format!("Couldn't change start at login: {e}"), cx),
                }
            }
            SettingsCmd::Run(item) => match item {
                Item::AddGame => self.open_browser(Purpose::AddExecutable),
                Item::SteamAccount => self.open_steam_accounts(cx),
                Item::SteamKey => {
                    self.set_modal(Modal::Keyboard(Osk::new(
                        TextTarget::SteamApiKey,
                        String::new(),
                    )));
                }
                Item::SgdbKey => {
                    self.set_modal(Modal::Keyboard(Osk::new(
                        TextTarget::SgdbKey,
                        String::new(),
                    )));
                }
                Item::Rescan => {
                    self.toast("Rescanning…", cx);
                    self.db_task(|db, cache| async move {
                        let s = crate::library::scan_all(&db, now_unix()).await?;
                        crate::artwork::refresh(&db, &cache, false).await?;
                        Ok(Some(s.lines.join(" · ")))
                    });
                }
                Item::Exit => cx.quit(),
                Item::AddEmulator => self.open_preset_menu(),
                Item::Emulator(i) => {
                    if let Some(id) = self.emulators.get(i).map(|e| e.emulator.id.clone()) {
                        self.open_emulator_menu(&id);
                    }
                }
                Item::RefreshArt => {
                    self.toast("Refreshing artwork…", cx);
                    self.db_task(|db, cache| async move {
                        let s = crate::artwork::refresh(&db, &cache, true).await?;
                        Ok(Some(s.to_string()))
                    });
                }
                _ => {}
            },
        }
        cx.notify();
    }

    // ------------------------------------------------------------- launch

    fn play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_game(None, window, cx);
    }

    /// Starts the focused game, or joins a friend in it.
    fn start_game(&mut self, join: Option<SteamJoin>, window: &mut Window, cx: &mut Context<Self>) {
        // One game at a time: a second launch would orphan the first.
        if self.session.is_some() || !self.lifecycle.accepts_navigation() {
            return;
        }
        let Some(game) = self.current().cloned() else {
            return;
        };
        // A borrowed game whose owner is playing it: say so once, and let a
        // second press go ahead (the family may have another copy).
        if let Some(owner) = self.friends.in_use_by(&game) {
            if self.play_anyway.as_ref() != Some(&game.id) {
                let msg = format!(
                    "{} is playing {} on their copy · press again to start anyway",
                    owner.name, game.title
                );
                self.play_anyway = Some(game.id.clone());
                self.toast(msg, cx);
                return;
            }
        }
        self.play_anyway = None;
        self.search_focused = false;
        let origin = Instant::now();
        let effects = self.lifecycle.launch(Duration::ZERO);
        let running = Arc::new(Mutex::new(None::<Running>));
        let stop_polling = Arc::new(AtomicBool::new(false));
        let stop_request = Arc::new(AtomicU8::new(STOP_NONE));
        self.last_session_id += 1;
        let session = self.last_session_id;
        self.session = Some(Session {
            id: session,
            game: game.clone(),
            stop_polling: stop_polling.clone(),
            stop_request: stop_request.clone(),
            origin,
            running_since: None,
        });
        self.restore_armed.store(true, Ordering::Relaxed);
        self.hotkey.set_armed(true);
        for e in effects {
            self.apply(e, window, cx);
        }

        let db = self.backend.db.clone();
        let tx = self.tx.clone();
        self.backend.rt.spawn(async move {
            match launch::start(&db, &game, false, join.as_ref()).await {
                Ok(r) => *running.lock().unwrap() = Some(r),
                Err(e) => {
                    let _ = tx.unbounded_send(UiEvent::LaunchFailed {
                        session,
                        message: format!("Couldn't start {}: {e:#}", game.title),
                    });
                    return;
                }
            }
            poll_game(session, &game, &running, &stop_polling, &stop_request, &tx).await;
        });
    }

    fn apply(&mut self, e: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            Effect::ShowLaunching => {
                self.overlay = Overlay::Launching {
                    still_waiting: false,
                };
            }
            Effect::ShowStillWaiting => {
                self.overlay = Overlay::Launching {
                    still_waiting: true,
                };
            }
            Effect::EnterRunning => {
                if let Some(s) = &mut self.session {
                    s.running_since = Some(Instant::now());
                }
                // Visible whenever the user comes back to Moonglass.
                self.overlay = Overlay::Playing {
                    stop: StopState::Idle,
                };
                moonglass_platform::window::hide_for_game(raw_window(window));
            }
            Effect::LaunchAbandoned { reason } => {
                let title = self.session_title();
                self.end_session();
                self.overlay = match reason {
                    AbandonReason::Cancelled => Overlay::None,
                    AbandonReason::TimedOut => Overlay::Message(format!("{title} didn't start.")),
                };
            }
            Effect::GameEnded { session } => {
                let game = self.session.as_ref().map(|s| s.game.clone());
                self.end_session();
                self.overlay = Overlay::None;
                // Before taking focus back: only a launcher window that is
                // in front is sent to the tray.
                if let Some(game) = &game {
                    self.dismiss_launcher(game);
                }
                self.regain_focus(window);
                if let Some(game) = game {
                    let mins = session.as_secs() / 60;
                    if mins > 0 {
                        self.toast = Some((
                            format!("Played {} for {mins} min", game.title),
                            Instant::now(),
                        ));
                    }
                    self.finish_game(game, session);
                }
            }
        }
        cx.notify();
    }

    fn session_title(&self) -> String {
        self.session
            .as_ref()
            .map(|s| s.game.title.clone())
            .unwrap_or_default()
    }

    fn regain_focus(&self, window: &mut Window) {
        match moonglass_platform::window::regain_focus(raw_window(window)) {
            FocusResult::Focused => {}
            FocusResult::UseToolkit => window.activate_window(),
            FocusResult::NotFocused => {
                tracing::warn!("could not regain focus after the game; flashing the taskbar");
            }
        }
        window.focus(&self.focus);
    }

    /// Spec 5.3: after a Steam or Epic game, a client window left in front
    /// goes to the tray (unless turned off in Settings).
    fn dismiss_launcher(&self, game: &Game) {
        if !self.prefs.dismiss_launcher_window {
            return;
        }
        let launcher = match game.source {
            Source::Steam => moonglass_platform::Launcher::Steam,
            Source::Epic => moonglass_platform::Launcher::Epic,
            Source::Exe | Source::Rom => return,
        };
        let closed = moonglass_platform::window::dismiss_launcher_window(launcher);
        tracing::debug!(?launcher, closed, "launcher window dismissal");
    }

    fn finish_game(&self, game: Game, session: Duration) {
        self.db_task(move |db, cache| async move {
            db.record_session(&game.id, now_unix(), session.as_secs() as i64)
                .await?;
            let _ = crate::library::scan_all(&db, now_unix()).await;
            let _ = crate::artwork::refresh(&db, &cache, false).await;
            Ok(None)
        });
    }

    fn end_session(&mut self) {
        if let Some(s) = self.session.take() {
            s.stop_polling.store(true, Ordering::Relaxed);
        }
        self.restore_armed.store(false, Ordering::Relaxed);
        self.hotkey.set_armed(false);
    }

    /// Restore hotkey: bring Moonglass forward over the game.
    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.is_some() && self.lifecycle.restore_hotkey_armed() {
            self.regain_focus(window);
            cx.notify();
        }
    }

    fn overlay_nav(&mut self, a: NavAction, window: &mut Window, cx: &mut Context<Self>) {
        match (&self.overlay, a) {
            (Overlay::Launching { .. }, NavAction::Back) => {
                for e in self.lifecycle.cancel() {
                    self.apply(e, window, cx);
                }
            }
            (Overlay::Playing { .. }, NavAction::Confirm | NavAction::Back) => {
                moonglass_platform::window::hide_for_game(raw_window(window));
            }
            (Overlay::Playing { .. }, NavAction::Search) => self.press_stop(cx),
            (Overlay::Message(_), _) => self.overlay = Overlay::None,
            _ => {}
        }
        cx.notify();
    }

    /// Stop: first a polite close request (the game can save), then, if it
    /// is still running after a few seconds, a forced stop.
    fn press_stop(&mut self, cx: &mut Context<Self>) {
        let Overlay::Playing { stop } = &mut self.overlay else {
            return;
        };
        let Some(s) = &self.session else { return };
        match *stop {
            StopState::Idle => {
                s.stop_request.store(STOP_POLITE, Ordering::Relaxed);
                *stop = StopState::Requested(Instant::now());
            }
            StopState::CanForce => {
                s.stop_request.store(STOP_FORCE, Ordering::Relaxed);
                *stop = StopState::Forcing;
            }
            StopState::Requested(_) | StopState::Forcing => {}
        }
        cx.notify();
    }

    fn overlay_click(&mut self, b: OverlayButton, window: &mut Window, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        match b {
            OverlayButton::Cancel => self.overlay_nav(NavAction::Back, window, cx),
            OverlayButton::ReturnToGame => self.overlay_nav(NavAction::Confirm, window, cx),
            OverlayButton::Stop => self.press_stop(cx),
            OverlayButton::Dismiss => {
                self.overlay = Overlay::None;
                cx.notify();
            }
        }
    }

    // ------------------------------------------------------------ drawing

    fn intro_t(&self) -> Option<f32> {
        let t = self.intro_start?.elapsed().as_secs_f32();
        (t < INTRO_SECS).then_some(t)
    }

    fn caret_on(&self) -> bool {
        (self.caret_epoch.elapsed().as_millis() / CARET_BLINK_MS).is_multiple_of(2)
    }

    /// Requests a repaint after `delay` (one outstanding at a time).
    fn repaint_after(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if self.repaint_pending {
            return;
        }
        self.repaint_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |view, cx| {
                view.repaint_pending = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Start of a frame: moves every animation forward by the time since the
    /// last one.
    fn advance_animation(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        self.spring.step(dt);
        let anims = self.anims.get_mut();
        anims.enabled = self.prefs.animate();
        anims.step(dt);
        if self.crossfade < 1.0 {
            self.crossfade = (self.crossfade + dt / CROSSFADE_SECS).min(1.0);
        }
        if self.intro_start.is_some() && self.intro_t().is_none() {
            self.intro_start = None;
        }
    }

    /// End of a frame: asks for the next one while anything is still moving.
    fn schedule_next_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home_idle =
            self.lifecycle.state() == State::Idle && matches!(self.overlay, Overlay::None);
        // Spec 5.3: the carousel does not animate while launching, and no
        // frames are drawn while a game runs. Overlays and panels settle in
        // a fraction of a second and then stop asking for frames.
        let carousel_moving = home_idle
            && (!self.spring.is_settled() || self.crossfade < 1.0 || self.intro_start.is_some());
        let moving =
            carousel_moving || self.anims.get_mut().is_moving() || self.quit_hold.is_some();
        let pulsing = home_idle
            && matches!(self.screen, Screen::Home)
            && self.prefs.animate()
            && self.prefs.glow_brightness() > 0.0
            && !self.order.is_empty();
        let caret = self.text_target_active() && window.is_window_active();
        if moving {
            window.request_animation_frame();
            return;
        }
        self.last_frame = None;
        if caret {
            let ms = CARET_BLINK_MS - self.caret_epoch.elapsed().as_millis() % CARET_BLINK_MS;
            self.repaint_after(Duration::from_millis(ms as u64 + 5), cx);
        } else if pulsing {
            self.repaint_after(PULSE_FRAME, cx);
        }
    }

    fn pulse(&self) -> f32 {
        if !self.prefs.animate() {
            return 0.5;
        }
        let t = self.started.elapsed().as_secs_f32();
        0.5 + 0.5 * (t * std::f32::consts::TAU / PULSE_PERIOD_SECS).sin()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayButton {
    Cancel,
    ReturnToGame,
    Stop,
    Dismiss,
}

/// Polls the game once per second (spec 5.3) and carries out Stop requests.
async fn poll_game(
    session: u64,
    game: &Game,
    running: &Mutex<Option<Running>>,
    stop_polling: &AtomicBool,
    stop_request: &AtomicU8,
    tx: &UnboundedSender<UiEvent>,
) {
    let mut watcher = ProcessWatcher::new(&[]);
    let mut done_request = STOP_NONE;
    let steam_app = (game.source == Source::Steam)
        .then(|| game.source_id.clone())
        .flatten();
    let mut last_signal: Option<(Option<bool>, bool)> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if stop_polling.load(Ordering::Relaxed) {
            break;
        }
        let req = stop_request.load(Ordering::Relaxed);
        let alive = {
            let mut guard = running.lock().unwrap();
            let Some(r) = guard.as_mut() else { break };
            watcher.refresh();
            if req > done_request {
                done_request = req;
                if req == STOP_POLITE {
                    if let Some(c) = &r.child {
                        moonglass_platform::request_close_pids(&[c.pid()]);
                    }
                    if let Some(m) = &r.matcher {
                        watcher.request_close(m);
                    }
                    tracing::info!(game = %game.title, "asked the game to close");
                } else if req == STOP_FORCE {
                    if let Some(c) = r.child.as_mut() {
                        let _ = c.terminate();
                    }
                    if let Some(m) = &r.matcher {
                        watcher.force_stop(m);
                    }
                    tracing::info!(game = %game.title, "force-stopped the game");
                }
            }
            let child = r.child.as_mut().is_some_and(TrackedChild::is_alive);
            let matched = r.matcher.as_ref().is_some_and(|m| watcher.any_matching(m));
            // Spec 5.3: record Steam's own running flag next to process
            // detection until it is shown to be dependable.
            if let Some(app) = &steam_app {
                let signal = (moonglass_platform::steam_app_running(app), matched);
                if last_signal != Some(signal) {
                    tracing::info!(app, registry = ?signal.0, processes = signal.1, "steam running signal");
                    last_signal = Some(signal);
                }
            }
            child || matched
        };
        if tx
            .unbounded_send(UiEvent::Observed { session, alive })
            .is_err()
        {
            break;
        }
    }
}

impl Render for LauncherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.drawn {
            self.drawn = true;
            crate::startup_mark("first frame");
            // The splash goes once this frame is on screen.
            let splash = self.splash.take();
            window.on_next_frame(move |_, _| drop(splash));
        }
        self.advance_animation();
        self.check_quit_hold(window, cx);
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height));
        let u = h / 1080.0; // 10-foot unit: 1 at 1080p

        let mut root = div()
            .id("root")
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)),
            )
            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.end_quit_hold(cx);
                }
            }))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(gpui::rgb(theme::OBSIDIAN))
            .text_color(theme::white(0.95))
            .text_size(px(22.0 * u));

        // Settings and the launch / Now Playing screens sit on an even
        // blurred backdrop, which the carousel crossfades into.
        let settings_open = matches!(self.screen, Screen::Settings(_));
        let backdrop_wanted = settings_open
            || matches!(
                self.overlay,
                Overlay::Launching { .. } | Overlay::Playing { .. }
            );
        let backdrop = self.glide("backdrop", on(backdrop_wanted));
        if backdrop < 0.997 {
            root = self.render_home(root, w, h, u, cx);
        }
        if backdrop > 0.003 {
            let layer = div()
                .id("backdrop")
                .absolute()
                .size_full()
                .bg(gpui::rgb(theme::OBSIDIAN))
                .opacity(backdrop);
            root = root.child(self.render_backdrop(layer));
        }
        if let Some(o) = self.render_overlay(w, h, u, cx) {
            root = root.child(o);
        }

        let settings = self.glide("settings", on(settings_open));
        if !settings_open && settings < 0.005 {
            self.settings_leaving = None;
        }
        let st = match &self.screen {
            Screen::Settings(st) => Some(st),
            Screen::Home => self.settings_leaving.as_ref(),
        };
        if let Some(st) = st.filter(|_| settings > 0.005) {
            let info = self.settings_info();
            root = root.child(self.render_settings(st, &info, settings, w, h, u, cx));
        }

        let modal_open = !matches!(self.modal, Modal::None);
        let modal = self.glide("modal", on(modal_open));
        if !modal_open && modal < 0.005 {
            self.modal_leaving = None;
        }
        let shown = if modal_open {
            Some(&self.modal)
        } else {
            self.modal_leaving.as_ref()
        };
        let caret = self.caret_on();
        match shown.filter(|_| modal > 0.005) {
            Some(Modal::Keyboard(k)) => root = root.child(self.render_osk(k, u, caret, modal, cx)),
            Some(Modal::Browser(b)) => {
                root = root.child(self.render_browser(b, w, h, u, modal, cx))
            }
            Some(Modal::Options(m)) => {
                let (x, y) = self.options_anchor(w, h, u);
                root = root.child(self.render_options(m, x, y, u, modal, cx));
            }
            Some(Modal::Menu(m)) => root = root.child(self.render_menu(m, w, u, modal, cx)),
            Some(Modal::None) | None => {}
        }

        let toast_due = self
            .toast
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() <= Duration::from_secs(TOAST_SECS));
        let toast = self.glide("toast", on(toast_due));
        if let Some((msg, _)) = self.toast.as_ref().filter(|_| toast > 0.005) {
            root = root.child(
                div()
                    .absolute()
                    .bottom(px((110.0 - 30.0 * (1.0 - toast)) * u))
                    .left_0()
                    .right_0()
                    .opacity(toast)
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .max_w(px(w * 0.7))
                            .px(px(24.0 * u))
                            .py(px(12.0 * u))
                            .rounded_full()
                            .bg(theme::black(0.82))
                            .border_1()
                            .border_color(theme::white(0.14))
                            .text_size(px(18.0 * u))
                            .child(msg.clone()),
                    ),
            );
        }
        let exiting = self.glide("quit-hold", on(self.quit_progress().is_some()));
        if exiting > 0.005 {
            let progress = self.quit_progress().unwrap_or(0.0);
            root = root.child(
                div()
                    .absolute()
                    .bottom(px((110.0 - 30.0 * (1.0 - exiting)) * u))
                    .left_0()
                    .right_0()
                    .opacity(exiting)
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w(px(340.0 * u))
                            .px(px(24.0 * u))
                            .py(px(14.0 * u))
                            .rounded(px(20.0 * u))
                            .bg(theme::black(0.82))
                            .border_1()
                            .border_color(theme::white(0.14))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(10.0 * u))
                            .text_size(px(18.0 * u))
                            .child("Keep holding to exit Moonglass")
                            .child(
                                div()
                                    .w_full()
                                    .h(px(6.0 * u))
                                    .rounded_full()
                                    .bg(theme::white(0.15))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(gpui::relative(progress))
                                            .rounded_full()
                                            .bg(theme::white(0.92)),
                                    ),
                            ),
                    ),
            );
        }
        self.schedule_next_frame(window, cx);
        root
    }
}

/// Opens the fullscreen window and runs until it closes. `start` supplies
/// what was prepared while the toolkit started, which is where most of the
/// startup time goes.
pub fn run(
    start: impl FnOnce() -> anyhow::Result<(Backend, Loaded)>,
    splash: Option<Splash>,
) -> anyhow::Result<()> {
    let app = Application::new().with_assets(icons::Assets);
    let (backend, loaded) = start()?;
    let backend = Arc::new(backend);
    app.run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1600.), px(900.)), cx);
        let windowed = std::env::var_os("MOONGLASS_WINDOWED").is_some();
        let options = WindowOptions {
            window_bounds: Some(if windowed {
                WindowBounds::Windowed(bounds)
            } else {
                WindowBounds::Fullscreen(bounds)
            }),
            titlebar: None,
            app_id: Some("moonglass".into()),
            ..Default::default()
        };
        let opened = cx.open_window(options, move |window, cx| {
            cx.new(|cx| LauncherView::new(backend, loaded, splash, window, cx))
        });
        if let Err(e) = opened {
            tracing::error!("could not open the window: {e:#}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
    Ok(())
}
