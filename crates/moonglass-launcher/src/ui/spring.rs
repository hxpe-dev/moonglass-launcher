//! A critically damped spring, stepped once per frame (spec 4.1: GPUI has
//! no spring physics). Critically damped means it reaches the target as
//! fast as possible without overshooting, which reads as "tactile" without
//! wobble.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    /// Angular frequency; higher is snappier. ~18 settles in about 0.25 s.
    pub omega: f32,
}

impl Spring {
    pub fn new(value: f32, omega: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            omega,
        }
    }

    /// Advances by `dt` seconds using the exact solution of the critically
    /// damped oscillator, so it is stable at any frame rate (60 or 120 Hz,
    /// or a long frame after a stall).
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        let x0 = self.value - self.target;
        let v0 = self.velocity;
        let w = self.omega;
        let e = (-w * dt).exp();
        let x = (x0 + (v0 + w * x0) * dt) * e;
        let v = (v0 - w * (v0 + w * x0) * dt) * e;
        self.value = self.target + x;
        self.velocity = v;
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.0005 && self.velocity.abs() < 0.005
    }

    /// Jumps without animating (Reduced Motion, or a filter change).
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_without_overshoot() {
        let mut s = Spring::new(0.0, 18.0);
        s.target = 1.0;
        let mut max: f32 = 0.0;
        for _ in 0..120 {
            s.step(1.0 / 120.0);
            max = max.max(s.value);
        }
        assert!(s.is_settled());
        assert_eq!(s.value, 1.0);
        assert!(max <= 1.0 + 1e-4, "overshoot {max}");
    }

    #[test]
    fn frame_rate_independent() {
        let run = |hz: f32| {
            let mut s = Spring::new(0.0, 18.0);
            s.target = 5.0;
            for _ in 0..(hz as usize / 10) {
                s.step(1.0 / hz);
            }
            s.value
        };
        assert!((run(60.0) - run(120.0)).abs() < 1e-3);
    }
}
