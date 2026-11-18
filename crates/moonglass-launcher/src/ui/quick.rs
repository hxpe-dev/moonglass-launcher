//! The Quick Menu (spec 5.8): what the player finds on coming back to
//! Moonglass during a game. The game keeps running behind it and, without
//! the focus, stops taking the controller. From here the player returns to
//! the game, saves a clip or a screenshot through a capture tool that
//! already records it, changes the game's own volume, or stops the game.
//!
//! Nothing is drawn inside the game and nothing is injected into it: clips
//! and screenshots are asked of the player's own capture tool, and the
//! volume is the OS mixer's per-program volume.

use std::time::Duration;

use gpui::{div, prelude::*, px, Context, Div, Hsla, Window};
use moonglass_platform::{CaptureKind, GameVolume};

use super::gamepad::NavAction;
use super::icons::{icon, Icon};
use super::theme::{keycap, mix, mono, on, white, INK, TEXT_2};
use super::{raw_window, InputMode, LauncherView, Overlay, OverlayButton, StopState, UiEvent};

/// How long the game gets to come back in front before a capture tool's
/// shortcut is pressed: the tools act on the window in front.
const IN_FRONT_DELAY: Duration = Duration::from_millis(600);
/// One press of left or right changes the volume by this much.
const VOLUME_STEP: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickItem {
    Return,
    Clip,
    Screenshot,
    Volume,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum VolumeState {
    /// Not read since the menu was last shown.
    Unknown,
    Loading,
    /// `None`: the game has no audio stream (yet).
    Known(Option<GameVolume>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuickMenu {
    focus: QuickItem,
    volume: VolumeState,
    /// What the last action did, under the menu.
    note: Option<String>,
}

impl Default for QuickMenu {
    fn default() -> Self {
        Self {
            focus: QuickItem::Return,
            volume: VolumeState::Unknown,
            note: None,
        }
    }
}

/// `level` moved by `delta` steps, on the 10% grid.
fn step_level(level: f32, delta: i8) -> f32 {
    ((level / VOLUME_STEP).round() + delta as f32).clamp(0.0, 1.0 / VOLUME_STEP) * VOLUME_STEP
}

fn what(kind: CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Clip => "the clip",
        CaptureKind::Screenshot => "the screenshot",
    }
}

impl LauncherView {
    /// The rows, in order: Save clip and Screenshot only with a capture
    /// tool that can do them.
    fn quick_items(&self) -> Vec<QuickItem> {
        let tool = self.prefs.capture.tool();
        let mut items = vec![QuickItem::Return];
        if tool.is_some() {
            items.push(QuickItem::Clip);
        }
        if tool.is_some_and(|t| t.takes_screenshots()) {
            items.push(QuickItem::Screenshot);
        }
        items.extend([QuickItem::Volume, QuickItem::Stop]);
        items
    }

    fn quick_menu(&mut self) -> Option<&mut QuickMenu> {
        match &mut self.overlay {
            Overlay::Playing { menu, .. } => Some(menu),
            _ => None,
        }
    }

    /// The restore hotkey brought Moonglass forward: start from the top
    /// with a fresh reading of the volume.
    pub(super) fn quick_opened(&mut self) {
        if let Some(m) = self.quick_menu() {
            *m = QuickMenu::default();
        }
        self.quick_read_volume();
    }

    /// Once a second: a menu in view whose volume is not known yet (the
    /// player came back with Alt+Tab, say) reads it.
    pub(super) fn quick_tick(&mut self, window: &Window) {
        let unknown = matches!(
            &self.overlay,
            Overlay::Playing { menu, .. } if menu.volume == VolumeState::Unknown
        );
        if unknown && window.is_window_active() {
            self.quick_read_volume();
        }
    }

    fn quick_read_volume(&mut self) {
        let Some(s) = &self.session else { return };
        let (session, pids) = (s.id, s.pids.lock().unwrap().clone());
        let tx = self.tx.clone();
        let Some(m) = self.quick_menu() else { return };
        m.volume = VolumeState::Loading;
        self.backend.rt.spawn_blocking(move || {
            let volume = moonglass_platform::game_volume(&pids);
            let _ = tx.unbounded_send(UiEvent::GameVolume { session, volume });
        });
    }

    pub(super) fn quick_volume_read(
        &mut self,
        session: u64,
        volume: Option<GameVolume>,
        cx: &mut Context<Self>,
    ) {
        if self.session.as_ref().is_none_or(|s| s.id != session) {
            return;
        }
        if let Some(m) = self.quick_menu() {
            if m.volume == VolumeState::Loading {
                m.volume = VolumeState::Known(volume);
                cx.notify();
            }
        }
    }

    pub(super) fn quick_nav(&mut self, a: NavAction, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.quick_items();
        let Some(m) = self.quick_menu() else { return };
        let i = items.iter().position(|x| *x == m.focus).unwrap_or(0);
        let n = items.len();
        match a {
            NavAction::Up => m.focus = items[(i + n - 1) % n],
            NavAction::Down => m.focus = items[(i + 1) % n],
            NavAction::Left | NavAction::Right if m.focus == QuickItem::Volume => {
                self.quick_volume(if a == NavAction::Left { -1 } else { 1 }, cx)
            }
            NavAction::Confirm => {
                let item = m.focus;
                self.quick_activate(item, window, cx);
            }
            NavAction::Back => self.quick_activate(QuickItem::Return, window, cx),
            // X / Square, as before the menu: Stop from anywhere.
            NavAction::Search => self.quick_activate(QuickItem::Stop, window, cx),
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn quick_activate(
        &mut self,
        item: QuickItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = self.quick_menu() {
            m.focus = item;
        }
        match item {
            QuickItem::Return => self.return_to_game(window),
            QuickItem::Clip => self.quick_capture(CaptureKind::Clip, window),
            QuickItem::Screenshot => self.quick_capture(CaptureKind::Screenshot, window),
            QuickItem::Volume => self.quick_toggle_mute(),
            QuickItem::Stop => self.press_stop(cx),
        }
        cx.notify();
    }

    /// Brings the game's window back in front, restoring it if it was
    /// minimized (a fullscreen game minimizes itself when it loses focus).
    /// Only if its window cannot be found is Moonglass minimized instead,
    /// which leaves whatever was behind it in front. The menu starts over
    /// next time.
    fn return_to_game(&mut self, window: &mut Window) {
        if let Some(m) = self.quick_menu() {
            *m = QuickMenu::default();
        }
        let pids = self
            .session
            .as_ref()
            .map(|s| s.pids.lock().unwrap().clone())
            .unwrap_or_default();
        if !moonglass_platform::window::show_game(&pids) {
            moonglass_platform::window::hide_for_game(raw_window(window));
        }
    }

    fn quick_capture(&mut self, kind: CaptureKind, window: &mut Window) {
        let Some(tool) = self.prefs.capture.tool() else {
            return;
        };
        let in_front = tool.needs_game_in_front();
        if in_front {
            self.return_to_game(window);
        } else if let Some(m) = self.quick_menu() {
            m.note = Some("Saving…".into());
        }
        let tx = self.tx.clone();
        self.backend.rt.spawn_blocking(move || {
            if in_front {
                std::thread::sleep(IN_FRONT_DELAY);
            }
            let result = moonglass_platform::capture(tool, kind).map_err(|e| e.to_string());
            let _ = tx.unbounded_send(UiEvent::Captured {
                kind,
                in_front,
                result,
            });
        });
    }

    pub(super) fn quick_captured(
        &mut self,
        kind: CaptureKind,
        in_front: bool,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tool = self.prefs.capture.label();
        let note = match &result {
            Ok(()) => {
                tracing::info!(tool, ?kind, "asked the capture tool to save");
                // A tool driven by its shortcut shows its own confirmation
                // over the game.
                (!in_front).then(|| format!("Asked {tool} to save {}", what(kind)))
            }
            Err(e) => {
                tracing::warn!(tool, ?kind, "capture failed: {e}");
                let title = format!("Couldn't save {}", what(kind));
                if in_front {
                    // Moonglass is out of sight: say it over the game.
                    moonglass_platform::window::notify(raw_window(window), &title, e);
                }
                Some(format!("{title}. {e}"))
            }
        };
        if let Some(m) = self.quick_menu() {
            m.note = note;
        }
        cx.notify();
    }

    /// Left or right on the volume row (or its arrows): one step down or up.
    /// Raising the volume unmutes.
    pub(super) fn quick_volume(&mut self, delta: i8, cx: &mut Context<Self>) {
        let Some(m) = self.quick_menu() else { return };
        m.focus = QuickItem::Volume;
        let VolumeState::Known(Some(v)) = m.volume else {
            return;
        };
        let level = step_level(v.level, delta);
        self.quick_set_volume(GameVolume {
            level,
            muted: v.muted && delta < 0,
        });
        cx.notify();
    }

    fn quick_toggle_mute(&mut self) {
        let Some(m) = self.quick_menu() else { return };
        if let VolumeState::Known(Some(v)) = m.volume {
            self.quick_set_volume(GameVolume {
                muted: !v.muted,
                ..v
            });
        }
    }

    /// Shows the new volume at once and sets it in the background.
    fn quick_set_volume(&mut self, v: GameVolume) {
        let Some(s) = &self.session else { return };
        let pids = s.pids.lock().unwrap().clone();
        if let Some(m) = self.quick_menu() {
            m.volume = VolumeState::Known(Some(v));
        }
        self.backend.rt.spawn_blocking(move || {
            if !moonglass_platform::set_game_volume(&pids, v) {
                tracing::debug!("no audio stream to set the volume of");
            }
        });
    }

    /// The menu's rows and the line under them.
    pub(super) fn render_quick_menu(
        &self,
        menu: &QuickMenu,
        stop: StopState,
        u: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let gamepad = self.input == InputMode::Gamepad;
        let keyboard = self.input == InputMode::Keyboard;
        let (k_back, k_stop) = if gamepad { ("B", "X") } else { ("Esc", "Del") };
        let ink: Hsla = gpui::rgb(INK).into();
        let tool = self.prefs.capture.tool();

        let mut rows = div().w(px(560.0 * u)).flex().flex_col().gap(px(8.0 * u));
        for item in self.quick_items() {
            // The mouse shows its own hover; keys and pads need the focus.
            let focused = menu.focus == item && (gamepad || keyboard);
            let t = self.glide(("quick", item), on(focused));
            let fg = mix(white(0.92), ink, t);
            let mut enabled = true;
            let (text, value): (String, Option<Div>) = match item {
                QuickItem::Return => ("Return to game".into(), Some(keycap(k_back, u))),
                QuickItem::Clip => (
                    "Save a clip".into(),
                    // The tool's name, dimmer than the label.
                    tool.map(|tl| small(tl.name(), fg.opacity(0.55), u)),
                ),
                QuickItem::Screenshot => ("Take a screenshot".into(), None),
                QuickItem::Volume => (
                    "Game volume".into(),
                    Some(self.volume_value(menu, fg, u, cx)),
                ),
                QuickItem::Stop => {
                    let (label, ok) = match stop {
                        StopState::Idle => ("Stop game", true),
                        StopState::Requested(_) | StopState::Forcing => ("Stopping…", false),
                        StopState::CanForce => ("Force stop", true),
                    };
                    enabled = ok;
                    (label.into(), Some(keycap(k_stop, u)))
                }
            };
            rows = rows.child(
                div()
                    .id(("quick", item as usize))
                    .h(px(58.0 * u))
                    .px(px(26.0 * u))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0 * u))
                    .bg(mix(white(0.08), white(0.92), t))
                    .text_color(fg)
                    .when(!enabled, |d| d.opacity(0.5))
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(white(0.16)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.overlay_click(OverlayButton::Quick(item), window, cx)
                            }))
                    })
                    .child(text)
                    .children(value),
            );
        }

        let mut c = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.0 * u))
            .child(rows);
        if let Some(n) = &menu.note {
            c = c.child(
                div()
                    .max_w(px(640.0 * u))
                    .text_size(px(17.0 * u))
                    .text_color(white(TEXT_2))
                    .child(n.clone()),
            );
        }
        c
    }

    /// "‹ 80% ›" with clickable arrows, or why there is no volume to show.
    fn volume_value(&self, menu: &QuickMenu, fg: Hsla, u: f32, cx: &mut Context<Self>) -> Div {
        let v = match menu.volume {
            VolumeState::Known(Some(v)) => v,
            VolumeState::Known(None) => return small("No sound from the game", fg.opacity(0.6), u),
            VolumeState::Unknown | VolumeState::Loading => return small("…", fg.opacity(0.6), u),
        };
        let arrow = |id: &'static str, which: Icon, delta: i8, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(34.0 * u))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(white(0.2)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.overlay_click(OverlayButton::Volume(delta), window, cx)
                }))
                .child(icon(which, 18.0 * u, fg))
        };
        let text = if v.muted {
            "Muted".to_string()
        } else {
            format!("{}%", (v.level * 100.0).round() as u32)
        };
        div()
            .flex()
            .items_center()
            .gap(px(6.0 * u))
            .child(arrow("quick-vol-down", Icon::ChevronLeft, -1, cx))
            .child(
                div()
                    .w(px(78.0 * u))
                    .flex()
                    .justify_center()
                    .font_family(mono())
                    .text_size(px(18.0 * u))
                    .child(text),
            )
            .child(arrow("quick-vol-up", Icon::ChevronRight, 1, cx))
    }
}

fn small(text: &str, color: Hsla, u: f32) -> Div {
    div()
        .font_family(mono())
        .text_size(px(15.0 * u))
        .text_color(color)
        .child(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_steps_on_a_ten_percent_grid() {
        assert!((step_level(0.8, 1) - 0.9).abs() < 1e-6);
        assert!((step_level(0.84, -1) - 0.7).abs() < 1e-6);
        assert_eq!(step_level(1.0, 1), 1.0);
        assert_eq!(step_level(0.0, -1), 0.0);
    }
}
