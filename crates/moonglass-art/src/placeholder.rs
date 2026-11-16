//! Generated placeholder: a gradient derived from a hash of the title. The
//! UI draws the game's name on top (Moonglass already renders text, so the
//! image itself carries no glyphs).

use std::path::Path;

use image::{Rgb as Px, RgbImage};

use crate::color::{from_hsl, Hsl, Rgb};
use crate::ArtError;

/// The two gradient stops for a title: stable for the same title, spread
/// across the hue wheel for different ones.
pub fn gradient_for(title: &str) -> (Rgb, Rgb) {
    let h = moonglass_core::id::path_hash(title);
    let n = u64::from_str_radix(&h, 16).unwrap_or(0);
    let hue = (n % 360) as f32;
    let top = from_hsl(Hsl {
        h: hue,
        s: 0.55,
        l: 0.42,
    });
    let bottom = from_hsl(Hsl {
        h: (hue + 40.0) % 360.0,
        s: 0.60,
        l: 0.16,
    });
    (top, bottom)
}

/// Writes a `width`×`height` vertical gradient PNG.
pub fn write(title: &str, width: u32, height: u32, path: &Path) -> Result<(), ArtError> {
    let (top, bottom) = gradient_for(title);
    let img = RgbImage::from_fn(width, height, |_, y| {
        let t = y as f32 / height.saturating_sub(1).max(1) as f32;
        let mix = |a: f32, b: f32| ((a + (b - a) * t) * 255.0).round() as u8;
        Px([
            mix(top.0, bottom.0),
            mix(top.1, bottom.1),
            mix(top.2, bottom.2),
        ])
    });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ArtError::io(dir, e))?;
    }
    img.save(path).map_err(|e| ArtError::image(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_distinct() {
        assert_eq!(gradient_for("Celeste"), gradient_for("Celeste"));
        assert_ne!(gradient_for("Celeste"), gradient_for("Hades"));
    }

    #[test]
    fn writes_png() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x/placeholder.png");
        write("Celeste", 60, 90, &p).unwrap();
        let img = image::open(&p).unwrap();
        assert_eq!((img.width(), img.height()), (60, 90));
    }
}
