//! Animated values for everything that is not the carousel's own motion:
//! highlights moving between rows, panels and sheets appearing, toggles
//! sliding, cards gliding to a new place when the list changes.
//!
//! Drawing code asks for a value by key and says where it should be; the
//! value glides there on a critically damped [`Spring`] over the next
//! frames. Nothing is registered or cleaned up by hand: a key that a frame
//! does not ask for is forgotten, so the next time it appears it starts
//! again (which is what makes "appear" animations replay).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::spring::Spring;

/// Settles in about 0.2 s.
const OMEGA: f32 = 22.0;

struct Slot {
    spring: Spring,
    /// Asked for since the last [`Anims::step`].
    used: bool,
}

pub struct Anims {
    slots: HashMap<u64, Slot>,
    /// False for Reduced Motion: every value jumps to its target.
    pub enabled: bool,
}

fn hash(key: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

impl Anims {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            enabled: true,
        }
    }

    /// The value of `key`, gliding toward `target`. A key seen for the
    /// first time is already at `target`.
    pub fn to(&mut self, key: impl Hash, target: f32) -> f32 {
        self.glide_from(key, target, target)
    }

    /// Like [`to`](Self::to), but a key seen for the first time starts at
    /// `initial`: `glide_from(key, 0.0, 1.0)` is an appear animation.
    pub fn glide_from(&mut self, key: impl Hash, initial: f32, target: f32) -> f32 {
        let slot = self.slots.entry(hash(key)).or_insert_with(|| Slot {
            spring: Spring::new(initial, OMEGA),
            used: true,
        });
        slot.used = true;
        slot.spring.target = target;
        if !self.enabled {
            slot.spring.snap(target);
        }
        slot.spring.value
    }

    /// Puts `key` at `value` at once, to replay an animation.
    pub fn set(&mut self, key: impl Hash, value: f32) {
        if let Some(slot) = self.slots.get_mut(&hash(key)) {
            slot.spring.snap(value);
        }
    }

    /// Once per frame, before drawing: forgets the values the last frame did
    /// not use and advances the rest by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        self.slots.retain(|_, s| std::mem::take(&mut s.used));
        for s in self.slots.values_mut() {
            s.spring.step(dt);
        }
    }

    /// After drawing: whether another frame is needed.
    pub fn is_moving(&self) -> bool {
        self.slots
            .values()
            .any(|s| s.used && !s.spring.is_settled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(a: &mut Anims, key: &str, target: f32, n: usize) -> f32 {
        let mut v = 0.0;
        for _ in 0..n {
            a.step(1.0 / 60.0);
            v = a.to(key, target);
        }
        v
    }

    #[test]
    fn glides_to_the_target_and_stops() {
        let mut a = Anims::new();
        assert_eq!(a.to("x", 0.0), 0.0);
        a.step(1.0 / 60.0);
        assert_eq!(a.to("x", 1.0), 0.0);
        assert!(a.is_moving());
        let mid = frames(&mut a, "x", 1.0, 4);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert_eq!(frames(&mut a, "x", 1.0, 60), 1.0);
        assert!(!a.is_moving());
    }

    #[test]
    fn appear_replays_after_a_frame_without_the_key() {
        let mut a = Anims::new();
        assert_eq!(a.glide_from("sheet", 0.0, 1.0), 0.0);
        for _ in 0..60 {
            a.step(1.0 / 60.0);
            a.glide_from("sheet", 0.0, 1.0);
        }
        assert_eq!(a.glide_from("sheet", 0.0, 1.0), 1.0);
        a.step(1.0 / 60.0); // drawn
        a.step(1.0 / 60.0); // a frame that did not draw it
        assert_eq!(a.glide_from("sheet", 0.0, 1.0), 0.0);
    }

    #[test]
    fn reduced_motion_jumps() {
        let mut a = Anims::new();
        a.enabled = false;
        assert_eq!(a.glide_from("x", 0.0, 1.0), 1.0);
        assert!(!a.is_moving());
    }
}
