//! First-run setup (spec 3, First-Run Flow): the games found, emulators and
//! ROM folders, a SteamGridDB key, then the carousel. Every step can be
//! skipped. It opens on the first start with an empty library, from the
//! "Add your games" card, and from Settings → System.

use gpui::{div, prelude::*, px, AnyElement, Context, Div, SharedString};
use moonglass_core::Source;

use super::gamepad::NavAction;
use super::osk::{Osk, TextTarget};
use super::theme::{black, keycap, on, selected, white};
use super::{InputMode, LauncherView, Modal, Screen};

/// Set once setup was finished or skipped; it then never opens by itself.
pub const SETTING_SETUP_DONE: &str = "setup_done";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Emulators,
    Artwork,
    Done,
}

impl Step {
    const ALL: [Step; 4] = [Step::Welcome, Step::Emulators, Step::Artwork, Step::Done];

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// What a button on a setup step does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAction {
    Next,
    AddEmulator,
    EnterSgdbKey,
    Finish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupState {
    pub step: Step,
    pub selected: usize,
}

impl SetupState {
    pub fn new() -> Self {
        Self {
            step: Step::Welcome,
            selected: 0,
        }
    }

    fn go(&mut self, step: Step) {
        self.step = step;
        self.selected = 0;
    }

    fn next(&mut self) {
        if let Some(&s) = Step::ALL.get(self.step.index() + 1) {
            self.go(s);
        }
    }

    /// `false` on the first step, where there is nothing to go back to.
    fn back(&mut self) -> bool {
        match self.step.index() {
            0 => false,
            i => {
                self.go(Step::ALL[i - 1]);
                true
            }
        }
    }
}

/// The buttons of a step, first is the default.
fn buttons(step: Step, has_emulators: bool, key_set: bool) -> Vec<(&'static str, SetupAction)> {
    match step {
        Step::Welcome => vec![("Continue", SetupAction::Next)],
        Step::Emulators => vec![
            (
                if has_emulators {
                    "Add another emulator"
                } else {
                    "Add an emulator"
                },
                SetupAction::AddEmulator,
            ),
            (
                if has_emulators { "Continue" } else { "Skip" },
                SetupAction::Next,
            ),
        ],
        Step::Artwork => vec![
            (
                if key_set {
                    "Change the key"
                } else {
                    "Enter a SteamGridDB key"
                },
                SetupAction::EnterSgdbKey,
            ),
            (if key_set { "Continue" } else { "Skip" }, SetupAction::Next),
        ],
        Step::Done => vec![("Start playing", SetupAction::Finish)],
    }
}

fn count_label(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

impl LauncherView {
    pub(super) fn open_setup(&mut self) {
        self.search_focused = false;
        self.set_modal(Modal::None);
        self.setup_leaving = None;
        self.screen = Screen::Setup(SetupState::new());
    }

    /// Leaves setup for the carousel, and remembers it was done.
    fn finish_setup(&mut self, cx: &mut Context<Self>) {
        if let Screen::Setup(st) = std::mem::replace(&mut self.screen, Screen::Home) {
            self.setup_leaving = Some(st);
        }
        let db = self.backend.db.clone();
        self.backend.rt.spawn(async move {
            if let Err(e) = db.set_setting(SETTING_SETUP_DONE, &true).await {
                tracing::error!(error = %e, "saving setup state");
            }
        });
        cx.notify();
    }

    fn setup_buttons(&self, step: Step) -> Vec<(&'static str, SetupAction)> {
        buttons(step, !self.emulators.is_empty(), self.sgdb_key_set)
    }

    fn run_setup_action(&mut self, action: SetupAction, cx: &mut Context<Self>) {
        let Screen::Setup(st) = &mut self.screen else {
            return;
        };
        match action {
            SetupAction::Next => st.next(),
            SetupAction::Finish => return self.finish_setup(cx),
            SetupAction::AddEmulator => self.open_preset_menu(),
            SetupAction::EnterSgdbKey => self.set_modal(Modal::Keyboard(Osk::new(
                TextTarget::SgdbKey,
                String::new(),
            ))),
        }
        cx.notify();
    }

    pub(super) fn setup_nav(&mut self, a: NavAction, cx: &mut Context<Self>) {
        let Screen::Setup(st) = &self.screen else {
            return;
        };
        let buttons = self.setup_buttons(st.step);
        let n = buttons.len();
        let pick = buttons[st.selected.min(n - 1)].1;
        let Screen::Setup(st) = &mut self.screen else {
            return;
        };
        match a {
            NavAction::Up | NavAction::Left => st.selected = st.selected.saturating_sub(1),
            NavAction::Down | NavAction::Right => st.selected = (st.selected + 1).min(n - 1),
            NavAction::Confirm => return self.run_setup_action(pick, cx),
            // Leaving from the first step skips setup.
            NavAction::Back if !st.back() => return self.finish_setup(cx),
            _ => {}
        }
        cx.notify();
    }

    fn setup_click(&mut self, i: usize, cx: &mut Context<Self>) {
        self.set_input(InputMode::Mouse, cx);
        let Screen::Setup(st) = &self.screen else {
            return;
        };
        if let Some(&(_, action)) = self.setup_buttons(st.step).get(i) {
            self.run_setup_action(action, cx);
        }
    }

    /// The text of a step.
    fn setup_body(&self, step: Step) -> (String, Vec<String>) {
        match step {
            Step::Welcome => {
                let installed = |s: Source| {
                    self.games
                        .iter()
                        .filter(|g| g.is_installed && g.source == s)
                        .count()
                };
                let found = if !self.scanned {
                    "Looking for your games…".to_string()
                } else {
                    let steam = installed(Source::Steam);
                    let epic = installed(Source::Epic);
                    if steam + epic == 0 {
                        "No Steam or Epic games were found on this computer.".into()
                    } else {
                        format!(
                            "Found {} on Steam and {} on Epic.",
                            count_label(steam, "game", "games"),
                            count_label(epic, "game", "games"),
                        )
                    }
                };
                (
                    "Welcome to Moonglass".into(),
                    vec![
                        found,
                        "Games installed later appear by themselves.".into(),
                    ],
                )
            }
            Step::Emulators => {
                let mut lines = vec![
                    "Play games from other consoles: choose an emulator, then the folder with its games.".into(),
                ];
                for e in &self.emulators {
                    lines.push(format!("{} · {}", e.emulator.name, e.summary()));
                }
                ("Emulators".into(), lines)
            }
            Step::Artwork => (
                "Artwork".into(),
                vec![
                    "Steam, Epic and Switch games get their artwork without any setup.".into(),
                    "For other games, a free SteamGridDB key (steamgriddb.com, Preferences → API) finds covers by title.".into(),
                    if self.sgdb_key_set {
                        "A key is saved.".into()
                    } else {
                        "You can add it later in Settings → Artwork.".into()
                    },
                ],
            ),
            Step::Done => {
                let n = self.games.iter().filter(|g| g.is_visible()).count();
                (
                    "You're all set".into(),
                    vec![
                        format!(
                            "{} in your library. Artwork fills in as it downloads.",
                            count_label(n, "game", "games")
                        ),
                        "Everything here can be changed later in Settings.".into(),
                    ],
                )
            }
        }
    }

    pub(super) fn render_setup(
        &self,
        st: &SetupState,
        shown: f32,
        u: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, lines) = self.setup_body(st.step);
        // Progress: one dot per step.
        let dots = Step::ALL
            .iter()
            .fold(div().flex().gap(px(8.0 * u)), |d, s| {
                let lit = self.glide(("setup-dot", s.index()), on(*s == st.step));
                d.child(
                    div()
                        .h(px(8.0 * u))
                        .w(px((8.0 + 20.0 * lit) * u))
                        .rounded_full()
                        .bg(white(0.25 + 0.65 * lit)),
                )
            });
        let mut body = div().flex().flex_col().gap(px(10.0 * u));
        for (i, line) in lines.into_iter().enumerate() {
            body = body.child(
                div()
                    .text_size(px(if i == 0 { 22.0 } else { 18.0 } * u))
                    .text_color(white(if i == 0 { 0.9 } else { 0.65 }))
                    .child(SharedString::from(line)),
            );
        }
        let mut list = div().flex().flex_col().gap(px(6.0 * u));
        for (i, (label, _)) in self.setup_buttons(st.step).into_iter().enumerate() {
            let lit = self.glide(("setup-button", st.step.index(), i), on(i == st.selected));
            list = list.child(
                selected(div().id(("setup-button", i)), lit)
                    .px(px(22.0 * u))
                    .py(px(14.0 * u))
                    .rounded(px(16.0 * u))
                    .cursor_pointer()
                    .when(i != st.selected, |d| d.hover(|s| s.bg(white(0.08))))
                    .on_click(cx.listener(move |this, _, _, cx| this.setup_click(i, cx)))
                    .child(label),
            );
        }
        let gamepad = self.input == InputMode::Gamepad;
        let hint = |k: &str, l: &str| -> Div {
            div()
                .flex()
                .items_center()
                .gap(px(8.0 * u))
                .child(keycap(k, u))
                .child(l.to_string())
        };
        let footer = div()
            .flex()
            .gap(px(26.0 * u))
            .text_size(px(16.0 * u))
            .text_color(white(0.75))
            .child(hint(if gamepad { "A" } else { "Enter" }, "Select"))
            .child(hint(
                if gamepad { "B" } else { "Esc" },
                if st.step == Step::Welcome {
                    "Skip setup"
                } else {
                    "Back"
                },
            ));
        // Each step slides in.
        let entered = self.appear(("setup-step", st.step.index()));
        div()
            .id("setup")
            .occlude()
            .absolute()
            .size_full()
            .opacity(shown)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .top(px((1.0 - shown) * 30.0 * u))
                    .w(px(820.0 * u))
                    .p(px(48.0 * u))
                    .rounded(px(32.0 * u))
                    .bg(black(0.45))
                    .border_1()
                    .border_color(white(0.10))
                    .flex()
                    .flex_col()
                    .gap(px(28.0 * u))
                    .child(dots)
                    .child(
                        div()
                            .relative()
                            .left(px((1.0 - entered) * 32.0 * u))
                            .opacity(entered)
                            .flex()
                            .flex_col()
                            .gap(px(28.0 * u))
                            .child(
                                div()
                                    .text_size(px(40.0 * u))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(SharedString::from(title)),
                            )
                            .child(body)
                            .child(list),
                    )
                    .child(footer),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_go_forward_and_back() {
        let mut s = SetupState::new();
        assert!(!s.back());
        s.next();
        assert_eq!(s.step, Step::Emulators);
        s.selected = 1;
        s.next();
        assert_eq!((s.step, s.selected), (Step::Artwork, 0));
        s.next();
        s.next();
        assert_eq!(s.step, Step::Done);
        assert!(s.back());
        assert_eq!(s.step, Step::Artwork);
    }

    #[test]
    fn skip_turns_into_continue() {
        assert_eq!(buttons(Step::Emulators, false, false)[1].0, "Skip");
        assert_eq!(buttons(Step::Emulators, true, false)[1].0, "Continue");
        assert_eq!(
            buttons(Step::Artwork, false, true)[0].1,
            SetupAction::EnterSgdbKey
        );
    }
}
