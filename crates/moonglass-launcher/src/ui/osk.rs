//! On-screen keyboard (spec 5.5): used for search and the few text fields
//! (SteamGridDB key, launch arguments, process to watch). A physical
//! keyboard types into the same fields.
//!
//! Gamepad: D-pad moves, A types the key, X deletes, Y types a space,
//! Menu (Start) is Done, B cancels.

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};
use moonglass_core::GameId;

use super::gamepad::NavAction;
use super::theme::{keycap, white, INK};
use super::LauncherView;

const ROWS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm,.-"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    Shift,
    Space,
    Delete,
    Done,
}

const SPECIALS: [Special; 4] = [
    Special::Shift,
    Special::Space,
    Special::Delete,
    Special::Done,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextTarget {
    Search,
    SgdbKey,
    LaunchArgs(GameId),
    WatchProcess(GameId),
}

impl TextTarget {
    pub fn label(&self) -> &'static str {
        match self {
            TextTarget::Search => "Search",
            TextTarget::SgdbKey => "SteamGridDB API key",
            TextTarget::LaunchArgs(_) => "Launch arguments",
            TextTarget::WatchProcess(_) => "Process to watch (for example Game.exe)",
        }
    }

    fn secret(&self) -> bool {
        matches!(self, TextTarget::SgdbKey)
    }
}

pub struct Osk {
    pub target: TextTarget,
    pub text: String,
    row: usize,
    col: usize,
    shift: bool,
}

/// What the view should do after a keyboard action.
pub enum OskResult {
    /// Text changed (search filters live).
    Edited,
    Done,
    Cancel,
    Nothing,
}

impl Osk {
    pub fn new(target: TextTarget, text: String) -> Self {
        Self {
            target,
            text,
            row: 1,
            col: 0,
            shift: false,
        }
    }

    fn row_len(row: usize) -> usize {
        if row < ROWS.len() {
            ROWS[row].chars().count()
        } else {
            SPECIALS.len()
        }
    }

    fn key_char(&self, row: usize, col: usize) -> Option<char> {
        let c = ROWS.get(row)?.chars().nth(col)?;
        Some(if self.shift {
            c.to_ascii_uppercase()
        } else {
            c
        })
    }

    pub fn type_text(&mut self, s: &str) -> OskResult {
        self.text.push_str(s);
        OskResult::Edited
    }

    pub fn backspace(&mut self) -> OskResult {
        if self.text.pop().is_some() {
            OskResult::Edited
        } else {
            OskResult::Nothing
        }
    }

    fn press(&mut self, row: usize, col: usize) -> OskResult {
        if let Some(c) = self.key_char(row, col) {
            self.text.push(c);
            if self.shift {
                self.shift = false;
            }
            return OskResult::Edited;
        }
        match SPECIALS.get(col) {
            Some(Special::Shift) => {
                self.shift = !self.shift;
                OskResult::Nothing
            }
            Some(Special::Space) => self.type_text(" "),
            Some(Special::Delete) => self.backspace(),
            Some(Special::Done) => OskResult::Done,
            None => OskResult::Nothing,
        }
    }

    pub fn nav(&mut self, a: NavAction) -> OskResult {
        let rows = ROWS.len() + 1;
        match a {
            NavAction::Left => {
                let n = Self::row_len(self.row);
                self.col = (self.col + n - 1) % n;
            }
            NavAction::Right => {
                let n = Self::row_len(self.row);
                self.col = (self.col + 1) % n;
            }
            NavAction::Up | NavAction::Down => {
                let old = Self::row_len(self.row) as f32;
                let frac = (self.col as f32 + 0.5) / old;
                self.row = if a == NavAction::Up {
                    (self.row + rows - 1) % rows
                } else {
                    (self.row + 1) % rows
                };
                let n = Self::row_len(self.row);
                self.col = ((frac * n as f32) as usize).min(n - 1);
            }
            NavAction::Confirm => return self.press(self.row, self.col),
            NavAction::Search => return self.backspace(),
            NavAction::Options => return self.type_text(" "),
            NavAction::OpenSettings => return OskResult::Done,
            NavAction::Back => return OskResult::Cancel,
            NavAction::FilterPrev | NavAction::FilterNext => {}
        }
        OskResult::Nothing
    }

    /// Mouse: a click on a key.
    pub fn click(&mut self, row: usize, col: usize) -> OskResult {
        self.row = row;
        self.col = col;
        self.press(row, col)
    }

    pub fn shown_text(&self) -> String {
        if self.target.secret() && !self.text.is_empty() {
            let tail: String = self
                .text
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!(
                "{}{tail}",
                "•".repeat(self.text.chars().count().saturating_sub(4))
            )
        } else {
            self.text.clone()
        }
    }
}

impl LauncherView {
    /// The keyboard as a bottom sheet. `caret` is the blink state.
    pub(super) fn render_osk(
        &self,
        osk: &Osk,
        u: f32,
        caret: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key_w = 64.0 * u;
        let key_h = 56.0 * u;
        let selected = |r: usize, c: usize| osk.row == r && osk.col == c;

        let key = |id: (&'static str, usize), label: String, w: f32, sel: bool| {
            div()
                .id(id)
                .w(px(w))
                .h(px(key_h))
                .rounded(px(10.0 * u))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(sel, |d| d.bg(white(0.92)).text_color(rgb(INK)))
                .when(!sel, |d| d.bg(white(0.10)).hover(|s| s.bg(white(0.2))))
                .child(label)
        };

        let mut rows = div().flex().flex_col().gap(px(8.0 * u)).items_center();
        for (r, keys) in ROWS.iter().enumerate() {
            let mut row = div().flex().gap(px(8.0 * u));
            for (c, ch) in keys.chars().enumerate() {
                let ch = if osk.shift {
                    ch.to_ascii_uppercase()
                } else {
                    ch
                };
                row = row.child(
                    key(
                        ("osk-key", r * 100 + c),
                        ch.to_string(),
                        key_w,
                        selected(r, c),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.osk_click(r, c, window, cx)),
                    ),
                );
            }
            rows = rows.child(row);
        }
        let mut specials = div().flex().gap(px(8.0 * u));
        for (c, sp) in SPECIALS.iter().enumerate() {
            let (label, w) = match sp {
                Special::Shift => (if osk.shift { "SHIFT" } else { "Shift" }, key_w * 2.0),
                Special::Space => ("Space", key_w * 4.0),
                Special::Delete => ("Delete", key_w * 2.0),
                Special::Done => ("Done", key_w * 2.0),
            };
            let r = ROWS.len();
            specials = specials.child(
                key(("osk-special", c), label.to_string(), w, selected(r, c)).on_click(
                    cx.listener(move |this, _, window, cx| this.osk_click(r, c, window, cx)),
                ),
            );
        }
        rows = rows.child(specials);

        let field = div()
            .w(px(key_w * 10.0 + 72.0 * u))
            .px(px(18.0 * u))
            .py(px(10.0 * u))
            .rounded(px(10.0 * u))
            .bg(white(0.12))
            .flex()
            .items_center()
            .child(
                div()
                    .when(osk.text.is_empty(), |d| d.text_color(white(0.5)))
                    .child(SharedString::from(if osk.text.is_empty() {
                        osk.target.label().to_string()
                    } else {
                        osk.shown_text()
                    })),
            )
            .child(
                div()
                    .ml(px(2.0 * u))
                    .w(px(2.0 * u))
                    .h(px(24.0 * u))
                    .when(caret, |d| d.bg(white(0.95))),
            );

        let keys: &[(&str, &str)] = if self.input == super::InputMode::Gamepad {
            &[
                ("A", "Type"),
                ("X", "Delete"),
                ("Y", "Space"),
                ("≡", "Done"),
                ("B", "Cancel"),
            ]
        } else {
            &[
                ("Type", "with your keyboard or click"),
                ("Enter", "Done"),
                ("Esc", "Cancel"),
            ]
        };
        let hints = div()
            .flex()
            .gap(px(26.0 * u))
            .text_size(px(16.0 * u))
            .text_color(white(0.75))
            .children(keys.iter().map(|(k, l)| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0 * u))
                    .child(keycap(k, u))
                    .child(l.to_string())
            }));

        div()
            .id("osk")
            .occlude()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .pt(px(22.0 * u))
            .pb(px(26.0 * u))
            .bg(super::theme::black(0.86))
            .border_t_1()
            .border_color(white(0.12))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(16.0 * u))
            .when(osk.target != TextTarget::Search, |d| {
                d.child(div().text_color(white(0.7)).child(osk.target.label()))
                    .child(field)
            })
            .child(rows)
            .child(hints)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_navigation() {
        let mut k = Osk::new(TextTarget::Search, String::new());
        // Starts on "q".
        assert!(matches!(k.nav(NavAction::Confirm), OskResult::Edited));
        assert_eq!(k.text, "q");
        k.nav(NavAction::Right);
        k.nav(NavAction::Confirm);
        assert_eq!(k.text, "qw");
        assert!(matches!(k.nav(NavAction::Search), OskResult::Edited));
        assert_eq!(k.text, "q");
        k.nav(NavAction::Options);
        assert_eq!(k.text, "q ");
        // Wraps from the top row to the specials row.
        k.nav(NavAction::Up);
        k.nav(NavAction::Up);
        assert_eq!(k.row, ROWS.len());
        assert!(matches!(k.nav(NavAction::Back), OskResult::Cancel));
        assert!(matches!(k.nav(NavAction::OpenSettings), OskResult::Done));
    }

    #[test]
    fn shift_applies_once() {
        let mut k = Osk::new(TextTarget::Search, String::new());
        k.click(ROWS.len(), 0); // Shift
        k.click(1, 0);
        k.click(1, 0);
        assert_eq!(k.text, "Qq");
    }

    #[test]
    fn secrets_are_masked() {
        let k = Osk::new(TextTarget::SgdbKey, "abcdef123456".into());
        assert_eq!(k.shown_text(), "••••••••3456");
    }
}
