//! The launch state machine (spec 5.3).
//!
//! ```text
//! Idle → Launching → Running → Exiting → Idle
//!             ↓ (timeout or cancel)        ↑
//!             └────────────────────────────┘
//! ```
//!
//! This type only decides; it never touches the OS. The caller starts the
//! game, polls for matching processes once per second, feeds the answer in
//! with [`Lifecycle::observe`], and carries out the returned [`Effect`]s
//! (show overlays, hide the window, regain focus, record playtime).
//!
//! `Exiting` is the grace period: the last matching process has gone, and
//! Moonglass waits before believing the game is over, because launchers often
//! hand off from a stub to the real game.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// When the Launching overlay adds "Still waiting… [B] Cancel".
    pub still_waiting_after: Duration,
    /// When Launching gives up and returns to Idle.
    pub give_up_after: Duration,
    /// How long no matching process must be seen before the game is over.
    pub exit_grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            still_waiting_after: Duration::from_secs(20),
            give_up_after: Duration::from_secs(180),
            exit_grace: Duration::from_secs(5),
        }
    }
}

/// Monotonic timestamps are passed as a `Duration` since an arbitrary
/// origin chosen by the caller (normally the app's start `Instant`).
pub type Now = Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Launching {
        started: Now,
        still_waiting_shown: bool,
    },
    Running {
        since: Now,
    },
    Exiting {
        running_since: Now,
        last_seen: Now,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Show "Launching *Game*…" and stop all animation.
    ShowLaunching,
    /// Add "Still waiting… [B] Cancel" to the overlay.
    ShowStillWaiting,
    /// The game is running: hide Moonglass (platform-specific), render nothing,
    /// suspend navigation, arm the restore hotkey.
    EnterRunning,
    /// Launching ended without the game starting (cancel or timeout).
    /// Remove the overlay and return to the carousel.
    LaunchAbandoned { reason: AbandonReason },
    /// The game is over. Regain focus, dismiss the launcher window, record
    /// `last_played` and add `session` to playtime, rescan, re-render.
    GameEnded { session: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbandonReason {
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone)]
pub struct Lifecycle {
    state: State,
    timing: Timing,
}

impl Lifecycle {
    pub fn new(timing: Timing) -> Self {
        Self {
            state: State::Idle,
            timing,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Gamepad navigation is dispatched only in Idle (spec 5.4).
    pub fn accepts_navigation(&self) -> bool {
        self.state == State::Idle
    }

    /// The restore hotkey is armed only while Launching or Running.
    pub fn restore_hotkey_armed(&self) -> bool {
        matches!(
            self.state,
            State::Launching { .. } | State::Running { .. } | State::Exiting { .. }
        )
    }

    /// Call right before starting the game. Ignored unless Idle.
    pub fn launch(&mut self, now: Now) -> Vec<Effect> {
        if self.state != State::Idle {
            return Vec::new();
        }
        self.state = State::Launching {
            started: now,
            still_waiting_shown: false,
        };
        vec![Effect::ShowLaunching]
    }

    /// `[B] Cancel` on the Launching overlay. Ignored in other states: a
    /// running game is left alone, and Force quit is a separate action that
    /// kills the processes and lets [`observe`](Self::observe) see them go.
    pub fn cancel(&mut self) -> Vec<Effect> {
        if let State::Launching { .. } = self.state {
            self.state = State::Idle;
            vec![Effect::LaunchAbandoned {
                reason: AbandonReason::Cancelled,
            }]
        } else {
            Vec::new()
        }
    }

    /// Feed one poll result: whether any process matching the game exists.
    pub fn observe(&mut self, now: Now, game_running: bool) -> Vec<Effect> {
        let t = self.timing;
        match self.state {
            State::Idle => Vec::new(),
            State::Launching {
                started,
                still_waiting_shown,
            } => {
                if game_running {
                    self.state = State::Running { since: now };
                    return vec![Effect::EnterRunning];
                }
                let waited = now.saturating_sub(started);
                if waited >= t.give_up_after {
                    self.state = State::Idle;
                    return vec![Effect::LaunchAbandoned {
                        reason: AbandonReason::TimedOut,
                    }];
                }
                if !still_waiting_shown && waited >= t.still_waiting_after {
                    self.state = State::Launching {
                        started,
                        still_waiting_shown: true,
                    };
                    return vec![Effect::ShowStillWaiting];
                }
                Vec::new()
            }
            State::Running { since } => {
                if !game_running {
                    self.state = State::Exiting {
                        running_since: since,
                        last_seen: now,
                    };
                }
                Vec::new()
            }
            State::Exiting {
                running_since,
                last_seen,
            } => {
                if game_running {
                    // Hand-off from a stub launcher to the real game.
                    self.state = State::Running {
                        since: running_since,
                    };
                    return Vec::new();
                }
                if now.saturating_sub(last_seen) >= t.exit_grace {
                    self.state = State::Idle;
                    // Playtime runs from Running to the moment the last
                    // process vanished; the grace wait is not play.
                    return vec![Effect::GameEnded {
                        session: last_seen.saturating_sub(running_since),
                    }];
                }
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: u64) -> Now {
        Duration::from_secs(secs)
    }

    fn lc() -> Lifecycle {
        Lifecycle::new(Timing::default())
    }

    #[test]
    fn happy_path() {
        let mut l = lc();
        assert!(l.accepts_navigation());
        assert_eq!(l.launch(s(0)), [Effect::ShowLaunching]);
        assert!(!l.accepts_navigation());
        assert!(l.restore_hotkey_armed());
        assert!(l.observe(s(1), false).is_empty());
        assert_eq!(l.observe(s(10), true), [Effect::EnterRunning]);
        assert!(l.observe(s(100), true).is_empty());
        assert!(l.observe(s(110), false).is_empty()); // grace starts
        assert!(l.observe(s(114), false).is_empty());
        assert_eq!(
            l.observe(s(115), false),
            [Effect::GameEnded { session: s(100) }]
        );
        assert_eq!(l.state(), State::Idle);
        assert!(!l.restore_hotkey_armed());
    }

    #[test]
    fn handoff_inside_grace_returns_to_running() {
        let mut l = lc();
        l.launch(s(0));
        l.observe(s(1), true);
        l.observe(s(5), false); // stub exited
        assert!(matches!(l.state(), State::Exiting { .. }));
        assert!(l.observe(s(8), true).is_empty()); // real game appeared
        assert_eq!(l.state(), State::Running { since: s(1) });
        l.observe(s(60), false);
        assert_eq!(
            l.observe(s(65), false),
            [Effect::GameEnded { session: s(59) }]
        );
    }

    #[test]
    fn still_waiting_then_timeout() {
        let mut l = lc();
        l.launch(s(0));
        assert!(l.observe(s(19), false).is_empty());
        assert_eq!(l.observe(s(20), false), [Effect::ShowStillWaiting]);
        assert!(l.observe(s(21), false).is_empty()); // shown once
        assert_eq!(
            l.observe(s(180), false),
            [Effect::LaunchAbandoned {
                reason: AbandonReason::TimedOut
            }]
        );
        assert_eq!(l.state(), State::Idle);
    }

    #[test]
    fn cancel_only_while_launching() {
        let mut l = lc();
        assert!(l.cancel().is_empty());
        l.launch(s(0));
        assert_eq!(
            l.cancel(),
            [Effect::LaunchAbandoned {
                reason: AbandonReason::Cancelled
            }]
        );
        l.launch(s(0));
        l.observe(s(1), true);
        assert!(l.cancel().is_empty());
        assert!(matches!(l.state(), State::Running { .. }));
    }

    #[test]
    fn launch_ignored_when_busy() {
        let mut l = lc();
        l.launch(s(0));
        assert!(l.launch(s(1)).is_empty());
    }
}
