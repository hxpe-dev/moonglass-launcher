//! Foreground gamepad navigation (spec 5.4): a dedicated `gilrs` thread
//! turns controller input into typed actions sent to the UI. The left stick
//! uses a radial deadzone; holding a direction repeats with acceleration.
//! View plays and pauses the background music, LT and RT change song.
//!
//! Where the platform says so (Linux, whose evdev backend works without
//! focus), the same thread also watches for the restore combination
//! (View + Menu, or both sticks, held; see Settings → System) while a game
//! runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs, PowerInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavAction {
    Left,
    Right,
    Up,
    Down,
    Confirm,
    Back,
    Options,
    Search,
    FilterPrev,
    FilterNext,
    OpenSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Xbox,
    PlayStation,
    Nintendo,
}

impl Family {
    fn from_name(name: &str) -> Self {
        let n = name.to_lowercase();
        if [
            "dualsense",
            "dualshock",
            "playstation",
            "ps4",
            "ps5",
            "wireless controller",
        ]
        .iter()
        .any(|k| n.contains(k))
        {
            Family::PlayStation
        } else if ["nintendo", "pro controller", "joy-con"]
            .iter()
            .any(|k| n.contains(k))
        {
            Family::Nintendo
        } else {
            Family::Xbox
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControllerStatus {
    pub connected: bool,
    pub family: Family,
    /// Battery percentage when the controller reports one.
    pub battery: Option<u8>,
    pub charging: bool,
}

/// The music buttons: View plays and pauses, LT and RT change song.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicControl {
    Previous,
    PlayPause,
    Next,
}

#[derive(Debug, Clone)]
pub enum PadEvent {
    Nav(NavAction),
    Music(MusicControl),
    Status(ControllerStatus),
    /// The restore combination was held while armed.
    Restore,
    /// B / Circle was let go (holding it on the home screen exits).
    BackReleased,
}

const DEADZONE: f32 = 0.45;
const FIRST_REPEAT: Duration = Duration::from_millis(380);
const SLOWEST_REPEAT: Duration = Duration::from_millis(170);
const FASTEST_REPEAT: Duration = Duration::from_millis(55);

/// Direction from the left stick using a radial deadzone (the stick's
/// distance from center, not each axis separately), so diagonals do not
/// trigger two directions.
fn stick_direction(x: f32, y: f32) -> Option<NavAction> {
    if (x * x + y * y).sqrt() < DEADZONE {
        return None;
    }
    Some(if x.abs() > y.abs() {
        if x > 0.0 {
            NavAction::Right
        } else {
            NavAction::Left
        }
    } else if y > 0.0 {
        NavAction::Up
    } else {
        NavAction::Down
    })
}

fn status_of(gilrs: &Gilrs, id: Option<GamepadId>) -> ControllerStatus {
    let Some(pad) = id.and_then(|id| gilrs.connected_gamepad(id)) else {
        return ControllerStatus::default();
    };
    let (battery, charging) = match pad.power_info() {
        PowerInfo::Discharging(p) => (Some(p), false),
        PowerInfo::Charging(p) => (Some(p), true),
        PowerInfo::Charged => (Some(100), false),
        PowerInfo::Wired | PowerInfo::Unknown => (None, false),
    };
    ControllerStatus {
        connected: true,
        family: Family::from_name(pad.name()),
        battery,
        charging,
    }
}

/// Starts the thread. `restore_armed` is set by the UI while Launching or
/// Running; the combination is only watched if `watch_restore` (the
/// platform reads the background gamepad through this thread).
pub fn spawn(
    tx: UnboundedSender<PadEvent>,
    restore_armed: Arc<AtomicBool>,
    watch_restore: bool,
    restore: Arc<moonglass_platform::RestoreConfig>,
) {
    let spawned = std::thread::Builder::new()
        .name("moonglass-gamepad".into())
        .spawn(move || {
            let mut gilrs = match Gilrs::new() {
                Ok(g) => g,
                Err(e) => {
                    tracing::warn!(error = %e, "gamepad input unavailable");
                    return;
                }
            };
            let mut active: Option<GamepadId> = gilrs.gamepads().next().map(|(id, _)| id);
            let _ = tx.unbounded_send(PadEvent::Status(status_of(&gilrs, active)));

            let mut held: Option<(NavAction, Instant, Instant, u32)> = None; // dir, pressed, last fire, repeats
            let mut combo_since: Option<Instant> = None;
            let mut combo_fired = false;
            let mut last_status = Instant::now();

            loop {
                let timeout = if held.is_some() || combo_since.is_some() {
                    Duration::from_millis(15)
                } else {
                    Duration::from_secs(5)
                };
                if let Some(ev) = gilrs.next_event_blocking(Some(timeout)) {
                    active = Some(ev.id);
                    let send = |a| {
                        let _ = tx.unbounded_send(PadEvent::Nav(a));
                    };
                    let music = |m| {
                        let _ = tx.unbounded_send(PadEvent::Music(m));
                    };
                    let press_dir = |dir: NavAction, held: &mut Option<_>| {
                        send(dir);
                        let now = Instant::now();
                        *held = Some((dir, now, now, 0));
                    };
                    match ev.event {
                        EventType::ButtonPressed(b, _) => match b {
                            Button::DPadLeft => press_dir(NavAction::Left, &mut held),
                            Button::DPadRight => press_dir(NavAction::Right, &mut held),
                            Button::DPadUp => press_dir(NavAction::Up, &mut held),
                            Button::DPadDown => press_dir(NavAction::Down, &mut held),
                            Button::South => send(NavAction::Confirm),
                            Button::East => send(NavAction::Back),
                            Button::West => send(NavAction::Search),
                            Button::North => send(NavAction::Options),
                            Button::LeftTrigger => send(NavAction::FilterPrev),
                            Button::RightTrigger => send(NavAction::FilterNext),
                            Button::Start => send(NavAction::OpenSettings),
                            Button::Select => music(MusicControl::PlayPause),
                            Button::LeftTrigger2 => music(MusicControl::Previous),
                            Button::RightTrigger2 => music(MusicControl::Next),
                            _ => {}
                        },
                        EventType::ButtonReleased(
                            Button::DPadLeft
                            | Button::DPadRight
                            | Button::DPadUp
                            | Button::DPadDown,
                            _,
                        ) => held = None,
                        EventType::ButtonReleased(Button::East, _) => {
                            let _ = tx.unbounded_send(PadEvent::BackReleased);
                        }
                        EventType::AxisChanged(Axis::LeftStickX | Axis::LeftStickY, _, _) => {
                            let pad = gilrs.gamepad(ev.id);
                            let x = pad.value(Axis::LeftStickX);
                            let y = pad.value(Axis::LeftStickY);
                            match (stick_direction(x, y), held) {
                                (Some(d), Some((h, ..))) if d == h => {}
                                (Some(d), _) => press_dir(d, &mut held),
                                (None, _) => held = None,
                            }
                        }
                        EventType::Connected | EventType::Disconnected => {
                            if matches!(ev.event, EventType::Disconnected) {
                                active = gilrs.gamepads().next().map(|(id, _)| id);
                            }
                            let _ = tx.unbounded_send(PadEvent::Status(status_of(&gilrs, active)));
                        }
                        _ => {}
                    }
                }

                // Repeat with acceleration while a direction is held.
                if let Some((dir, pressed, last, n)) = held {
                    let interval = SLOWEST_REPEAT
                        .saturating_sub(Duration::from_millis(20) * n)
                        .max(FASTEST_REPEAT);
                    let due = if n == 0 {
                        pressed + FIRST_REPEAT
                    } else {
                        last + interval
                    };
                    if Instant::now() >= due {
                        let _ = tx.unbounded_send(PadEvent::Nav(dir));
                        held = Some((dir, pressed, Instant::now(), n + 1));
                    }
                }

                // Background restore combination (platforms that use this thread).
                if watch_restore && restore_armed.load(Ordering::Relaxed) {
                    let (a, b) = match restore.combo() {
                        moonglass_platform::RestoreCombo::ViewMenu => {
                            (Button::Select, Button::Start)
                        }
                        moonglass_platform::RestoreCombo::StickClicks => {
                            (Button::LeftThumb, Button::RightThumb)
                        }
                    };
                    let down = gilrs
                        .gamepads()
                        .any(|(_, p)| p.is_pressed(a) && p.is_pressed(b));
                    match (down, combo_since) {
                        (true, None) => combo_since = Some(Instant::now()),
                        (true, Some(t)) if !combo_fired && t.elapsed() >= restore.hold() => {
                            combo_fired = true;
                            let _ = tx.unbounded_send(PadEvent::Restore);
                        }
                        (false, _) => {
                            combo_since = None;
                            combo_fired = false;
                        }
                        _ => {}
                    }
                } else {
                    combo_since = None;
                    combo_fired = false;
                }

                // Battery changes produce no event; refresh occasionally.
                if last_status.elapsed() > Duration::from_secs(60) {
                    last_status = Instant::now();
                    let _ = tx.unbounded_send(PadEvent::Status(status_of(&gilrs, active)));
                }
                if tx.is_closed() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start gamepad thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radial_deadzone() {
        assert_eq!(stick_direction(0.3, 0.3), None); // 0.42 from center
        assert_eq!(stick_direction(0.9, 0.2), Some(NavAction::Right));
        assert_eq!(stick_direction(-0.2, -0.8), Some(NavAction::Down));
        assert_eq!(stick_direction(0.1, 0.7), Some(NavAction::Up));
    }

    #[test]
    fn families() {
        assert_eq!(
            Family::from_name("DualSense Wireless Controller"),
            Family::PlayStation
        );
        assert_eq!(Family::from_name("Xbox Series X Controller"), Family::Xbox);
        assert_eq!(
            Family::from_name("Nintendo Switch Pro Controller"),
            Family::Nintendo
        );
    }
}
