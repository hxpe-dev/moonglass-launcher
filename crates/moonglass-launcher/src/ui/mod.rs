//! The GPUI front end: hero carousel, ambient lighting, gamepad navigation
//! and the launch overlay (spec 3, 4.1, 5.3, 5.4).
//!
//! Threading: UI state lives on GPUI's foreground executor. Database,
//! scanning, artwork and process polling run on a small tokio runtime and
//! report back through one channel ([`UiEvent`]), the same bridge pattern as
//! Zed's `gpui_tokio`.

mod gamepad;
pub mod spring;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, rgb, size, App,
    Application, Bounds, BoxShadow, Context, FocusHandle, Hsla, KeyDownEvent, ObjectFit,
    SharedString, Window, WindowBounds, WindowOptions,
};
use moonglass_art::ingest::blurred_hero_path;
use moonglass_core::library::{carousel, search, visible_filters, Filter, RecentWindow};
use moonglass_core::lifecycle::{AbandonReason, Effect, Lifecycle, State, Timing};
use moonglass_core::{Game, Source};
use moonglass_db::Db;
use moonglass_platform::hotkey::RestoreHotkey;
use moonglass_platform::{AppPaths, FocusResult, InstanceGuard, ProcessWatcher, TrackedChild};

use crate::launch::{self, Running};
use gamepad::{ControllerStatus, Family, NavAction, PadEvent};
use spring::Spring;

const ACTIVE_SCALE: f32 = 1.12;
const INACTIVE_OPACITY: f32 = 0.75;
const CROSSFADE_SECS: f32 = 0.35;
const PULSE_PERIOD_SECS: f32 = 3.2;
/// The glow pulse is a slow opacity change; redrawing it at ~30 Hz is
/// indistinguishable from the display rate and costs a fraction of the
/// CPU/GPU. Motion (scrolling, crossfades) still runs at the refresh rate.
const PULSE_FRAME: Duration = Duration::from_millis(33);
/// Cards decoded and kept on each side of the focused one (spec 5.2).
const TEXTURE_NEIGHBORS: usize = 6;
const OBSIDIAN: u32 = 0x07080B;

/// Everything the UI needs from the rest of the app.
pub struct Backend {
    pub db: Db,
    pub rt: tokio::runtime::Handle,
    pub paths: AppPaths,
    pub instance: InstanceGuard,
}

enum UiEvent {
    Library(Vec<Game>),
    Pad(PadEvent),
    Restore,
    LaunchFailed(String),
    Observed(bool),
    Tick,
}

#[derive(Clone, PartialEq)]
struct Ambient {
    background: Option<PathBuf>,
    dominant: Hsla,
    accent: Hsla,
}

enum Overlay {
    None,
    Launching { title: String, still_waiting: bool },
    Restore { title: String, can_force_quit: bool },
    Message(String),
}

struct Session {
    game: Game,
    running: Arc<Mutex<Option<Running>>>,
    stop: Arc<AtomicBool>,
    origin: Instant,
}

/// Display preferences (Settings → Appearance; defaults until Phase 3).
struct Look {
    reduced_motion: bool,
    blur_level: usize,
    glow_brightness: f32,
}

pub struct LauncherView {
    backend: Arc<Backend>,
    tx: UnboundedSender<UiEvent>,
    focus: FocusHandle,

    games: Vec<Game>,
    filters: Vec<Filter>,
    filter: Filter,
    order: Vec<usize>,
    selected: usize,
    query: Option<String>,
    saved_position: Option<(Filter, usize)>,
    recent: RecentWindow,

    spring: Spring,
    last_frame: Option<Instant>,
    started: Instant,
    ambient_from: Option<Ambient>,
    ambient_to: Option<Ambient>,
    crossfade: f32,
    look: Look,

    lifecycle: Lifecycle,
    session: Option<Session>,
    overlay: Overlay,
    controller: ControllerStatus,
    restore_armed: Arc<AtomicBool>,
    hotkey: RestoreHotkey,
    loaded: bool,
    /// Logo sizes, so the logo can be laid out left-aligned at its own
    /// aspect ratio (an image box with Contain would center it).
    logo_sizes: std::collections::HashMap<String, Option<(u32, u32)>>,
    pulse_tick_pending: bool,
    /// Until the user moves, rescans keep focus on the first card (the most
    /// recently played game) instead of following the previous one.
    user_moved: bool,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn hex_color(s: Option<&str>, fallback: u32) -> Hsla {
    let v = s
        .and_then(|s| s.strip_prefix('#'))
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .unwrap_or(fallback);
    rgb(v).into()
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
    fn new(backend: Arc<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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

        // Clock refresh and second-instance activation.
        let tick_tx = tx.clone();
        backend.rt.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if tick_tx.unbounded_send(UiEvent::Tick).is_err() {
                    break;
                }
            }
        });

        let view = Self {
            backend,
            tx,
            focus: cx.focus_handle(),
            games: Vec::new(),
            filters: vec![Filter::All],
            filter: Filter::All,
            order: Vec::new(),
            selected: 0,
            query: None,
            saved_position: None,
            recent: RecentWindow::default(),
            spring: Spring::new(0.0, 18.0),
            last_frame: None,
            started: Instant::now(),
            ambient_from: None,
            ambient_to: None,
            crossfade: 1.0,
            look: Look {
                reduced_motion: false,
                blur_level: 1,
                glow_brightness: 1.0,
            },
            lifecycle: Lifecycle::new(Timing::default()),
            session: None,
            overlay: Overlay::None,
            controller: ControllerStatus::default(),
            restore_armed,
            hotkey,
            loaded: false,
            logo_sizes: Default::default(),
            pulse_tick_pending: false,
            user_moved: false,
        };
        view.startup();
        window.focus(&view.focus);
        view
    }

    // ------------------------------------------------------------ backend

    /// Show what the database has at once (cold start), then scan, then
    /// fill in artwork, then watch for changes.
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
            send(db.clone(), tx.clone()).await;
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

    // -------------------------------------------------------------- state

    fn current(&self) -> Option<&Game> {
        self.order.get(self.selected).map(|&i| &self.games[i])
    }

    fn recompute(&mut self, keep_id: Option<String>) {
        let now = now_unix();
        self.filters = visible_filters(&self.games, now, self.recent);
        if !self.filters.contains(&self.filter) {
            self.filter = Filter::All;
        }
        self.order = match &self.query {
            Some(q) => search(&self.games, q),
            None => carousel(&self.games, self.filter, now, self.recent),
        };
        let pos = keep_id
            .and_then(|id| {
                self.order
                    .iter()
                    .position(|&i| self.games[i].id.as_str() == id)
            })
            .unwrap_or(0);
        self.select(pos, true);
    }

    fn select(&mut self, index: usize, snap: bool) {
        self.selected = index.min(self.order.len().saturating_sub(1));
        if snap || self.look.reduced_motion {
            self.spring.snap(self.selected as f32);
        } else {
            self.spring.target = self.selected as f32;
        }
        let next = self.current().map(|g| Ambient {
            background: g
                .hero_path
                .as_ref()
                .map(|h| blurred_hero_path(&PathBuf::from(h), self.look.blur_level)),
            dominant: hex_color(g.dominant_color.as_deref(), 0x2A3550),
            accent: hex_color(g.accent_color.as_deref(), 0x5A78C8),
        });
        if next != self.ambient_to {
            self.ambient_from = self.ambient_to.take();
            self.ambient_to = next;
            self.crossfade = if self.look.reduced_motion { 1.0 } else { 0.0 };
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

    fn type_search(&mut self, text: &str) {
        if self.query.is_none() {
            self.saved_position = Some((self.filter, self.selected));
        }
        let q = self.query.get_or_insert_with(String::new);
        q.push_str(text);
        self.recompute(None);
    }

    fn clear_search(&mut self) {
        if self.query.take().is_some() {
            let (filter, pos) = self.saved_position.take().unwrap_or((Filter::All, 0));
            self.filter = filter;
            self.recompute(None);
            self.select(pos, true);
        }
    }

    // ------------------------------------------------------------- events

    fn handle(&mut self, ev: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            UiEvent::Library(games) => {
                let keep = self
                    .user_moved
                    .then(|| self.current().map(|g| g.id.to_string()))
                    .flatten();
                self.games = games;
                self.loaded = true;
                self.recompute(keep);
                cx.notify();
            }
            UiEvent::Pad(PadEvent::Status(s)) => {
                self.controller = s;
                cx.notify();
            }
            UiEvent::Pad(PadEvent::Nav(a)) => {
                // Spec 5.4: dispatch only while Idle and focused (overlays
                // still take B / X so a launch can be cancelled).
                if window.is_window_active() {
                    self.nav(a, window, cx);
                }
            }
            UiEvent::Pad(PadEvent::Restore) | UiEvent::Restore => self.restore(window, cx),
            UiEvent::LaunchFailed(msg) => {
                self.lifecycle.cancel();
                self.end_session();
                self.overlay = Overlay::Message(msg);
                cx.notify();
            }
            UiEvent::Observed(alive) => {
                let Some(origin) = self.session.as_ref().map(|s| s.origin) else {
                    return;
                };
                let effects = self.lifecycle.observe(origin.elapsed(), alive);
                for e in effects {
                    self.apply(e, window, cx);
                }
            }
            UiEvent::Tick => {
                if self.backend.instance.take_activation_request() {
                    window.activate_window();
                }
                // Clock: repaint once a minute, and never while a game runs.
                if self.lifecycle.state() == State::Idle
                    && chrono::Local::now().timestamp() % 60 == 0
                {
                    cx.notify();
                }
            }
        }
    }

    fn nav(&mut self, a: NavAction, window: &mut Window, cx: &mut Context<Self>) {
        match (&self.overlay, a) {
            (Overlay::Launching { .. }, NavAction::Back) => {
                for e in self.lifecycle.cancel() {
                    self.apply(e, window, cx);
                }
                return;
            }
            (Overlay::Restore { .. }, NavAction::Back | NavAction::Confirm) => {
                self.overlay = Overlay::None;
                moonglass_platform::window::hide_for_game(raw_window(window));
                cx.notify();
                return;
            }
            (
                Overlay::Restore {
                    can_force_quit: true,
                    ..
                },
                NavAction::Search,
            ) => {
                self.force_quit();
                return;
            }
            (Overlay::Message(_), _) => {
                self.overlay = Overlay::None;
                cx.notify();
                return;
            }
            _ => {}
        }
        if !self.lifecycle.accepts_navigation() {
            return;
        }
        self.user_moved = true;
        match a {
            NavAction::Left => self.select(self.selected.saturating_sub(1), false),
            NavAction::Right => self.select(self.selected + 1, false),
            NavAction::FilterPrev => self.change_filter(-1),
            NavAction::FilterNext => self.change_filter(1),
            NavAction::Confirm => self.play(window, cx),
            NavAction::Back => self.clear_search(),
            NavAction::Search => {
                if self.query.is_none() {
                    self.type_search("");
                }
            }
            NavAction::Up | NavAction::Down | NavAction::Options | NavAction::OpenSettings => {
                // Options and Settings arrive in Phase 3.
            }
        }
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let action = match k.key.as_str() {
            "left" => Some(NavAction::Left),
            "right" => Some(NavAction::Right),
            "up" => Some(NavAction::Up),
            "down" => Some(NavAction::Down),
            "enter" => Some(NavAction::Confirm),
            "escape" => Some(NavAction::Back),
            "pageup" => Some(NavAction::FilterPrev),
            "pagedown" => Some(NavAction::FilterNext),
            "f11" => {
                window.toggle_fullscreen();
                None
            }
            "space" if self.query.is_some() => {
                self.type_search(" ");
                cx.notify();
                None
            }
            "backspace" if self.query.is_some() => {
                if let Some(q) = &mut self.query {
                    q.pop();
                }
                self.recompute(None);
                cx.notify();
                None
            }
            _ => None,
        };
        if let Some(a) = action {
            self.nav(a, window, cx);
            return;
        }
        // Typing on a physical keyboard starts a search (spec 3, Search).
        if self.lifecycle.accepts_navigation() && !k.modifiers.control && !k.modifiers.alt {
            if let Some(ch) = k
                .key_char
                .as_deref()
                .filter(|c| c.chars().all(|c| !c.is_control()))
            {
                if !ch.is_empty() {
                    self.type_search(ch);
                    cx.notify();
                }
            }
        }
    }

    // ------------------------------------------------------------- launch

    fn play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.current().cloned() else {
            return;
        };
        let origin = Instant::now();
        let effects = self.lifecycle.launch(Duration::ZERO);
        let running = Arc::new(Mutex::new(None::<Running>));
        let stop = Arc::new(AtomicBool::new(false));
        self.session = Some(Session {
            game: game.clone(),
            running: running.clone(),
            stop: stop.clone(),
            origin,
        });
        self.restore_armed.store(true, Ordering::Relaxed);
        self.hotkey.set_armed(true);
        for e in effects {
            self.apply(e, window, cx);
        }

        let db = self.backend.db.clone();
        let tx = self.tx.clone();
        self.backend.rt.spawn(async move {
            match launch::start(&db, &game, false).await {
                Ok(r) => *running.lock().unwrap() = Some(r),
                Err(e) => {
                    let _ = tx.unbounded_send(UiEvent::LaunchFailed(format!(
                        "Couldn't start {}: {e:#}",
                        game.title
                    )));
                    return;
                }
            }
            // Poll once per second (spec 5.3).
            let mut watcher = ProcessWatcher::new(&[]);
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let alive = {
                    let mut guard = running.lock().unwrap();
                    let Some(r) = guard.as_mut() else { break };
                    let child = r.child.as_mut().is_some_and(TrackedChild::is_alive);
                    let matched = r.matcher.as_ref().is_some_and(|m| {
                        watcher.refresh();
                        watcher.any_matching(m)
                    });
                    child || matched
                };
                if tx.unbounded_send(UiEvent::Observed(alive)).is_err() {
                    break;
                }
            }
        });
    }

    fn apply(&mut self, e: Effect, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .session
            .as_ref()
            .map(|s| s.game.title.clone())
            .unwrap_or_default();
        match e {
            Effect::ShowLaunching => {
                self.overlay = Overlay::Launching {
                    title,
                    still_waiting: false,
                };
            }
            Effect::ShowStillWaiting => {
                self.overlay = Overlay::Launching {
                    title,
                    still_waiting: true,
                };
            }
            Effect::EnterRunning => {
                self.overlay = Overlay::None;
                moonglass_platform::window::hide_for_game(raw_window(window));
            }
            Effect::LaunchAbandoned { reason } => {
                self.end_session();
                self.overlay = match reason {
                    AbandonReason::Cancelled => Overlay::None,
                    AbandonReason::TimedOut => {
                        Overlay::Message(format!("{title} didn't start. Press any button."))
                    }
                };
            }
            Effect::GameEnded { session } => {
                let game = self.session.as_ref().map(|s| s.game.clone());
                self.end_session();
                self.overlay = Overlay::None;
                self.regain_focus(window);
                if let Some(game) = game {
                    self.finish_game(game, session);
                }
            }
        }
        cx.notify();
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

    fn finish_game(&self, game: Game, session: Duration) {
        let db = self.backend.db.clone();
        let tx = self.tx.clone();
        let cache = self.backend.paths.cache_dir.clone();
        self.backend.rt.spawn(async move {
            let dismiss = db
                .setting::<bool>("dismiss_launcher_window")
                .await
                .ok()
                .flatten()
                .unwrap_or(true);
            let launcher = match game.source {
                Source::Steam => Some(moonglass_platform::Launcher::Steam),
                Source::Epic => Some(moonglass_platform::Launcher::Epic),
                _ => None,
            };
            if let (true, Some(l)) = (dismiss, launcher) {
                moonglass_platform::window::dismiss_launcher_window(l);
            }
            if let Err(e) = db
                .record_session(&game.id, now_unix(), session.as_secs() as i64)
                .await
            {
                tracing::error!(error = %e, "recording session");
            }
            let _ = crate::library::scan_all(&db, now_unix()).await;
            let _ = crate::artwork::refresh(&db, &cache, false).await;
            if let Ok(g) = db.games().await {
                let _ = tx.unbounded_send(UiEvent::Library(g));
            }
        });
    }

    fn end_session(&mut self) {
        if let Some(s) = self.session.take() {
            s.stop.store(true, Ordering::Relaxed);
        }
        self.restore_armed.store(false, Ordering::Relaxed);
        self.hotkey.set_armed(false);
    }

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        if !matches!(
            self.lifecycle.state(),
            State::Launching { .. } | State::Running { .. }
        ) {
            return;
        }
        self.overlay = Overlay::Restore {
            title: s.game.title.clone(),
            // Steam and Epic games are never killed by Moonglass.
            can_force_quit: !s.game.source.is_launcher(),
        };
        self.regain_focus(window);
        cx.notify();
    }

    fn force_quit(&mut self) {
        if let Some(s) = &self.session {
            if let Some(r) = s.running.lock().unwrap().as_mut() {
                if let Some(child) = r.child.as_mut() {
                    if let Err(e) = child.terminate() {
                        tracing::warn!(error = %e, "force quit failed");
                    }
                }
            }
        }
        self.overlay = Overlay::None;
    }

    // ------------------------------------------------------------ drawing

    fn advance_animation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        self.spring.step(dt);
        if self.crossfade < 1.0 {
            self.crossfade = (self.crossfade + dt / CROSSFADE_SECS).min(1.0);
        }
        let idle = self.lifecycle.state() == State::Idle && matches!(self.overlay, Overlay::None);
        let pulsing = idle && !self.look.reduced_motion && !self.order.is_empty();
        let moving = !self.spring.is_settled() || self.crossfade < 1.0;
        // Spec 5.3: no animation while launching; zero frames while running.
        if idle && moving {
            window.request_animation_frame();
        } else if idle && pulsing {
            self.last_frame = None;
            if !self.pulse_tick_pending {
                self.pulse_tick_pending = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(PULSE_FRAME).await;
                    let _ = this.update(cx, |view, cx| {
                        view.pulse_tick_pending = false;
                        cx.notify();
                    });
                })
                .detach();
            }
        } else {
            self.last_frame = None;
        }
    }

    fn pulse(&self) -> f32 {
        if self.look.reduced_motion {
            return 0.5;
        }
        let t = self.started.elapsed().as_secs_f32();
        0.5 + 0.5 * (t * std::f32::consts::TAU / PULSE_PERIOD_SECS).sin()
    }
}

// ---------------------------------------------------------------- render

fn glyphs(f: Family) -> [&'static str; 4] {
    // Confirm, Back, Search, Options.
    match f {
        Family::Xbox => ["A", "B", "X", "Y"],
        Family::PlayStation => ["✕", "○", "□", "△"],
        Family::Nintendo => ["A", "B", "Y", "X"],
    }
}

fn source_label(g: &Game) -> String {
    match g.source {
        Source::Steam => "STEAM".into(),
        Source::Epic => "EPIC".into(),
        Source::Exe => "APP".into(),
        Source::Rom => g
            .platform
            .clone()
            .unwrap_or_else(|| "EMULATOR".into())
            .to_uppercase(),
    }
}

fn is_placeholder_cover(g: &Game) -> bool {
    g.cover_path
        .as_deref()
        .is_none_or(|p| p.ends_with("cover.png"))
}

impl Render for LauncherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.advance_animation(window, cx);
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height));
        let u = h / 1080.0; // 10-foot unit: 1 at 1080p

        let white = |a: f32| gpui::white().opacity(a);
        let mut root = div()
            .id("root")
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)),
            )
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(OBSIDIAN))
            .text_color(white(0.95))
            .text_size(px(22.0 * u));

        // Ambient background: previous and current blurred hero crossfaded,
        // then a two-stop tint from the dominant color to near-black.
        for (amb, opacity) in [
            (self.ambient_from.clone(), 1.0 - self.crossfade),
            (self.ambient_to.clone(), self.crossfade),
        ] {
            let Some(amb) = amb else { continue };
            if opacity <= 0.001 {
                continue;
            }
            if let Some(bg) = amb.background.filter(|p| p.is_file()) {
                root = root.child(
                    img(bg)
                        .absolute()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(opacity * 0.85),
                );
            }
            root = root.child(
                div()
                    .absolute()
                    .size_full()
                    .opacity(opacity)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(amb.dominant.opacity(0.35), 0.),
                        linear_color_stop(rgb(OBSIDIAN), 0.92),
                    )),
            );
        }

        // Top bar.
        let pills = self.filters.iter().map(|f| {
            let active = *f == self.filter && self.query.is_none();
            div()
                .px(px(18.0 * u))
                .py(px(6.0 * u))
                .rounded_full()
                .when(active, |d| d.bg(white(0.92)).text_color(rgb(0x101114)))
                .when(!active, |d| d.text_color(white(0.62)))
                .child(f.label())
        });
        let pad = &self.controller;
        let pad_text = match (pad.connected, pad.battery) {
            (false, _) => "No controller".to_string(),
            (true, Some(b)) => format!("● {b}%{}", if pad.charging { " ⚡" } else { "" }),
            (true, None) => "●".to_string(),
        };
        let clock = chrono::Local::now().format("%H:%M").to_string();
        root = root.child(
            div()
                .absolute()
                .top(px(36.0 * u))
                .left(px(64.0 * u))
                .right(px(64.0 * u))
                .flex()
                .items_center()
                .justify_between()
                .child(div().flex().gap(px(6.0 * u)).children(pills))
                .child(
                    div()
                        .flex()
                        .gap(px(28.0 * u))
                        .text_color(white(0.8))
                        .child(pad_text)
                        .child(clock),
                ),
        );

        // Search field.
        if let Some(q) = &self.query {
            root = root.child(
                div()
                    .absolute()
                    .top(px(110.0 * u))
                    .left(px(64.0 * u))
                    .px(px(20.0 * u))
                    .py(px(10.0 * u))
                    .rounded(px(12.0 * u))
                    .bg(white(0.1))
                    .min_w(px(420.0 * u))
                    .child(format!("Search: {q}▏")),
            );
        }

        // Carousel.
        let card_h = h * 0.44;
        let card_w = card_h * 2.0 / 3.0;
        let gap = card_w * 0.14;
        let left = 64.0 * u + card_w * (ACTIVE_SCALE - 1.0) / 2.0;
        let top = h * 0.40;
        let pulse = self.pulse();

        if !self.loaded {
            // Nothing yet; the first frame stays plain.
        } else if self.games.iter().all(|g| !g.is_visible()) {
            root = root.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(card_w))
                    .h(px(card_h))
                    .rounded(px(18.0 * u))
                    .border_2()
                    .border_color(white(0.5))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(30.0 * u))
                    .child("Add your games"),
            );
        } else if self.order.is_empty() {
            let msg = match &self.query {
                Some(q) => format!("No games match \"{q}\""),
                None => "Nothing here".into(),
            };
            root = root.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top + card_h / 2.0))
                    .text_size(px(32.0 * u))
                    .text_color(white(0.7))
                    .child(msg),
            );
        } else {
            let pos = self.spring.value;
            let lo = self.selected.saturating_sub(TEXTURE_NEIGHBORS);
            let hi = (self.selected + TEXTURE_NEIGHBORS + 1).min(self.order.len());
            for slot in lo..hi {
                let g = &self.games[self.order[slot]];
                let d = slot as f32 - pos;
                let closeness = (1.0 - d.abs()).max(0.0);
                let scale = 1.0 + (ACTIVE_SCALE - 1.0) * closeness;
                let extra = d.clamp(0.0, 1.0) * (ACTIVE_SCALE - 1.0) * card_w;
                let x = left + d * (card_w + gap) + extra - (scale - 1.0) * card_w / 2.0;
                if x > w + card_w || x < -card_w * 2.0 {
                    continue;
                }
                let cw = card_w * scale;
                let ch = card_h * scale;
                let y = top - (ch - card_h) / 2.0;
                let active = slot == self.selected;
                let accent = hex_color(g.accent_color.as_deref(), 0x5A78C8);
                let dominant = hex_color(g.dominant_color.as_deref(), 0x2A3550);

                let mut card = div()
                    .id(("card", slot))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(cw))
                    .h(px(ch))
                    .rounded(px(14.0 * u))
                    .opacity(INACTIVE_OPACITY + (1.0 - INACTIVE_OPACITY) * closeness)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.selected == slot {
                            this.play(window, cx);
                        } else {
                            this.select(slot, false);
                        }
                        cx.notify();
                    }));
                if active {
                    // Glow: stacked box shadows in the accent and dominant
                    // colors; the pulse animates their opacity and spread.
                    let b = self.look.glow_brightness;
                    card = card.border_2().border_color(white(0.92)).shadow(vec![
                        BoxShadow {
                            color: accent.opacity((0.45 + 0.25 * pulse) * b),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(card_w * 0.22),
                            spread_radius: px(card_w * (0.02 + 0.03 * pulse)),
                        },
                        BoxShadow {
                            color: dominant.opacity((0.35 + 0.2 * pulse) * b),
                            offset: point(px(0.), px(card_w * 0.05)),
                            blur_radius: px(card_w * 0.55),
                            spread_radius: px(card_w * (0.06 + 0.06 * pulse)),
                        },
                        BoxShadow {
                            color: accent.opacity(0.18 * b),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(card_w * 0.9),
                            spread_radius: px(card_w * 0.1),
                        },
                    ]);
                }
                let mut inner = div()
                    .size_full()
                    .rounded(px(14.0 * u))
                    .overflow_hidden()
                    .relative();
                if let Some(cover) = g.cover_path.clone() {
                    inner = inner.child(
                        img(PathBuf::from(cover))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    );
                }
                if is_placeholder_cover(g) {
                    inner = inner.child(
                        div()
                            .absolute()
                            .size_full()
                            .p(px(18.0 * u))
                            .flex()
                            .items_end()
                            .text_size(px(26.0 * u))
                            .child(SharedString::from(g.title.clone())),
                    );
                }
                root = root.child(card.child(inner));
            }

            // Hero zone: logo (or title) with a source badge, above the row.
            let logo = self
                .current()
                .and_then(|g| g.logo_path.clone())
                .and_then(|p| {
                    let size = *self
                        .logo_sizes
                        .entry(p.clone())
                        .or_insert_with(|| moonglass_art::dimensions(std::path::Path::new(&p)));
                    size.map(|s| (p, s))
                });
            if let Some(g) = self.current() {
                let mut hero = div()
                    .absolute()
                    .left(px(64.0 * u))
                    .top(px(top - h * 0.24))
                    .h(px(h * 0.20))
                    .flex()
                    .flex_col()
                    .items_start()
                    .justify_end()
                    .gap(px(12.0 * u));
                hero = match logo {
                    Some((l, (lw, lh))) => {
                        // Fit inside 34% width x 15% height, keep aspect.
                        let (max_w, max_h) = (w * 0.34, h * 0.15);
                        let k = (max_w / lw.max(1) as f32).min(max_h / lh.max(1) as f32);
                        hero.child(
                            img(PathBuf::from(l))
                                .w(px(lw as f32 * k))
                                .h(px(lh as f32 * k)),
                        )
                    }
                    None => hero.child(
                        div()
                            .text_size(px(54.0 * u))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(SharedString::from(g.title.clone())),
                    ),
                };
                let mut meta = div().flex().gap(px(14.0 * u)).items_center().child(
                    div()
                        .px(px(10.0 * u))
                        .py(px(3.0 * u))
                        .rounded(px(6.0 * u))
                        .bg(white(0.16))
                        .text_size(px(16.0 * u))
                        .child(source_label(g)),
                );
                if g.playtime_seconds > 0 {
                    let mins = g.playtime_seconds / 60;
                    meta = meta.child(
                        div()
                            .text_size(px(18.0 * u))
                            .text_color(white(0.7))
                            .child(format!("{}h {:02}m in Moonglass", mins / 60, mins % 60)),
                    );
                }
                root = root.child(hero.child(meta));
            }
        }

        // Bottom action bar.
        let [a, b, x, y] = glyphs(self.controller.family);
        let hint = |glyph: &str, label: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(10.0 * u))
                .child(
                    div()
                        .h(px(34.0 * u))
                        .min_w(px(34.0 * u))
                        .px(px(if glyph.chars().count() > 1 {
                            12.0 * u
                        } else {
                            0.0
                        }))
                        .rounded_full()
                        .bg(white(0.9))
                        .text_color(rgb(0x101114))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(18.0 * u))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(glyph.to_string()),
                )
                .child(div().text_color(white(0.85)).child(label.to_string()))
        };
        let back_label = if self.query.is_some() {
            "Clear"
        } else {
            "Back"
        };
        root = root.child(
            div()
                .absolute()
                .bottom(px(40.0 * u))
                .left(px(64.0 * u))
                .right(px(64.0 * u))
                .flex()
                .justify_end()
                .gap(px(34.0 * u))
                .child(hint(a, "Play"))
                .when(self.query.is_some(), |d| d.child(hint(b, back_label)))
                .child(hint(x, "Search"))
                .child(hint(y, "Options"))
                .child(hint("LB RB", "Filter"))
                .child(hint("≡", "Settings")),
        );

        // Overlays.
        let overlay: Option<(String, Vec<String>)> = match &self.overlay {
            Overlay::None => None,
            Overlay::Launching {
                title,
                still_waiting,
            } => Some((
                format!("Launching {title}…"),
                if *still_waiting {
                    vec![format!("Still waiting…   [{b}] Cancel")]
                } else {
                    vec![]
                },
            )),
            Overlay::Restore {
                title,
                can_force_quit,
            } => {
                let mut lines = vec![format!("[{a}] Return to game")];
                if *can_force_quit {
                    lines.push(format!("[{x}] Force quit"));
                }
                Some((title.clone(), lines))
            }
            Overlay::Message(m) => Some((m.clone(), vec![])),
        };
        if let Some((headline, lines)) = overlay {
            root = root.child(
                div()
                    .absolute()
                    .size_full()
                    .bg(gpui::black().opacity(0.82))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(22.0 * u))
                    .child(div().text_size(px(44.0 * u)).child(headline))
                    .children(
                        lines
                            .into_iter()
                            .map(|l| div().text_color(white(0.75)).child(l)),
                    ),
            );
        }

        root
    }
}

/// Opens the fullscreen window and runs until it closes.
pub fn run(backend: Backend) {
    let backend = Arc::new(backend);
    Application::new().run(move |cx: &mut App| {
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
        let b = backend.clone();
        let opened = cx.open_window(options, move |window, cx| {
            cx.new(|cx| LauncherView::new(b.clone(), window, cx))
        });
        if let Err(e) = opened {
            tracing::error!("could not open the window: {e:#}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
