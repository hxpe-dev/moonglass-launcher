//! Dominant and accent colors (spec 5.2): k-means over a downscaled image,
//! then clamped to a minimum saturation and a lightness band so dark or grey
//! covers still produce a visible glow.

use image::RgbImage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb(pub f32, pub f32, pub f32); // 0..=1

impl Rgb {
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Self(
            ((v >> 16) & 0xff) as f32 / 255.0,
            ((v >> 8) & 0xff) as f32 / 255.0,
            (v & 0xff) as f32 / 255.0,
        ))
    }

    pub fn to_hex(self) -> String {
        let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(self.0), c(self.1), c(self.2))
    }

    fn dist2(self, o: Rgb) -> f32 {
        (self.0 - o.0).powi(2) + (self.1 - o.1).powi(2) + (self.2 - o.2).powi(2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsl {
    pub h: f32, // degrees 0..360
    pub s: f32,
    pub l: f32,
}

pub fn to_hsl(c: Rgb) -> Hsl {
    let (r, g, b) = (c.0, c.1, c.2);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return Hsl { h: 0.0, s: 0.0, l };
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    Hsl { h, s, l }
}

pub fn from_hsl(c: Hsl) -> Rgb {
    let chroma = (1.0 - (2.0 * c.l - 1.0).abs()) * c.s;
    let hp = c.h.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = c.l - chroma / 2.0;
    Rgb(r + m, g + m, b + m)
}

/// Glow-friendly limits.
pub const MIN_SATURATION: f32 = 0.35;
pub const MIN_LIGHTNESS: f32 = 0.35;
pub const MAX_LIGHTNESS: f32 = 0.65;

/// Clamps saturation up and lightness into the band. A fully grey color
/// keeps its (meaningless) hue of 0, so greys become a muted red-free blue
/// instead: hue 220 reads as neutral on a dark UI.
pub fn clamp_for_glow(c: Rgb) -> Rgb {
    let mut hsl = to_hsl(c);
    if hsl.s < 0.05 {
        hsl.h = 220.0;
    }
    hsl.s = hsl.s.max(MIN_SATURATION);
    hsl.l = hsl.l.clamp(MIN_LIGHTNESS, MAX_LIGHTNESS);
    from_hsl(hsl)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dominant: Rgb,
    pub accent: Rgb,
}

/// k-means (k = 5, deterministic seeding) over the pixels of a small image.
/// Dominant is the largest cluster; accent is the most saturated of the
/// others with a meaningful share, falling back to the dominant color.
pub fn extract(img: &RgbImage) -> Palette {
    let pixels: Vec<Rgb> = img
        .pixels()
        .map(|p| {
            Rgb(
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            )
        })
        .collect();
    if pixels.is_empty() {
        let grey = clamp_for_glow(Rgb(0.5, 0.5, 0.5));
        return Palette {
            dominant: grey,
            accent: grey,
        };
    }
    let k = 5.min(pixels.len());
    // Seed with evenly spaced pixels: deterministic, so a rescan never
    // changes a game's colors.
    let mut centers: Vec<Rgb> = (0..k).map(|i| pixels[i * pixels.len() / k]).collect();
    let mut assign = vec![0usize; pixels.len()];
    for _ in 0..12 {
        let mut changed = false;
        for (i, p) in pixels.iter().enumerate() {
            let best = (0..k)
                .min_by(|&a, &b| p.dist2(centers[a]).total_cmp(&p.dist2(centers[b])))
                .unwrap_or(0);
            if assign[i] != best {
                assign[i] = best;
                changed = true;
            }
        }
        let mut sums = vec![(0.0f32, 0.0f32, 0.0f32, 0usize); k];
        for (p, &a) in pixels.iter().zip(&assign) {
            let s = &mut sums[a];
            s.0 += p.0;
            s.1 += p.1;
            s.2 += p.2;
            s.3 += 1;
        }
        for (c, s) in centers.iter_mut().zip(&sums) {
            if s.3 > 0 {
                let n = s.3 as f32;
                *c = Rgb(s.0 / n, s.1 / n, s.2 / n);
            }
        }
        if !changed {
            break;
        }
    }
    let mut counts = vec![0usize; k];
    for &a in &assign {
        counts[a] += 1;
    }
    let dominant_i = (0..k).max_by_key(|&i| counts[i]).unwrap_or(0);
    let min_share = pixels.len() / 20; // 5%
    let accent_i = (0..k)
        .filter(|&i| i != dominant_i && counts[i] >= min_share.max(1))
        .max_by(|&a, &b| to_hsl(centers[a]).s.total_cmp(&to_hsl(centers[b]).s))
        .unwrap_or(dominant_i);
    Palette {
        dominant: clamp_for_glow(centers[dominant_i]),
        accent: clamp_for_glow(centers[accent_i]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        assert_eq!(Rgb::from_hex("#1A2B3C").unwrap().to_hex(), "#1A2B3C");
        assert!(Rgb::from_hex("123456").is_none());
    }

    #[test]
    fn hsl_round_trip() {
        for c in [
            Rgb(1.0, 0.0, 0.0),
            Rgb(0.2, 0.6, 0.4),
            Rgb(0.1, 0.1, 0.9),
            Rgb(0.5, 0.5, 0.5),
        ] {
            let back = from_hsl(to_hsl(c));
            assert!(c.dist2(back) < 1e-6, "{c:?} -> {back:?}");
        }
    }

    #[test]
    fn dark_and_grey_covers_still_glow() {
        for c in [
            Rgb(0.02, 0.02, 0.03),
            Rgb(0.5, 0.5, 0.5),
            Rgb(1.0, 1.0, 1.0),
        ] {
            let hsl = to_hsl(clamp_for_glow(c));
            assert!(hsl.s >= MIN_SATURATION - 1e-3);
            assert!((MIN_LIGHTNESS - 1e-3..=MAX_LIGHTNESS + 1e-3).contains(&hsl.l));
        }
    }

    #[test]
    fn picks_largest_and_most_saturated() {
        // 70% dull blue-grey, 20% bright orange, 10% black.
        let mut img = RgbImage::new(10, 10);
        for (i, p) in img.pixels_mut().enumerate() {
            *p = match i {
                0..=69 => image::Rgb([90, 100, 120]),
                70..=89 => image::Rgb([250, 140, 20]),
                _ => image::Rgb([0, 0, 0]),
            };
        }
        let pal = extract(&img);
        let d = to_hsl(pal.dominant);
        let a = to_hsl(pal.accent);
        assert!((200.0..240.0).contains(&d.h), "dominant hue {}", d.h);
        assert!((20.0..45.0).contains(&a.h), "accent hue {}", a.h);
    }
}
