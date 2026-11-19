//! A small list menu shown over Settings or setup: the emulator presets, an
//! emulator profile's actions, the Steam accounts, the Steam folder, and
//! the Steam apps that are not games. Rows that remove something ask for a
//! second press.

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};

use super::gamepad::NavAction;
use super::theme::{black, keycap, on, selected, white};
use super::LauncherView;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// Add an emulator from the preset with this index.
    AddEmulator(usize),
    ChangeProgram,
    EditArguments,
    EditFileTypes,
    AddRomFolder,
    /// By ROM directory ID.
    RemoveRomFolder(String),
    RemoveEmulator,
    /// Sign Steam in with this account (SteamID64).
    SwitchSteamAccount(String),
    ChooseSteamFolder,
    /// Forget the chosen Steam folder and find it again.
    AutoSteamFolder,
    /// Count this Steam app as a game again.
    UnignoreSteamApp(u32),
    ChooseMusicFolder,
    NextTrack,
    MusicOff,
}

pub struct MenuRow {
    pub label: String,
    pub action: MenuAction,
    /// Removes something: asks for a second press.
    pub danger: bool,
}

impl MenuRow {
    pub fn new(label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            action,
            danger: false,
        }
    }

    pub fn danger(label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            action,
            danger: true,
        }
    }
}

pub struct Menu {
    pub title: String,
    /// One line of help under the title.
    pub note: Option<String>,
    pub rows: Vec<MenuRow>,
    pub selected: usize,
    /// The selected danger row was pressed once.
    pub armed: bool,
    /// The emulator profile these rows act on, if any.
    pub emulator: Option<String>,
    /// The Steam account list, refreshed when the accounts change.
    pub steam_accounts: bool,
}

impl Menu {
    fn activate(&mut self, i: usize) -> Option<MenuAction> {
        let row = self.rows.get(i)?;
        if row.danger && !self.armed {
            self.armed = true;
            return None;
        }
        Some(row.action.clone())
    }

    /// Returns the action to run, if any.
    pub fn nav(&mut self, a: NavAction) -> Option<MenuAction> {
        let n = self.rows.len().max(1);
        match a {
            NavAction::Up => {
                self.selected = (self.selected + n - 1) % n;
                self.armed = false;
            }
            NavAction::Down => {
                self.selected = (self.selected + 1) % n;
                self.armed = false;
            }
            NavAction::Confirm => return self.activate(self.selected),
            _ => {}
        }
        None
    }

    /// Mouse: a click on a row selects it and acts on it.
    pub fn click(&mut self, i: usize) -> Option<MenuAction> {
        if self.selected != i {
            self.selected = i;
            self.armed = false;
        }
        self.activate(i)
    }
}

impl LauncherView {
    pub(super) fn render_menu(
        &self,
        m: &Menu,
        w: f32,
        u: f32,
        shown: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_col().gap(px(4.0 * u));
        for (i, row) in m.rows.iter().enumerate() {
            let sel = i == m.selected;
            let lit = self.glide(("menu-row", i), on(sel));
            let label = if row.danger && sel && m.armed {
                "Press again to confirm".to_string()
            } else {
                row.label.clone()
            };
            list = list.child(
                selected(div().id(("menu-row", i)), lit)
                    .px(px(18.0 * u))
                    .py(px(11.0 * u))
                    .rounded_full()
                    .cursor_pointer()
                    .when(!sel, |d| d.hover(|s| s.bg(white(0.08))))
                    .when(row.danger && !sel, |d| d.text_color(rgb(0xF08A88)))
                    .truncate()
                    .on_click(cx.listener(move |this, _, _, cx| this.menu_click(i, cx)))
                    .child(SharedString::from(label)),
            );
        }

        let keyboard = self.input != super::InputMode::Gamepad;
        let (k_ok, k_back) = if keyboard {
            ("Enter", "Esc")
        } else {
            ("A", "B")
        };
        let hint = |k: &str, l: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(8.0 * u))
                .child(keycap(k, u))
                .child(super::theme::label(l, 14.0, u).text_color(white(super::theme::TEXT_2)))
        };

        let mut panel = div()
            .id("menu")
            .occlude()
            .relative()
            .top(px((1.0 - shown) * 30.0 * u))
            .w(px((w * 0.5).clamp(560.0 * u, 900.0 * u)))
            .p(px(30.0 * u))
            .bg(rgb(0x000000))
            .border_1()
            .border_color(white(super::theme::LINE))
            .children(super::theme::crop_marks(18.0 * u, white(0.4)))
            .flex()
            .flex_col()
            .gap(px(14.0 * u))
            // Clicks inside the menu must not close it.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(34.0 * u))
                    .font_weight(gpui::FontWeight::LIGHT)
                    .child(SharedString::from(m.title.clone())),
            );
        if let Some(note) = &m.note {
            panel = panel.child(
                div()
                    .text_size(px(16.0 * u))
                    .text_color(white(super::theme::TEXT_2))
                    .child(SharedString::from(note.clone())),
            );
        }
        panel = panel.child(list).child(
            div()
                .flex()
                .gap(px(26.0 * u))
                .text_size(px(16.0 * u))
                .text_color(white(0.75))
                .child(hint(k_ok, "Select"))
                .child(hint(k_back, "Close")),
        );

        div()
            .id("menu-scrim")
            .occlude()
            .absolute()
            .size_full()
            .bg(black(0.6))
            .opacity(shown)
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, _, _, cx| this.close_modal(cx)))
            .child(panel)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        Menu {
            title: "Dolphin".into(),
            note: None,
            rows: vec![
                MenuRow::new("Program", MenuAction::ChangeProgram),
                MenuRow::danger("Remove", MenuAction::RemoveEmulator),
            ],
            selected: 0,
            armed: false,
            emulator: Some("dolphin".into()),
            steam_accounts: false,
        }
    }

    #[test]
    fn removing_needs_a_second_press() {
        let mut m = menu();
        assert_eq!(m.nav(NavAction::Confirm), Some(MenuAction::ChangeProgram));
        m.nav(NavAction::Down);
        assert_eq!(m.nav(NavAction::Confirm), None);
        assert!(m.armed);
        assert_eq!(m.nav(NavAction::Confirm), Some(MenuAction::RemoveEmulator));
    }

    #[test]
    fn moving_away_disarms() {
        let mut m = menu();
        assert_eq!(m.click(1), None);
        m.nav(NavAction::Up);
        m.nav(NavAction::Down);
        assert_eq!(m.nav(NavAction::Confirm), None, "asks again");
    }
}
