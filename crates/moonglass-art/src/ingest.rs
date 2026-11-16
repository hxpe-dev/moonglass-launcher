//! Ingestion, once per asset (spec 5.2): resize the cover, extract colors,
//! crop one image per card shape, produce the three pre-blurred hero
//! variants, and copy everything into Moonglass's cache so that changes in a
//! launcher's own cache cannot break it.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView};

use crate::color::{self, Palette};
use crate::{placeholder, ArtError, FoundArt};

/// Width of the blurred background textures. They are stretched by the
/// GPU, so a small texture blurred on the CPU looks the same as a large one
/// and costs little memory.
pub const BLUR_WIDTH: u32 = 480;
/// Gaussian sigmas, in pixels of the `BLUR_WIDTH` texture, for the three
/// user-selectable strengths (Settings → Appearance).
pub const BLUR_SIGMAS: [f32; 3] = [6.0, 12.0, 24.0];

#[derive(Debug, Clone, Copy)]
pub struct IngestConfig {
    /// Largest image height the carousel can show (the focused card on a
    /// 2160p screen).
    pub cover_max_height: u32,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            cover_max_height: 1100,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ingested {
    pub cover_path: PathBuf,
    pub hero_path: PathBuf,
    pub logo_path: Option<PathBuf>,
    pub dominant: String,
    pub accent: String,
    /// The cover is a generated gradient: the UI must draw the title.
    pub is_placeholder: bool,
}

/// The card shapes of the carousel. A card's image is cropped to its shape
/// here, at ingestion: the renderer rounds an image's own corners, so an
/// image wider or taller than its card would show square corners past the
/// card's outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardCrop {
    /// 2:3, from the cover.
    Poster,
    /// 1:1, from the cover.
    Square,
    /// 16:9, from the hero art.
    Banner,
}

impl CardCrop {
    const ALL: [CardCrop; 3] = [CardCrop::Poster, CardCrop::Square, CardCrop::Banner];

    fn aspect(self) -> (u32, u32) {
        match self {
            CardCrop::Poster => (2, 3),
            CardCrop::Square => (1, 1),
            CardCrop::Banner => (16, 9),
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            CardCrop::Poster => "card_poster.jpg",
            CardCrop::Square => "card_square.jpg",
            CardCrop::Banner => "card_banner.jpg",
        }
    }
}

/// Path of the image for a card shape, derived from the cover path so the
/// schema needs no extra columns.
pub fn card_image_path(cover_path: &Path, crop: CardCrop) -> PathBuf {
    cover_path.with_file_name(crop.file_name())
}

/// The largest centered `aspect` rectangle of `img`.
fn center_crop(img: &DynamicImage, (aw, ah): (u32, u32)) -> DynamicImage {
    let (w, h) = img.dimensions();
    let (cw, ch) = if w as u64 * ah as u64 > h as u64 * aw as u64 {
        ((h as u64 * aw as u64 / ah as u64) as u32, h)
    } else {
        (w, (w as u64 * ah as u64 / aw as u64) as u32)
    };
    let (cw, ch) = (cw.clamp(1, w), ch.clamp(1, h));
    img.crop_imm((w - cw) / 2, (h - ch) / 2, cw, ch)
}

/// Scales `img` down to `max_height` if it is taller.
fn fit_height(img: DynamicImage, max_height: u32) -> DynamicImage {
    if img.height() <= max_height {
        return img;
    }
    let w = (img.width() as u64 * max_height as u64 / img.height() as u64) as u32;
    img.resize_exact(w.max(1), max_height, FilterType::Lanczos3)
}

/// Whether a stored cover is the generated placeholder (the only cover
/// saved as `cover.png`), on which the UI draws the title.
pub fn is_placeholder_cover(cover_path: &Path) -> bool {
    cover_path.file_name().is_some_and(|n| n == "cover.png")
}

/// Path of a pre-blurred variant, derived from the hero path so the schema
/// needs no extra columns. `level` is 0, 1 or 2.
pub fn blurred_hero_path(hero_path: &Path, level: usize) -> PathBuf {
    hero_path.with_file_name(format!("hero_blur_{}.jpg", level.min(2)))
}

fn open(path: &Path) -> Result<DynamicImage, ArtError> {
    image::ImageReader::open(path)
        .map_err(|e| ArtError::io(path, e))?
        .with_guessed_format()
        .map_err(|e| ArtError::io(path, e))?
        .decode()
        .map_err(|e| ArtError::image(path, e))
}

fn save_jpeg(img: &DynamicImage, path: &Path, quality: u8) -> Result<(), ArtError> {
    let file = File::create(path).map_err(|e| ArtError::io(path, e))?;
    let enc = JpegEncoder::new_with_quality(BufWriter::new(file), quality);
    img.to_rgb8()
        .write_with_encoder(enc)
        .map_err(|e| ArtError::image(path, e))
}

fn palette_of(img: &DynamicImage) -> Palette {
    color::extract(&img.thumbnail(48, 72).to_rgb8())
}

pub fn ingest(
    found: &FoundArt,
    title: &str,
    out_dir: &Path,
    cfg: IngestConfig,
) -> Result<Ingested, ArtError> {
    std::fs::create_dir_all(out_dir).map_err(|e| ArtError::io(out_dir, e))?;

    // Cover: resized to what the carousel can display, or a placeholder.
    let (cover_img, cover_path, is_placeholder) = match &found.cover {
        Some(src) => {
            let img = fit_height(open(src)?, cfg.cover_max_height);
            let path = out_dir.join("cover.jpg");
            save_jpeg(&img, &path, 90)?;
            (img, path, false)
        }
        None => {
            let path = out_dir.join("cover.png");
            placeholder::write(title, 600, 900, &path)?;
            (open(&path)?, path, true)
        }
    };
    let pal = palette_of(&cover_img);

    // Hero: the game's hero art, or the cover when there is none.
    let (hero_img, hero_path) = match &found.hero {
        Some(src) => {
            let img = open(src)?;
            let path = out_dir.join("hero.jpg");
            save_jpeg(&img, &path, 90)?;
            (img, path)
        }
        None => (cover_img.clone(), cover_path.clone()),
    };
    // One image per card shape, cropped to fit it exactly.
    for crop in CardCrop::ALL {
        let source = if crop == CardCrop::Banner {
            &hero_img
        } else {
            &cover_img
        };
        let card = fit_height(center_crop(source, crop.aspect()), cfg.cover_max_height);
        save_jpeg(&card, &card_image_path(&cover_path, crop), 90)?;
    }

    let (w, h) = hero_img.dimensions();
    let small_h = ((h as u64 * BLUR_WIDTH as u64) / w.max(1) as u64).max(1) as u32;
    let small = hero_img.resize_exact(BLUR_WIDTH, small_h, FilterType::Triangle);
    for (level, sigma) in BLUR_SIGMAS.iter().enumerate() {
        let blurred = DynamicImage::ImageRgb8(image::imageops::blur(&small.to_rgb8(), *sigma));
        let target = out_dir.join(format!("hero_blur_{level}.jpg"));
        save_jpeg(&blurred, &target, 85)?;
    }

    let logo_path = match &found.logo {
        Some(src) => {
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png");
            let path = out_dir.join(format!("logo.{ext}"));
            if src != &path {
                std::fs::copy(src, &path).map_err(|e| ArtError::io(src, e))?;
            }
            Some(path)
        }
        None => None,
    };

    Ok(Ingested {
        cover_path,
        hero_path,
        logo_path,
        dominant: pal.dominant.to_hex(),
        accent: pal.accent.to_hex(),
        is_placeholder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn write_img(path: &Path, w: u32, h: u32, px: [u8; 3]) {
        RgbImage::from_pixel(w, h, Rgb(px)).save(path).unwrap();
    }

    #[test]
    fn full_ingest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_img(&src.join("cover.png"), 600, 1800, [200, 40, 40]);
        write_img(&src.join("hero.png"), 1920, 620, [40, 40, 200]);
        write_img(&src.join("logo.png"), 400, 100, [255, 255, 255]);
        let found = FoundArt {
            cover: Some(src.join("cover.png")),
            hero: Some(src.join("hero.png")),
            logo: Some(src.join("logo.png")),
        };
        let out = tmp.path().join("out");
        let r = ingest(
            &found,
            "Test",
            &out,
            IngestConfig {
                cover_max_height: 900,
            },
        )
        .unwrap();
        assert!(!r.is_placeholder);
        let cover = image::open(&r.cover_path).unwrap();
        assert_eq!((cover.width(), cover.height()), (300, 900));
        // Each card image has exactly its shape, whatever the source's.
        let size = |crop| {
            let i = image::open(card_image_path(&r.cover_path, crop)).unwrap();
            (i.width(), i.height())
        };
        assert_eq!(size(CardCrop::Poster), (300, 450));
        assert_eq!(size(CardCrop::Square), (300, 300));
        assert_eq!(size(CardCrop::Banner), (1102, 620));
        for level in 0..3 {
            let b = image::open(blurred_hero_path(&r.hero_path, level)).unwrap();
            assert_eq!(b.width(), BLUR_WIDTH);
        }
        assert_eq!(r.logo_path, Some(out.join("logo.png")));
        // Red-ish cover gives a red-ish dominant color.
        let d = color::to_hsl(color::Rgb::from_hex(&r.dominant).unwrap());
        assert!(!(30.0..330.0).contains(&d.h), "hue {}", d.h);
    }

    #[test]
    fn placeholder_when_nothing_found() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ingest(
            &FoundArt::default(),
            "Celeste",
            tmp.path(),
            IngestConfig::default(),
        )
        .unwrap();
        assert!(r.is_placeholder);
        assert_eq!(r.hero_path, r.cover_path);
        assert!(blurred_hero_path(&r.hero_path, 2).is_file());
        assert_eq!(r.logo_path, None);
    }
}
