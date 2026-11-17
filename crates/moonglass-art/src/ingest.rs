//! Ingestion, once per asset (spec 5.2): resize the cover, extract colors,
//! make one image per card shape (plus a zoomed-in crop of the cover),
//! produce the three pre-blurred hero variants and the motion-blurred one,
//! and copy everything into Moonglass's cache so that changes in a
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

/// The card shapes of the carousel. A card's image is given its shape
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

    fn fill_file_name(self) -> &'static str {
        match self {
            CardCrop::Poster => "card_poster_fill.jpg",
            CardCrop::Square => "card_square_fill.jpg",
            CardCrop::Banner => "card_banner.jpg",
        }
    }
}

/// Path of the image for a card shape, derived from the cover path so the
/// schema needs no extra columns.
pub fn card_image_path(cover_path: &Path, crop: CardCrop) -> PathBuf {
    cover_path.with_file_name(crop.file_name())
}

/// Path of the image for a card shape zoomed in to fill it: always a
/// center crop, even of a cover much wider than the card (where
/// [`card_image_path`] shows it whole). The banner is a crop either way.
pub fn card_fill_image_path(cover_path: &Path, crop: CardCrop) -> PathBuf {
    cover_path.with_file_name(crop.fill_file_name())
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

/// A cover shaped for a card. Covers about as wide as the card or taller
/// are center-cropped (a 2:3 poster on a square card loses a little top and
/// bottom). A cover much wider than the card (a square Switch icon or a
/// landscape box on a poster card) would lose its sides, often half the
/// title: it is shown whole instead, centered on a blurred, darkened fill
/// made from itself.
fn card_from_cover(img: &DynamicImage, (aw, ah): (u32, u32)) -> DynamicImage {
    let (w, h) = img.dimensions();
    let (src, dst) = (w as f32 / h.max(1) as f32, aw as f32 / ah as f32);
    if src <= dst * 1.2 {
        return center_crop(img, (aw, ah));
    }
    let out_h = (w as u64 * ah as u64 / aw as u64) as u32;
    let small_w = 64u32;
    let small_h = (small_w * ah / aw).max(1);
    let small = center_crop(img, (aw, ah)).resize_exact(small_w, small_h, FilterType::Triangle);
    let mut fill = image::imageops::blur(&small.to_rgb8(), 4.0);
    for p in fill.pixels_mut() {
        for c in p.0.iter_mut() {
            *c = (*c as f32 * 0.55) as u8;
        }
    }
    let mut canvas = DynamicImage::ImageRgb8(fill)
        .resize_exact(w, out_h, FilterType::Triangle)
        .to_rgb8();
    image::imageops::overlay(&mut canvas, &img.to_rgb8(), 0, ((out_h - h) / 2) as i64);
    DynamicImage::ImageRgb8(canvas)
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

fn write_fill_card(
    cover: &DynamicImage,
    cover_path: &Path,
    crop: CardCrop,
    cfg: IngestConfig,
) -> Result<(), ArtError> {
    let card = fit_height(center_crop(cover, crop.aspect()), cfg.cover_max_height);
    save_jpeg(&card, &card_fill_image_path(cover_path, crop), 90)
}

/// Makes the zoomed-in card images for artwork ingested before they
/// existed, from the stored cover. Does nothing when they are there.
pub fn ensure_fill_cards(cover_path: &Path) -> Result<(), ArtError> {
    let missing: Vec<CardCrop> = [CardCrop::Poster, CardCrop::Square]
        .into_iter()
        .filter(|c| !card_fill_image_path(cover_path, *c).is_file())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let cover = open(cover_path)?;
    for crop in missing {
        write_fill_card(&cover, cover_path, crop, IngestConfig::default())?;
    }
    Ok(())
}

/// Width of the motion-blurred background texture: wider than the blur
/// textures, since the art is only smeared sideways and stays recognizable.
pub const MOTION_WIDTH: u32 = 960;

/// Path of the horizontally motion-blurred hero, derived like
/// [`blurred_hero_path`].
pub fn motion_hero_path(hero_path: &Path) -> PathBuf {
    hero_path.with_file_name("hero_motion.jpg")
}

/// Smears `img` sideways, as a fast horizontal camera pan would: three
/// box blurs along each row (close to a Gaussian), none vertically.
fn motion_blur(img: &DynamicImage) -> image::RgbImage {
    let (w, h) = img.dimensions();
    let small_h = ((h as u64 * MOTION_WIDTH as u64) / w.max(1) as u64).max(1) as u32;
    let mut buf = img
        .resize_exact(MOTION_WIDTH, small_h, FilterType::Triangle)
        .to_rgb8();
    let width = buf.width() as i64;
    let radius = (MOTION_WIDTH / 40) as i64;
    let mut row = vec![[0u32; 3]; width as usize];
    for _pass in 0..3 {
        for y in 0..buf.height() {
            for x in 0..width {
                row[x as usize] = buf.get_pixel(x as u32, y).0.map(u32::from);
            }
            // Running sum over [x - r, x + r], edges clamped.
            let at = |i: i64| row[i.clamp(0, width - 1) as usize];
            let mut sum = [0u32; 3];
            for i in -radius..=radius {
                let p = at(i);
                (0..3).for_each(|c| sum[c] += p[c]);
            }
            let n = (2 * radius + 1) as u32;
            for x in 0..width {
                let px = image::Rgb(sum.map(|s| (s / n) as u8));
                buf.put_pixel(x as u32, y, px);
                let (add, sub) = (at(x + radius + 1), at(x - radius));
                (0..3).for_each(|c| sum[c] = sum[c] + add[c] - sub[c]);
            }
        }
    }
    buf
}

fn write_motion(hero: &DynamicImage, path: &Path) -> Result<(), ArtError> {
    save_jpeg(&DynamicImage::ImageRgb8(motion_blur(hero)), path, 85)
}

/// Makes the motion-blurred texture for artwork ingested before it
/// existed. Does nothing when it is already there.
pub fn ensure_motion_hero(hero_path: &Path) -> Result<PathBuf, ArtError> {
    let path = motion_hero_path(hero_path);
    if !path.is_file() {
        write_motion(&open(hero_path)?, &path)?;
    }
    Ok(path)
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
    // One image per card shape, with exactly its proportions.
    for crop in CardCrop::ALL {
        let shaped = if crop == CardCrop::Banner {
            center_crop(&hero_img, crop.aspect())
        } else {
            card_from_cover(&cover_img, crop.aspect())
        };
        let card = fit_height(shaped, cfg.cover_max_height);
        save_jpeg(&card, &card_image_path(&cover_path, crop), 90)?;
        if crop != CardCrop::Banner {
            write_fill_card(&cover_img, &cover_path, crop, cfg)?;
        }
    }

    let (w, h) = hero_img.dimensions();
    let small_h = ((h as u64 * BLUR_WIDTH as u64) / w.max(1) as u64).max(1) as u32;
    let small = hero_img.resize_exact(BLUR_WIDTH, small_h, FilterType::Triangle);
    for (level, sigma) in BLUR_SIGMAS.iter().enumerate() {
        let blurred = DynamicImage::ImageRgb8(image::imageops::blur(&small.to_rgb8(), *sigma));
        let target = out_dir.join(format!("hero_blur_{level}.jpg"));
        save_jpeg(&blurred, &target, 85)?;
    }

    write_motion(&hero_img, &motion_hero_path(&hero_path))?;

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
        assert!(motion_hero_path(&r.hero_path).is_file());
        assert_eq!(r.logo_path, Some(out.join("logo.png")));
        // Red-ish cover gives a red-ish dominant color.
        let d = color::to_hsl(color::Rgb::from_hex(&r.dominant).unwrap());
        assert!(!(30.0..330.0).contains(&d.h), "hue {}", d.h);
    }

    #[test]
    fn wide_covers_are_shown_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("icon.png");
        write_img(&src, 600, 600, [250, 250, 250]);
        let found = FoundArt {
            cover: Some(src),
            ..Default::default()
        };
        let r = ingest(&found, "Icon", tmp.path(), IngestConfig::default()).unwrap();
        let poster = image::open(card_image_path(&r.cover_path, CardCrop::Poster))
            .unwrap()
            .to_rgb8();
        assert_eq!((poster.width(), poster.height()), (600, 900));
        // The icon is whole in the middle; the fill above it is darker.
        assert!(poster.get_pixel(300, 450).0[0] > 230);
        assert!(poster.get_pixel(300, 20).0[0] < 160);
        // Zoomed in, the same icon fills the whole card.
        let fill = image::open(card_fill_image_path(&r.cover_path, CardCrop::Poster))
            .unwrap()
            .to_rgb8();
        assert_eq!((fill.width(), fill.height()), (400, 600));
        assert!(fill.get_pixel(200, 5).0[0] > 230);
    }

    #[test]
    fn motion_blur_is_horizontal() {
        // Left half black, right half white, split by a vertical edge.
        let mut img = RgbImage::new(200, 100);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = Rgb(if x < 100 { [0; 3] } else { [255; 3] });
        }
        let out = motion_blur(&DynamicImage::ImageRgb8(img));
        let mid = out.width() / 2;
        let v = |x: u32, y: u32| out.get_pixel(x, y).0[0];
        // Smeared across the edge, identical down each column.
        assert!(v(mid, 10) > 60 && v(mid, 10) < 200);
        assert_eq!(v(mid, 10), v(mid, out.height() - 10));
        assert!(v(5, 10) < 5 && v(out.width() - 5, 10) > 250);
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
