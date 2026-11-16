//! Settings (spec 5.5): a full-screen, controller-navigable view with
//! Library, Appearance, Artwork and System sections. Every change applies
//! immediately and is saved.

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};

use super::gamepad::{Family, NavAction};
use super::prefs::{Choice, Prefs};
use super::theme::{black, keycap, white, INK};
use super::LauncherView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Library,
    Artwork,
    System,
}

pub const SECTIONS: [Section; 4] = [
    Section::Appearance,
    Section::Library,
    Section::Artwork,
    Section::System,
];

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Library => "Library",
            Section::Artwork => "Artwork",
            Section::System => "System",
        }
    }
}

/// Every row in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    CardShape,
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
    SgdbKey,
    RefreshArt,
    ArtInfo,
    StartAtLogin,
    DismissLauncher,
    RestoreHotkey,
    Logs,
    Version,
}

pub fn items(section: Section) -> &'static [Item] {
    match section {
        Section::Appearance => &[
            Item::CardShape,
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
        ],
        Section::Artwork => &[Item::SgdbKey, Item::RefreshArt, Item::ArtInfo],
        Section::System => &[
            Item::StartAtLogin,
            Item::DismissLauncher,
            Item::RestoreHotkey,
            Item::Logs,
            Item::Version,
        ],
    }
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
            | Item::FocusSize
            | Item::Spacing
            | Item::CardDetails
            | Item::Ambient
            | Item::Blur
            | Item::Glow
            | Item::Clock
            | Item::Recent => Kind::Choice,
            Item::StartupAnimation
            | Item::ReducedMotion
            | Item::ShowHidden
            | Item::StartAtLogin
            | Item::DismissLauncher => Kind::Toggle,
            Item::AddGame | Item::Rescan | Item::SgdbKey | Item::RefreshArt => Kind::Action,
            Item::LibraryInfo
            | Item::ArtInfo
            | Item::RestoreHotkey
            | Item::Logs
            | Item::Version => Kind::Info,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Item::CardShape => "Card shape",
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
            Item::SgdbKey => "SteamGridDB API key",
            Item::RefreshArt => "Refresh all artwork",
            Item::ArtInfo => "Sources",
            Item::StartAtLogin => "Start Moonglass at login",
            Item::DismissLauncher => "Send Steam / Epic to the tray after a game",
            Item::RestoreHotkey => "Return to Moonglass during a game",
            Item::Logs => "Logs",
            Item::Version => "Version",
        }
    }
}

/// Live values Settings shows that are not preferences.
pub struct SettingsInfo {
    pub games: usize,
    pub hidden: usize,
    pub sgdb_key_set: bool,
    pub start_at_login: bool,
    pub family: Family,
    pub keyboard_hotkey: bool,
    pub logs: String,
}

pub struct SettingsState {
    pub section: usize,
    pub item: usize,
    /// Focus is on the section list rather than the items.
    pub in_sections: bool,
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
    pub fn new() -> Self {
        Self {
            section: 0,
            item: 0,
            in_sections: false,
        }
    }

    pub fn current(&self) -> Item {
        items(SECTIONS[self.section])[self.item]
    }

    fn set_section(&mut self, s: usize) {
        self.section = s % SECTIONS.len();
        self.item = 0;
    }

    /// Changes a choice or toggle. Returns whether prefs changed.
    pub fn adjust(item: Item, prefs: &mut Prefs, delta: isize) -> bool {
        match item {
            Item::CardShape => prefs.card_shape = prefs.card_shape.step(delta),
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
            Item::DismissLauncher => prefs.dismiss_launcher_window = !prefs.dismiss_launcher_window,
            _ => return false,
        }
        true
    }

    fn activate(&self, item: Item, prefs: &mut Prefs, delta: isize) -> SettingsCmd {
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
        let n_items = items(SECTIONS[self.section]).len();
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
                    return self.activate(item, prefs, -1);
                }
                self.in_sections = true;
            }
            NavAction::Right => {
                let item = self.current();
                if matches!(item.kind(), Kind::Choice | Kind::Toggle) {
                    return self.activate(item, prefs, 1);
                }
            }
            NavAction::Confirm => return self.activate(self.current(), prefs, 1),
            _ => {}
        }
        SettingsCmd::None
    }

    /// Mouse: click on a row selects it and acts on it.
    pub fn click_item(&mut self, i: usize, prefs: &mut Prefs) -> SettingsCmd {
        self.in_sections = false;
        self.item = i;
        self.activate(self.current(), prefs, 1)
    }

    pub fn click_section(&mut self, s: usize) {
        self.set_section(s);
        self.in_sections = true;
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
pub fn restore_combo(f: Family) -> &'static str {
    match f {
        Family::Xbox => "Hold View + Menu for 2 seconds",
        Family::PlayStation => "Hold Share (Create) + Options for 2 seconds",
        Family::Nintendo => "Hold − (Minus) + + (Plus) for 2 seconds",
    }
}

fn value(item: Item, prefs: &Prefs, info: &SettingsInfo) -> String {
    match item {
        Item::CardShape => prefs.card_shape.label().into(),
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
        Item::Rescan => "Rescan".into(),
        Item::LibraryInfo => format!("{} games, {} hidden", info.games, info.hidden),
        Item::SgdbKey => if info.sgdb_key_set {
            "Set · change…"
        } else {
            "Not set · enter…"
        }
        .into(),
        Item::RefreshArt => "Refresh".into(),
        Item::ArtInfo => "Steam's local cache, then SteamGridDB, then a placeholder".into(),
        Item::StartAtLogin => on_off(info.start_at_login).into(),
        Item::DismissLauncher => on_off(prefs.dismiss_launcher_window).into(),
        Item::RestoreHotkey => {
            if info.keyboard_hotkey {
                format!("{} · or Ctrl + Alt + Home", restore_combo(info.family))
            } else {
                restore_combo(info.family).to_string()
            }
        }
        Item::Logs => info.logs.clone(),
        Item::Version => env!("CARGO_PKG_VERSION").into(),
    }
}

impl LauncherView {
    pub(super) fn render_settings(
        &self,
        st: &SettingsState,
        info: &SettingsInfo,
        w: f32,
        h: f32,
        u: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let prefs = &self.prefs;
        let mut sections = div().flex().flex_col().gap(px(6.0 * u)).w(px(280.0 * u));
        for (i, s) in SECTIONS.iter().enumerate() {
            let active = i == st.section;
            let focused = active && st.in_sections;
            sections = sections.child(
                div()
                    .id(("settings-section", i))
                    .px(px(20.0 * u))
                    .py(px(12.0 * u))
                    .rounded(px(12.0 * u))
                    .cursor_pointer()
                    .when(focused, |d| d.bg(white(0.92)).text_color(rgb(INK)))
                    .when(active && !focused, |d| d.bg(white(0.14)))
                    .when(!active, |d| {
                        d.text_color(white(0.7)).hover(|s| s.bg(white(0.08)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_click_section(i, cx);
                    }))
                    .child(s.label()),
            );
        }

        let mut rows = div().flex().flex_col().gap(px(6.0 * u)).flex_1();
        for (i, item) in items(SECTIONS[st.section]).iter().copied().enumerate() {
            let sel = i == st.item && !st.in_sections;
            let kind = item.kind();
            let v = value(item, prefs, info);
            let value_el = match kind {
                Kind::Choice => div()
                    .flex()
                    .items_center()
                    .gap(px(10.0 * u))
                    .child(div().opacity(0.6).child("‹"))
                    .child(v)
                    .child(div().opacity(0.6).child("›")),
                Kind::Toggle => {
                    let on = v == "On";
                    div()
                        .w(px(52.0 * u))
                        .h(px(28.0 * u))
                        .rounded_full()
                        .p(px(3.0 * u))
                        .flex()
                        .when(on, |d| d.justify_end().bg(rgb(0x5A8DEE)))
                        .when(!on, |d| d.justify_start().bg(white(0.25)))
                        .child(div().size(px(22.0 * u)).rounded_full().bg(gpui::white()))
                }
                Kind::Action => div()
                    .px(px(14.0 * u))
                    .py(px(4.0 * u))
                    .rounded_full()
                    .when(sel, |d| d.bg(rgb(INK)).text_color(gpui::white()))
                    .when(!sel, |d| d.bg(white(0.14)))
                    .child(v),
                Kind::Info => div()
                    .text_color(if sel {
                        rgb(0x3A3D45).into()
                    } else {
                        white(0.6)
                    })
                    .truncate()
                    .child(v),
            };
            rows = rows.child(
                div()
                    .id(("settings-item", i))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(24.0 * u))
                    .px(px(22.0 * u))
                    .py(px(14.0 * u))
                    .rounded(px(12.0 * u))
                    .when(kind != Kind::Info, |d| d.cursor_pointer())
                    .when(sel, |d| d.bg(white(0.92)).text_color(rgb(INK)))
                    .when(!sel, |d| d.hover(|s| s.bg(white(0.07))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.settings_click_item(i, window, cx);
                    }))
                    .child(div().flex_none().child(SharedString::from(item.label())))
                    .child(div().min_w_0().flex().justify_end().child(value_el)),
            );
        }

        let hint = |k: &str, l: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(8.0 * u))
                .child(keycap(k, u))
                .child(l.to_string())
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
            .bg(black(0.93))
            .px(px(w * 0.06))
            .py(px(h * 0.06))
            .flex()
            .flex_col()
            .gap(px(28.0 * u))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(40.0 * u))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .id("settings-close")
                            .cursor_pointer()
                            .px(px(18.0 * u))
                            .py(px(8.0 * u))
                            .rounded_full()
                            .bg(white(0.12))
                            .hover(|s| s.bg(white(0.24)))
                            .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx)))
                            .child("Close"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .gap(px(36.0 * u))
                    .min_h_0()
                    .child(sections)
                    .child(rows),
            )
            .child(
                div()
                    .text_size(px(16.0 * u))
                    .text_color(white(0.75))
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
        let mut st = SettingsState::new();
        let mut p = Prefs::default();
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
        let mut st = SettingsState::new();
        let mut p = Prefs::default();
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
        let mut st = SettingsState::new();
        let mut p = Prefs::default();
        st.nav(NavAction::FilterNext, &mut p); // Library
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
            assert!(!items(s).is_empty());
        }
    }
}
