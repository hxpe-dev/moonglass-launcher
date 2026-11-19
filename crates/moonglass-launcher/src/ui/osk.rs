//! On-screen keyboard (spec 5.5): used for search and the few text fields
//! (the SteamGridDB and Steam keys, launch arguments, process to watch,
//! emulator arguments and file types, a ROM folder's platform name). A
//! physical keyboard types into the same fields.
//!
//! Gamepad: D-pad moves, A types the key, X deletes, Y types a space,
//! RB pastes, Menu (Start) is Done, B cancels. Ctrl+V pastes too.

use gpui::{
    div, linear_color_stop, linear_gradient, prelude::*, px, AnyElement, Context, SharedString,
};
use moonglass_core::GameId;

use super::gamepad::NavAction;
use super::theme::{keycap, on, selected, white};
use super::LauncherView;

/// The last row has what launch arguments and paths need.
const ROWS: [&str; 5] = [
    "1234567890",
    "qwertyuiop",
    "asdfghjkl'",
    "zxcvbnm,.-",
    "{}\"/\\:_=+@",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    Shift,
    Space,
    Delete,
    Paste,
    Done,
}

const SPECIALS: [Special; 5] = [
    Special::Shift,
    Special::Space,
    Special::Delete,
    Special::Paste,
    Special::Done,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextTarget {
    Search,
    SgdbKey,
    SteamApiKey,
    LaunchArgs(GameId),
    WatchProcess(GameId),
    /// By emulator profile ID.
    EmulatorArguments(String),
    EmulatorFileTypes(String),
    /// The platform label for a ROM folder being added.
    RomPlatform {
        emulator: String,
        path: std::path::PathBuf,
    },
}

impl TextTarget {
    pub fn label(&self) -> &'static str {
        match self {
            TextTarget::Search => "Search",
            TextTarget::SgdbKey => "SteamGridDB API key",
            TextTarget::SteamApiKey => "Steam Web API key (steamcommunity.com/dev/apikey)",
            TextTarget::LaunchArgs(_) => "Launch arguments",
            TextTarget::WatchProcess(_) => "Process to watch (for example Game.exe)",
            TextTarget::EmulatorArguments(_) => "Arguments ({rom} is the game's file)",
            TextTarget::EmulatorFileTypes(_) => "File types (for example .iso,.rvz)",
            TextTarget::RomPlatform { .. } => "Platform name (for example GameCube)",
        }
    }

    /// The emulator profile this text belongs to, if any.
    pub fn emulator(&self) -> Option<&str> {
        match self {
            TextTarget::EmulatorArguments(id)
            | TextTarget::EmulatorFileTypes(id)
            | TextTarget::RomPlatform { emulator: id, .. } => Some(id),
            _ => None,
        }
    }

    pub fn is_secret(&self) -> bool {
        matches!(self, TextTarget::SgdbKey | TextTarget::SteamApiKey)
    }
}

/// Clipboard text made fit for a one-line field: the first non-empty line,
/// trimmed, without control characters. A key (`secret`) loses every space,
/// as copying from a web page often adds some.
pub fn clean_paste(text: &str, secret: bool) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    line.chars()
        .filter(|c| !c.is_control() && !(secret && c.is_whitespace()))
        .collect()
}

/// Whether text looks like a SteamGridDB or Steam Web API key: 32
/// hexadecimal digits. Only such text is pasted without asking.
pub fn looks_like_key(text: &str) -> bool {
    text.len() == 32 && text.chars().all(|c| c.is_ascii_hexdigit())
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
    /// Paste the clipboard (the view reads it).
    Paste,
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
            Some(Special::Paste) => OskResult::Paste,
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
            NavAction::FilterNext => return OskResult::Paste,
            NavAction::FilterPrev => {}
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
        if self.target.is_secret() && !self.text.is_empty() {
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
    /// The keyboard as a bottom sheet. `caret` is the blink state; `shown`
    /// (0 to 1) slides the sheet up from the bottom edge.
    pub(super) fn render_osk(
        &self,
        osk: &Osk,
        u: f32,
        caret: bool,
        shown: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key_w = 64.0 * u;
        let key_h = 56.0 * u;
        // How far the highlight has moved onto each key.
        let lit =
            |r: usize, c: usize| self.glide(("osk-key", r, c), on(osk.row == r && osk.col == c));

        // Outlined keys, as the site's pills; the highlighted one is solid.
        // `strong` brightens the outline (Shift while on, Done).
        let key = |id: (&'static str, usize), label: String, w: f32, lit: f32, strong: bool| {
            selected(div().id(id), lit)
                .w(px(w))
                .h(px(key_h))
                .rounded(px(6.0 * u))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .border_1()
                .border_color(white(if strong { 0.7 } else { 0.18 } + 0.3 * lit))
                .when(lit < 0.5, |d| d.hover(|s| s.border_color(white(0.6))))
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
                        lit(r, c),
                        false,
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.osk_click(r, c, window, cx)),
                    ),
                );
            }
            rows = rows.child(row);
        }
        // The special keys are named in mono capitals, like the site's labels.
        let mut specials = div()
            .flex()
            .gap(px(8.0 * u))
            .font_family(super::theme::mono())
            .text_size(px(15.0 * u));
        for (c, sp) in SPECIALS.iter().enumerate() {
            let (label, w, strong) = match sp {
                Special::Shift => ("SHIFT", key_w * 2.0, osk.shift),
                Special::Space => ("SPACE", key_w * 3.0, false),
                Special::Delete => ("DELETE", key_w * 2.0, false),
                Special::Paste => ("PASTE", key_w * 2.0, false),
                Special::Done => ("DONE", key_w * 2.0, true),
            };
            let r = ROWS.len();
            specials = specials.child(
                key(("osk-special", c), label.to_string(), w, lit(r, c), strong).on_click(
                    cx.listener(move |this, _, window, cx| this.osk_click(r, c, window, cx)),
                ),
            );
        }
        rows = rows
            .child(specials)
            // Framed by corner marks, like the site's wordmark.
            .relative()
            .children(super::theme::crop_marks_out(20.0 * u, 18.0 * u, white(0.4)));

        let field = div()
            .w(px(key_w * 10.0 + 72.0 * u))
            .px(px(18.0 * u))
            .py(px(10.0 * u))
            .rounded_full()
            .border_1()
            .border_color(white(0.6))
            .flex()
            .items_center()
            .child(
                div()
                    .when(osk.text.is_empty(), |d| {
                        d.text_color(white(super::theme::TEXT_3))
                    })
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
                ("RB", "Paste"),
                ("≡", "Done"),
                ("B", "Cancel"),
            ]
        } else {
            &[
                ("Type", "with your keyboard or click"),
                ("Ctrl+V", "Paste"),
                ("Enter", "Done"),
                ("Esc", "Cancel"),
            ]
        };
        let hints = div()
            .mt(px(6.0 * u))
            .flex()
            .gap(px(26.0 * u))
            .children(keys.iter().map(|(k, l)| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0 * u))
                    .child(keycap(k, u))
                    .child(super::theme::label(l, 14.0, u).text_color(white(super::theme::TEXT_2)))
            }));

        div()
            .id("osk")
            .occlude()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(-(1.0 - shown) * 560.0 * u))
            .opacity(shown)
            .pt(px(110.0 * u))
            .pb(px(26.0 * u))
            // No hard top edge: the sheet fades in from the screen above.
            .bg(linear_gradient(
                180.,
                linear_color_stop(super::theme::black(0.0), 0.),
                linear_color_stop(super::theme::black(0.97), 0.17),
            ))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(16.0 * u))
            .when(osk.target != TextTarget::Search, |d| {
                d.child(super::theme::label(osk.target.label(), 14.0, u))
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
    fn pasted_text_is_one_clean_line() {
        assert_eq!(
            clean_paste("\n  hello world \r\nsecond", false),
            "hello world"
        );
        assert_eq!(clean_paste(" ab cd\tef \n", true), "abcdef");
        assert_eq!(clean_paste("", false), "");
    }

    #[test]
    fn keys_are_recognised() {
        assert!(looks_like_key("0123456789abcdef0123456789ABCDEF"));
        assert!(!looks_like_key("0123456789abcdef0123456789ABCDE"));
        assert!(!looks_like_key("0123456789abcdef0123456789ABCDEG"));
        assert!(!looks_like_key("https://www.steamgriddb.com/profi"));
    }

    #[test]
    fn paste_key_and_rb_ask_to_paste() {
        let mut k = Osk::new(TextTarget::SgdbKey, String::new());
        assert!(matches!(k.nav(NavAction::FilterNext), OskResult::Paste));
        let paste = SPECIALS.iter().position(|s| *s == Special::Paste).unwrap();
        assert!(matches!(k.click(ROWS.len(), paste), OskResult::Paste));
    }

    #[test]
    fn secrets_are_masked() {
        let k = Osk::new(TextTarget::SgdbKey, "abcdef123456".into());
        assert_eq!(k.shown_text(), "••••••••3456");
    }
}
