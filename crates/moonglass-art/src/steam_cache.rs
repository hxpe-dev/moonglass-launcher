//! Steam's local library cache: artwork Steam already downloaded, so no
//! network and no API key are needed.
//!
//! Two layouts exist:
//! * current: `appcache/librarycache/<appid>/[<hash>/]library_600x900.jpg`,
//!   `library_capsule.jpg`, `library_hero.jpg`, `logo.png` (and `_2x` variants);
//! * legacy: `appcache/librarycache/<appid>_library_600x900.jpg`,
//!   `<appid>_library_hero.jpg`, `<appid>_logo.png`.

use std::path::{Path, PathBuf};

use crate::FoundArt;

const COVER_NAMES: &[&str] = &[
    "library_600x900_2x.jpg",
    "library_600x900.jpg",
    "library_capsule_2x.jpg",
    "library_capsule.jpg",
];
const HERO_NAMES: &[&str] = &["library_hero_2x.jpg", "library_hero.jpg"];
const LOGO_NAMES: &[&str] = &["logo_2x.png", "logo.png"];

/// Files under the app's folder, at most one hash level deep.
fn app_files(app_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(app_dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Ok(inner) = std::fs::read_dir(&p) {
                out.extend(inner.flatten().map(|e| e.path()).filter(|p| p.is_file()));
            }
        } else {
            out.push(p);
        }
    }
    out
}

fn pick(files: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    // Earlier names are preferred (higher resolution first).
    names.iter().find_map(|n| {
        files
            .iter()
            .find(|f| f.file_name().is_some_and(|f| f.eq_ignore_ascii_case(n)))
            .cloned()
    })
}

pub fn find(steam_root: &Path, app_id: &str) -> FoundArt {
    let cache = steam_root.join("appcache").join("librarycache");
    let files = app_files(&cache.join(app_id));
    let legacy = |suffix: &str| {
        let p = cache.join(format!("{app_id}_{suffix}"));
        p.is_file().then_some(p)
    };
    FoundArt {
        cover: pick(&files, COVER_NAMES).or_else(|| legacy("library_600x900.jpg")),
        hero: pick(&files, HERO_NAMES).or_else(|| legacy("library_hero.jpg")),
        logo: pick(&files, LOGO_NAMES).or_else(|| legacy("logo.png")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn current_layout_prefers_high_res() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("appcache/librarycache/367520");
        touch(&app.join("aaa/library_hero.jpg"));
        touch(&app.join("aaa/library_hero_blur.jpg"));
        touch(&app.join("bbb/library_capsule.jpg"));
        touch(&app.join("ccc/library_600x900.jpg"));
        touch(&app.join("logo.png"));
        let art = find(tmp.path(), "367520");
        assert_eq!(art.cover, Some(app.join("ccc/library_600x900.jpg")));
        assert_eq!(art.hero, Some(app.join("aaa/library_hero.jpg")));
        assert_eq!(art.logo, Some(app.join("logo.png")));
    }

    #[test]
    fn legacy_layout_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("appcache/librarycache");
        touch(&cache.join("620_library_600x900.jpg"));
        let art = find(tmp.path(), "620");
        assert_eq!(art.cover, Some(cache.join("620_library_600x900.jpg")));
        assert_eq!(art.hero, None);
        assert_eq!(find(tmp.path(), "1").cover, None);
    }
}
