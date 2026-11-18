//! Settings (spec 5.5): a full-screen, controller-navigable view with
//! Appearance, Library, Emulators, Artwork and System sections, drawn as
//! glass panels over the blurred backdrop. Every change applies immediately
//! and is saved.

use std::cell::Cell;

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, Div, Hsla, SharedString};

use super::browser::scroll_to_show;
use super::gamepad::{Family, NavAction};
use super::icons::{icon, Icon};
use moonglass_core::Source;

use super::prefs::{Choice, Prefs};
use super::theme::{black, crop_marks_out, keycap, label, mix, on, selected, white, INK};
use super::LauncherView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Library,
    Emulation,
    Artwork,
    System,
}

pub const SECTIONS: [Section; 5] = [
    Section::Appearance,
    Section::Library,
    Section::Emulation,
    Section::Artwork,
    Section::System,
];

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Library => "Library",
            Section::Emulation => "Emulators",
            Section::Artwork => "Artwork",
            Section::System => "System",
        }
    }
}

/// Every row in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    CardShape,
    CardFit,
    FocusSize,
    Spacing,
    CardDetails,
    Ambient,
    Blur,
    Glow,
    StartupAnimation,
    ReducedMotion,
    Clock,
    AddGame,
    Recent,
    ShowHidden,
    Rescan,
    LibraryInfo,
    /// Whether a store's games are shown (Steam, Epic, GOG).
    Store(Source),
    SteamAccount,
    SteamPlayableOnly,
    SteamFolder,
    SteamIgnored,
    SteamKey,
    FriendsInfo,
    /// An emulator profile, by its position in the list.
    Emulator(usize),
    AddEmulator,
    SgdbKey,
    RefreshArt,
    ArtInfo,
    StartAtLogin,
    DismissLauncher,
    RestoreHotkey,
    RestoreButtons,
    RestoreHold,
    Capture,
    CaptureInfo,
    LaunchTimeout,
    RunSetup,
    Logs,
    Version,
    Exit,
}

/// The rows of a section. `emulators` is how many profiles exist: the
/// Emulators section has one row for each.
pub fn items(section: Section, emulators: usize) -> Vec<Item> {
    let rows: &[Item] = match section {
        Section::Emulation => {
            return (0..emulators)
                .map(Item::Emulator)
                .chain([Item::AddEmulator])
                .collect()
        }
        Section::Appearance => &[
            Item::CardShape,
            Item::CardFit,
            Item::FocusSize,
            Item::Spacing,
            Item::CardDetails,
            Item::Ambient,
            Item::Blur,
            Item::Glow,
            Item::StartupAnimation,
            Item::ReducedMotion,
            Item::Clock,
        ],
        Section::Library => &[
            Item::AddGame,
            Item::Recent,
            Item::ShowHidden,
            Item::Rescan,
            Item::LibraryInfo,
            Item::Store(Source::Steam),
            Item::Store(Source::Epic),
            Item::Store(Source::Gog),
            Item::SteamAccount,
            Item::SteamPlayableOnly,
            Item::SteamFolder,
            Item::SteamIgnored,
            Item::SteamKey,
            Item::FriendsInfo,
        ],
        Section::Artwork => &[Item::SgdbKey, Item::RefreshArt, Item::ArtInfo],
        Section::System => &[
            Item::StartAtLogin,
            Item::DismissLauncher,
            Item::RestoreHotkey,
            Item::RestoreButtons,
            Item::RestoreHold,
            Item::Capture,
            Item::CaptureInfo,
            Item::LaunchTimeout,
            Item::RunSetup,
            Item::Logs,
            Item::Version,
            Item::Exit,
        ],
    };
    rows.to_vec()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Choice,
    Toggle,
    Action,
    Info,
}

impl Item {
    fn kind(self) -> Kind {
        match self {
            Item::CardShape
            | Item::CardFit
            | Item::FocusSize
            | Item::Spacing
            | Item::CardDetails
            | Item::Ambient
            | Item::Blur
            | Item::Glow
            | Item::Clock
            | Item::Recent
            | Item::RestoreButtons
            | Item::RestoreHold
            | Item::Capture
            | Item::LaunchTimeout => Kind::Choice,
            Item::StartupAnimation
            | Item::ReducedMotion
            | Item::ShowHidden
            | Item::SteamPlayableOnly
            | Item::Store(_)
            | Item::StartAtLogin
            | Item::DismissLauncher => Kind::Toggle,
            Item::AddGame
            | Item::Rescan
            | Item::Emulator(_)
            | Item::AddEmulator
            | Item::SteamKey
            | Item::SteamAccount
            | Item::SteamFolder
            | Item::SteamIgnored
            | Item::SgdbKey
            | Item::RefreshArt
            | Item::RunSetup
            | Item::Exit => Kind::Action,
            Item::LibraryInfo
            | Item::FriendsInfo
            | Item::ArtInfo
            | Item::RestoreHotkey
            | Item::CaptureInfo
            | Item::Logs
            | Item::Version => Kind::Info,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Item::CardShape => "Card shape",
            Item::CardFit => "Card art",
            Item::FocusSize => "Focused card size",
            Item::Spacing => "Card spacing",
            Item::CardDetails => "Card details",
            Item::Ambient => "Background",
            Item::Blur => "Background blur",
            Item::Glow => "Glow",
            Item::StartupAnimation => "Startup animation",
            Item::ReducedMotion => "Reduced motion",
            Item::Clock => "Clock",
            Item::AddGame => "Add a game",
            Item::Recent => "Recent shows games played within",
            Item::ShowHidden => "Show hidden games",
            Item::Rescan => "Rescan library now",
            Item::LibraryInfo => "Library",
            Item::Store(s) => match s {
                Source::Steam => "Show Steam games",
                Source::Epic => "Show Epic games",
                _ => "Show GOG games",
            },
            Item::SteamAccount => "Steam account",
            Item::SteamPlayableOnly => "Only Steam games this account can play",
            Item::SteamFolder => "Steam folder",
            Item::SteamIgnored => "Steam apps that are not games",
            Item::SteamKey => "Steam friends: Web API key",
            Item::FriendsInfo => "Friends",
            // Drawn with the profile's own name.
            Item::Emulator(_) => "Emulator",
            Item::AddEmulator => "Add an emulator",
            Item::SgdbKey => "SteamGridDB API key",
            Item::RefreshArt => "Refresh all artwork",
            Item::ArtInfo => "Sources",
            Item::StartAtLogin => "Start Moonglass at login",
            Item::DismissLauncher => "Send Steam / Epic / GOG to the tray after a game",
            Item::RestoreHotkey => "Return to Moonglass during a game",
            Item::RestoreButtons => "Return to Moonglass: buttons",
            Item::RestoreHold => "Return to Moonglass: hold for",
            Item::Capture => "Clips and screenshots",
            Item::CaptureInfo => "Saving clips",
            Item::LaunchTimeout => "Stop waiting for a game after",
            Item::RunSetup => "First-time setup",
            Item::Logs => "Logs",
            Item::Version => "Version",
            Item::Exit => "Exit Moonglass",
        }
    }
}

/// Live values Settings shows that are not preferences.
pub struct SettingsInfo {
    pub games: usize,
    pub hidden: usize,
    pub sgdb_key_set: bool,
    pub steam_key_set: bool,
    /// The signed-in Steam account, as the Steam account row shows it.
    pub steam_account: String,
    /// The Steam folder, as its row shows it.
    pub steam_folder: String,
    /// The Steam apps marked as not games, as their row shows them.
    pub steam_ignored: String,
    /// What the friends feature last reported.
    pub friends: String,
    pub start_at_login: bool,
    /// Name and summary of each emulator profile.
    pub emulators: Vec<(String, String)>,
    pub family: Family,
    pub keyboard_hotkey: bool,
    pub logs: String,
    /// Each store: how many of its games are installed, and whether the
    /// store itself was found on this computer.
    pub stores: Vec<(Source, usize, bool)>,
}

fn store_value(s: Source, info: &SettingsInfo) -> String {
    let (games, found) = info
        .stores
        .iter()
        .find(|(x, ..)| *x == s)
        .map_or((0, false), |&(_, n, f)| (n, f));
    match (games, found) {
        (0, false) => "Not found on this computer".into(),
        (1, _) => "1 game".into(),
        (n, _) => format!("{n} games"),
    }
}

/// Whether a toggle row is on. Most toggles show only "On" / "Off"; a
/// store row shows its number of games instead, next to the switch.
fn toggle_on(item: Item, prefs: &Prefs, value: &str) -> bool {
    match item {
        Item::Store(s) => prefs.store_enabled(s),
        _ => value == "On",
    }
}

/// An on/off switch; `t` goes from 0 (off) to 1 (on). Shared with setup.
pub(super) fn toggle_switch(t: f32, u: f32) -> Div {
    div()
        .flex_none()
        .w(px(52.0 * u))
        .h(px(28.0 * u))
        .rounded_full()
        .p(px(3.0 * u))
        // Monochrome, as on the site: an outline when off, solid when on.
        .border_1()
        .border_color(white(0.45 + 0.55 * t))
        .bg(white(t))
        .child(
            div()
                .ml(px(24.0 * u * t))
                .size(px(20.0 * u))
                .rounded_full()
                .bg(mix(white(0.6), rgb(INK).into(), t)),
        )
}

pub struct SettingsState {
    pub section: usize,
    pub item: usize,
    /// Focus is on the section list rather than the items.
    pub in_sections: bool,
    /// How many emulator profiles exist (rows of the Emulators section).
    emulators: usize,
    /// The first item row shown. How many rows fit is known only when
    /// drawing, so drawing moves it.
    top: Cell<usize>,
}

/// What Settings asks the view to do.
pub enum SettingsCmd {
    None,
    /// A preference changed; save and re-apply.
    PrefsChanged,
    Run(Item),
    ToggleStartAtLogin,
    Close,
}

impl SettingsState {
    /// Opens on the section list, so the first thing to do is choose one.
    pub fn new(emulators: usize) -> Self {
        Self {
            section: 0,
            item: 0,
            in_sections: true,
            emulators,
            top: Cell::new(0),
        }
    }

    pub fn items(&self) -> Vec<Item> {
        items(SECTIONS[self.section], self.emulators)
    }

    pub fn current(&self) -> Item {
        self.items()[self.item]
    }

    /// The list of emulator profiles changed length.
    pub fn set_emulators(&mut self, emulators: usize) {
        self.emulators = emulators;
        self.item = self.item.min(self.items().len() - 1);
    }

    fn set_section(&mut self, s: usize) {
        self.section = s % SECTIONS.len();
        self.item = 0;
        self.top.set(0);
    }

    /// Changes a choice or toggle. Returns whether prefs changed.
    pub fn adjust(item: Item, prefs: &mut Prefs, delta: isize) -> bool {
        match item {
            Item::CardShape => prefs.card_shape = prefs.card_shape.step(delta),
            Item::CardFit => prefs.card_fit = prefs.card_fit.step(delta),
            Item::FocusSize => prefs.focus_size = prefs.focus_size.step(delta),
            Item::Spacing => prefs.spacing = prefs.spacing.step(delta),
            Item::CardDetails => prefs.card_details = prefs.card_details.step(delta),
            Item::Ambient => prefs.ambient = prefs.ambient.step(delta),
            Item::Blur => prefs.blur = prefs.blur.step(delta),
            Item::Glow => prefs.glow = prefs.glow.step(delta),
            Item::Clock => prefs.clock = prefs.clock.step(delta),
            Item::Recent => prefs.recent = prefs.recent.step(delta),
            Item::StartupAnimation => prefs.startup_animation = !prefs.startup_animation,
            Item::ReducedMotion => prefs.reduced_motion = !prefs.reduced_motion,
            Item::ShowHidden => prefs.show_hidden = !prefs.show_hidden,
            Item::RestoreButtons => prefs.restore_buttons = prefs.restore_buttons.step(delta),
            Item::RestoreHold => prefs.restore_hold = prefs.restore_hold.step(delta),
            Item::LaunchTimeout => prefs.launch_timeout = prefs.launch_timeout.step(delta),
            Item::Capture => prefs.capture = prefs.capture.step(delta),
            Item::SteamPlayableOnly => prefs.steam_playable_only = !prefs.steam_playable_only,
            Item::Store(s) => prefs.set_store_enabled(s, !prefs.store_enabled(s)),
            Item::DismissLauncher => prefs.dismiss_launcher_window = !prefs.dismiss_launcher_window,
            _ => return false,
        }
        true
    }

    fn activate(item: Item, prefs: &mut Prefs, delta: isize) -> SettingsCmd {
        match item.kind() {
            Kind::Choice | Kind::Toggle if item == Item::StartAtLogin => {
                SettingsCmd::ToggleStartAtLogin
            }
            Kind::Choice | Kind::Toggle => {
                if Self::adjust(item, prefs, delta) {
                    SettingsCmd::PrefsChanged
                } else {
                    SettingsCmd::None
                }
            }
            Kind::Action => SettingsCmd::Run(item),
            Kind::Info => SettingsCmd::None,
        }
    }

    pub fn nav(&mut self, a: NavAction, prefs: &mut Prefs) -> SettingsCmd {
        let n_items = self.items().len();
        match a {
            NavAction::FilterPrev => self.set_section(self.section + SECTIONS.len() - 1),
            NavAction::FilterNext => self.set_section(self.section + 1),
            NavAction::Back => {
                if self.in_sections {
                    return SettingsCmd::Close;
                }
                self.in_sections = true;
            }
            NavAction::OpenSettings => return SettingsCmd::Close,
            NavAction::Up if self.in_sections => {
                self.set_section(self.section + SECTIONS.len() - 1)
            }
            NavAction::Down if self.in_sections => self.set_section(self.section + 1),
            NavAction::Right | NavAction::Confirm if self.in_sections => self.in_sections = false,
            NavAction::Up => self.item = (self.item + n_items - 1) % n_items,
            NavAction::Down => self.item = (self.item + 1) % n_items,
            NavAction::Left => {
                let item = self.current();
                if matches!(item.kind(), Kind::Choice | Kind::Toggle) {
                    return Self::activate(item, prefs, -1);
                }
                self.in_sections = true;
            }
            NavAction::Right => {
                let item = self.current();
                if matches!(item.kind(), Kind::Choice | Kind::Toggle) {
                    return Self::activate(item, prefs, 1);
                }
            }
            NavAction::Confirm => return Self::activate(self.current(), prefs, 1),
            _ => {}
        }
        SettingsCmd::None
    }

    /// Mouse: click on a row selects it and acts on it.
    pub fn click_item(&mut self, i: usize, prefs: &mut Prefs) -> SettingsCmd {
        self.in_sections = false;
        self.item = i;
        Self::activate(self.current(), prefs, 1)
    }

    pub fn click_section(&mut self, s: usize) {
        self.set_section(s);
        self.in_sections = true;
    }
}

/// What the chosen capture tool needs before the Quick Menu can use it.
fn capture_info(prefs: &Prefs) -> &'static str {
    use moonglass_platform::CaptureTool as T;
    match prefs.capture.tool() {
        None => "Save clip and Screenshot are left out of the Quick Menu",
        Some(T::GameBar) => {
            "Turn on \"Record what happened\" in Windows Settings → Gaming → Captures. Uses Win+Alt+G and Win+Alt+PrtScn"
        }
        Some(T::Nvidia) => {
            "Turn on Instant Replay in the NVIDIA overlay (Alt+Z). Uses Alt+F10 and Alt+F1"
        }
        Some(T::Amd) => {
            "Turn on Instant Replay in AMD Software. Uses Ctrl+Shift+S and Ctrl+Shift+I"
        }
        Some(T::GpuScreenRecorder) => {
            "Run gpu-screen-recorder in replay mode (-r). Moonglass asks it to save; it has no screenshots"
        }
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "On"
    } else {
        "Off"
    }
}

/// Names of the View and Menu buttons on each controller family.
pub fn restore_combo(f: Family, prefs: &Prefs) -> String {
    use super::prefs::RestoreButtons;
    let buttons = match (prefs.restore_buttons, f) {
        (RestoreButtons::ViewMenu, Family::Xbox) => "View + Menu",
        (RestoreButtons::ViewMenu, Family::PlayStation) => "Share (Create) + Options",
        (RestoreButtons::ViewMenu, Family::Nintendo) => "− (Minus) + + (Plus)",
        (RestoreButtons::Sticks, _) => "both sticks pressed in",
    };
    format!("Hold {buttons} for {}", prefs.restore_hold.label())
}

fn key_state(set: bool) -> &'static str {
    if set {
        "Set · change…"
    } else {
        "Not set · enter…"
    }
}

fn value(item: Item, prefs: &Prefs, info: &SettingsInfo) -> String {
    match item {
        Item::CardShape => prefs.card_shape.label().into(),
        Item::CardFit => prefs.card_fit.label().into(),
        Item::FocusSize => prefs.focus_size.label().into(),
        Item::Spacing => prefs.spacing.label().into(),
        Item::CardDetails => prefs.card_details.label().into(),
        Item::Ambient => prefs.ambient.label().into(),
        Item::Blur => prefs.blur.label().into(),
        Item::Glow => prefs.glow.label().into(),
        Item::StartupAnimation => on_off(prefs.startup_animation).into(),
        Item::ReducedMotion => on_off(prefs.reduced_motion).into(),
        Item::Clock => prefs.clock.label().into(),
        Item::AddGame => "Choose an executable…".into(),
        Item::Recent => prefs.recent.label().into(),
        Item::ShowHidden => on_off(prefs.show_hidden).into(),
        Item::SteamAccount => info.steam_account.clone(),
        Item::SteamPlayableOnly => on_off(prefs.steam_playable_only).into(),
        Item::Store(s) => store_value(s, info),
        Item::SteamFolder => info.steam_folder.clone(),
        Item::SteamIgnored => info.steam_ignored.clone(),
        Item::RestoreButtons => prefs.restore_buttons.label().into(),
        Item::RestoreHold => prefs.restore_hold.label().into(),
        Item::LaunchTimeout => prefs.launch_timeout.label().into(),
        Item::Capture => prefs.capture.label().into(),
        Item::CaptureInfo => capture_info(prefs).into(),
        Item::Rescan => "Rescan".into(),
        Item::LibraryInfo => format!("{} games, {} hidden", info.games, info.hidden),
        Item::Emulator(i) => info
            .emulators
            .get(i)
            .map(|(_, summary)| summary.clone())
            .unwrap_or_default(),
        Item::AddEmulator => "Choose…".into(),
        Item::SgdbKey => key_state(info.sgdb_key_set).into(),
        Item::SteamKey => key_state(info.steam_key_set).into(),
        Item::FriendsInfo => info.friends.clone(),
        Item::RefreshArt => "Refresh".into(),
        Item::ArtInfo => {
            "Steam's cache, Epic's catalog, GOG's catalog, SteamGridDB (with a key), Switch and retro game databases, then a placeholder".into()
        }
        Item::StartAtLogin => on_off(info.start_at_login).into(),
        Item::DismissLauncher => on_off(prefs.dismiss_launcher_window).into(),
        Item::RestoreHotkey => {
            if info.keyboard_hotkey {
                format!(
                    "{} · or Ctrl + Alt + Home",
                    restore_combo(info.family, prefs)
                )
            } else {
                restore_combo(info.family, prefs)
            }
        }
        Item::RunSetup => "Open".into(),
        Item::Logs => info.logs.clone(),
        Item::Version => env!("CARGO_PKG_VERSION").into(),
        Item::Exit => "Exit".into(),
    }
}

impl LauncherView {
    /// `shown` (0 to 1) fades the view in and out; the carousel behind it is
    /// already blurred by the caller.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_settings(
        &self,
        st: &SettingsState,
        info: &SettingsInfo,
        shown: f32,
        w: f32,
        h: f32,
        u: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let prefs = &self.prefs;
        let ink: Hsla = rgb(INK).into();
        // A dark glass panel: the blurred artwork shows through. No outline:
        // focus brackets at its corners frame it instead.
        let glass = |d: Div| {
            d.relative()
                .bg(black(0.38))
                .children(crop_marks_out(22.0 * u, 10.0 * u, white(0.45)))
        };

        let mut sections = glass(div())
            .flex_none()
            .w(px(300.0 * u))
            .p(px(12.0 * u))
            .flex()
            .flex_col()
            .gap(px(4.0 * u));
        for (i, s) in SECTIONS.iter().enumerate() {
            let active = i == st.section;
            // The current section is tinted; it turns white while the
            // section list itself has the focus.
            let tint = self.glide(("settings-section", i), on(active));
            let lit = self.glide(("settings-section-focus", i), on(active && st.in_sections));
            sections = sections.child(
                div()
                    .id(("settings-section", i))
                    .px(px(20.0 * u))
                    .py(px(13.0 * u))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(white(0.14 * tint + 0.78 * lit))
                    .text_color(mix(white(0.65 + 0.30 * tint), ink, lit))
                    .when(!active, |d| d.hover(|s| s.bg(white(0.08))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_click_section(i, cx);
                    }))
                    .flex()
                    .items_center()
                    .gap(px(16.0 * u))
                    // Numbered, as the site's sections are.
                    .child(label(format!("{:02}", i + 1), 13.0, u).text_color(mix(
                        white(0.30 + 0.25 * tint),
                        ink,
                        lit,
                    )))
                    .child(s.label()),
            );
        }

        let section = SECTIONS[st.section];
        // As many rows as fit between the heading and the footer. The
        // list scrolls only when the selected row would leave them.
        let items = st.items();
        let fit = (((h - 420.0 * u) / (70.0 * u)).floor() as usize).max(3);
        let start = scroll_to_show(st.top.get(), st.item, fit, items.len());
        st.top.set(start);
        let end = (start + fit).min(items.len());
        let more = |shown: bool| {
            div()
                .h(px(16.0 * u))
                .flex()
                .justify_center()
                .when(shown, |d| {
                    d.child(icon(Icon::ChevronDown, 16.0 * u, white(0.45)))
                })
        };
        let mut rows = div().flex().flex_col().gap(px(4.0 * u));
        if start > 0 {
            rows = rows.child(div().h(px(16.0 * u)).flex().justify_center().child(icon(
                Icon::ChevronUp,
                16.0 * u,
                white(0.45),
            )));
        }
        for (i, item) in items.into_iter().enumerate().take(end).skip(start) {
            let sel = i == st.item && !st.in_sections;
            // Keyed by the row's place on screen, so a scroll at an edge
            // leaves the highlight where it is.
            let lit = self.glide(("settings-row", st.section, i - start), on(sel));
            let kind = item.kind();
            let label = match item {
                Item::Emulator(n) => info
                    .emulators
                    .get(n)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default(),
                _ => item.label().to_string(),
            };
            let v = value(item, prefs, info);
            let value_el = match kind {
                Kind::Choice => {
                    // A new value fades in.
                    let fresh = self.appear(("settings-value", st.section, i, v.as_str()));
                    // The arrows take the row's text color, which follows
                    // the highlight.
                    let arrow = mix(white(0.95), ink, lit).opacity(0.55);
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0 * u))
                        .child(icon(Icon::ChevronLeft, 16.0 * u, arrow))
                        .child(div().opacity(fresh).child(v))
                        .child(icon(Icon::ChevronRight, 16.0 * u, arrow))
                }
                Kind::Toggle => {
                    // The knob slides and the track changes color.
                    let is_on = toggle_on(item, prefs, &v);
                    let t = self.glide(("settings-toggle", st.section, i), on(is_on));
                    let switch = toggle_switch(t, u);
                    match item {
                        // A store's number of games sits beside its switch.
                        Item::Store(_) => div()
                            .flex()
                            .items_center()
                            .gap(px(14.0 * u))
                            .child(
                                div()
                                    .text_color(mix(white(0.6), rgb(0x3A3D45).into(), lit))
                                    .child(v),
                            )
                            .child(switch),
                        _ => switch,
                    }
                }
                Kind::Action => div()
                    .px(px(14.0 * u))
                    .py(px(4.0 * u))
                    .rounded_full()
                    .border_1()
                    .border_color(mix(white(0.45), ink, lit))
                    .text_color(mix(white(0.95), ink, lit))
                    .child(v),
                Kind::Info => div()
                    .text_color(mix(white(0.6), rgb(0x3A3D45).into(), lit))
                    .truncate()
                    .child(v),
            };
            rows = rows.child(
                selected(div().id(("settings-item", i)), lit)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(24.0 * u))
                    .px(px(22.0 * u))
                    .py(px(14.0 * u))
                    .rounded_full()
                    .when(kind != Kind::Info, Styled::cursor_pointer)
                    .when(!sel, |d| d.hover(|s| s.bg(white(0.07))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.settings_click_item(i, window, cx);
                    }))
                    .child(div().flex_none().child(SharedString::from(label)))
                    .child(div().min_w_0().flex().justify_end().child(value_el)),
            );
        }
        // The rows of a newly chosen section slide in from the right.
        let entered = self.appear(("settings-rows", st.section));
        let panel = glass(div())
            .flex_1()
            .min_w_0()
            .p(px(14.0 * u))
            .flex()
            .flex_col()
            .gap(px(8.0 * u))
            .child(
                div()
                    .px(px(22.0 * u))
                    .pt(px(10.0 * u))
                    .pb(px(4.0 * u))
                    .child(label(
                        format!("{:02} / {}", st.section + 1, section.label()),
                        13.0,
                        u,
                    )),
            )
            .child(
                div()
                    .relative()
                    .left(px((1.0 - entered) * 32.0 * u))
                    .opacity(entered)
                    .child(rows.when(end < st.items().len(), |r| r.child(more(true)))),
            );

        let hint = |k: &str, l: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(8.0 * u))
                .child(keycap(k, u))
                .child(super::theme::label(l, 14.0, u).text_color(white(super::theme::TEXT_2)))
        };
        let keyboard = self.input != super::InputMode::Gamepad;
        let footer = if keyboard {
            div()
                .flex()
                .gap(px(26.0 * u))
                .child(hint("↑↓", "Move"))
                .child(hint("←→", "Change"))
                .child(hint("Enter", "Select"))
                .child(hint("Tab", "Section"))
                .child(hint("Esc", "Back"))
        } else {
            div()
                .flex()
                .gap(px(26.0 * u))
                .child(hint("A", "Select"))
                .child(hint("←→", "Change"))
                .child(hint("LB RB", "Section"))
                .child(hint("B", "Back"))
        };

        div()
            .id("settings")
            .occlude()
            .absolute()
            .size_full()
            // A light scrim: the blur behind does most of the separation.
            .bg(black(0.30))
            .opacity(shown)
            .px(px(w * 0.08))
            .py(px(h * 0.07))
            .flex()
            .flex_col()
            .gap(px(26.0 * u))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0 * u))
                            .child(label(
                                concat!("Moonglass · ", env!("CARGO_PKG_VERSION")),
                                13.0,
                                u,
                            ))
                            .child(
                                div()
                                    .text_size(px(44.0 * u))
                                    .font_weight(gpui::FontWeight::LIGHT)
                                    .child("Settings"),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-close")
                            .cursor_pointer()
                            .px(px(20.0 * u))
                            .py(px(8.0 * u))
                            .rounded_full()
                            .border_1()
                            .border_color(white(0.45))
                            .hover(|s| s.bg(white(1.0)).text_color(rgb(INK)))
                            .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx)))
                            .child("Close"),
                    ),
            )
            .child(
                // The panels rise into place as the view opens.
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .top(px((1.0 - shown) * 28.0 * u))
                    .flex()
                    .items_start()
                    // Room between the panels for their corner marks.
                    .gap(px(44.0 * u))
                    .child(sections)
                    .child(panel),
            )
            .child(
                div()
                    .text_size(px(16.0 * u))
                    .text_color(white(0.8))
                    .child(footer),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_change_with_left_right() {
        let mut st = SettingsState::new(0);
        let mut p = Prefs::default();
        st.nav(NavAction::Confirm, &mut p); // into Appearance
        let before = p.card_shape;
        assert!(matches!(
            st.nav(NavAction::Right, &mut p),
            SettingsCmd::PrefsChanged
        ));
        assert_ne!(p.card_shape, before);
        st.nav(NavAction::Left, &mut p);
        assert_eq!(p.card_shape, before);
    }

    #[test]
    fn back_goes_to_sections_then_closes() {
        let mut st = SettingsState::new(0);
        let mut p = Prefs::default();
        assert!(st.in_sections, "opens on the section list");
        st.nav(NavAction::Right, &mut p);
        assert!(!st.in_sections);
        assert!(matches!(st.nav(NavAction::Back, &mut p), SettingsCmd::None));
        assert!(st.in_sections);
        st.nav(NavAction::Down, &mut p);
        assert_eq!(SECTIONS[st.section], Section::Library);
        assert!(matches!(
            st.nav(NavAction::Back, &mut p),
            SettingsCmd::Close
        ));
    }

    #[test]
    fn actions_and_toggles() {
        let mut st = SettingsState::new(0);
        let mut p = Prefs::default();
        st.nav(NavAction::FilterNext, &mut p); // Library
        st.nav(NavAction::Confirm, &mut p); // into its rows
        assert!(matches!(
            st.nav(NavAction::Confirm, &mut p),
            SettingsCmd::Run(Item::AddGame)
        ));
        st.nav(NavAction::Down, &mut p);
        st.nav(NavAction::Down, &mut p); // Show hidden
        st.nav(NavAction::Confirm, &mut p);
        assert!(p.show_hidden);
    }

    #[test]
    fn every_section_has_items() {
        for s in SECTIONS {
            assert!(!items(s, 0).is_empty());
        }
    }

    #[test]
    fn emulators_section_lists_profiles_then_add() {
        assert_eq!(
            items(Section::Emulation, 2),
            [Item::Emulator(0), Item::Emulator(1), Item::AddEmulator]
        );
        let mut st = SettingsState::new(2);
        let mut p = Prefs::default();
        st.nav(NavAction::FilterNext, &mut p);
        st.nav(NavAction::FilterNext, &mut p); // Emulators
        st.nav(NavAction::Confirm, &mut p);
        st.nav(NavAction::Up, &mut p); // wraps to "Add an emulator"
        assert_eq!(st.current(), Item::AddEmulator);
        // A profile was removed while the last row was selected.
        st.set_emulators(0);
        assert_eq!(st.current(), Item::AddEmulator);
    }
}
