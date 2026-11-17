//! Releasing images that are no longer drawn (spec 5.2, At runtime).
//!
//! GPUI keeps every image it has decoded, in memory and in the GPU's sprite
//! atlas, for as long as the app runs. Scrolling through a large library
//! would keep every cover and background ever shown. Each image drawn here
//! is noted with the time; images not drawn for a while are dropped from
//! both, and decoded again (off the UI thread) if they come back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{img, Context, Img, ImgResourceLoader, Resource, Window};

use super::LauncherView;

/// An image not drawn for this long is released.
pub const IDLE_RELEASE: Duration = Duration::from_secs(20);
/// How often the carousel looks for images to release.
pub const RELEASE_CHECK: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct DrawnImages {
    last_drawn: HashMap<PathBuf, Instant>,
    last_check: Option<Instant>,
}

impl DrawnImages {
    fn note(&mut self, path: PathBuf) {
        self.last_drawn.insert(path, Instant::now());
    }

    /// Takes the images not drawn within `idle`.
    fn take_idle(&mut self, idle: Duration) -> Vec<PathBuf> {
        let now = Instant::now();
        let old: Vec<PathBuf> = self
            .last_drawn
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= idle)
            .map(|(p, _)| p.clone())
            .collect();
        for p in &old {
            self.last_drawn.remove(p);
        }
        old
    }

    fn due(&mut self) -> bool {
        let due = self.last_check.is_none_or(|t| t.elapsed() >= RELEASE_CHECK);
        if due {
            self.last_check = Some(Instant::now());
        }
        due
    }

    pub fn len(&self) -> usize {
        self.last_drawn.len()
    }
}

impl LauncherView {
    /// An image element for a file, noted as drawn now.
    pub(super) fn image(&self, path: PathBuf) -> Img {
        self.drawn_images.borrow_mut().note(path.clone());
        img(path)
    }

    /// Releases the images not drawn within `idle`.
    pub(super) fn release_images(
        &mut self,
        idle: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old = self.drawn_images.borrow_mut().take_idle(idle);
        if old.is_empty() {
            return;
        }
        for path in &old {
            let resource = Resource::from(path.clone());
            // Only images that were drawn are asked for, so this finds the
            // decoded one rather than starting a new load.
            if let Some(Ok(image)) = window.get_asset::<ImgResourceLoader>(&resource, cx) {
                cx.drop_image(image, Some(window));
            }
            cx.remove_asset::<ImgResourceLoader>(&resource);
        }
        tracing::debug!(
            released = old.len(),
            kept = self.drawn_images.borrow().len(),
            "released images"
        );
    }

    /// Once in a while during frames: releases images idle for a while.
    pub(super) fn release_idle_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drawn_images.borrow_mut().due() {
            self.release_images(IDLE_RELEASE, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_images_are_taken_once() {
        let mut d = DrawnImages::default();
        d.note("a.jpg".into());
        assert!(d.take_idle(Duration::from_secs(60)).is_empty());
        assert_eq!(d.take_idle(Duration::ZERO), vec![PathBuf::from("a.jpg")]);
        assert_eq!(d.len(), 0);
        assert!(d.due());
        assert!(!d.due());
    }
}
