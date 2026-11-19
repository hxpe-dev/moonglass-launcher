//! Deterministic game IDs (spec section 6).
//!
//! * Steam: `steam:<AppID>`
//! * Epic: `epic:<AppName>`
//! * GOG: `gog:<product ID>`
//! * Battle.net: `battlenet:<product UID>` (`s2`, `wow`, …)
//! * Microsoft Store: `msstore:<package family name>`
//! * Executable: `exe:<hash of normalized path>`
//! * ROM: `rom:<hash of normalized path>`
//!
//! Path normalization (symlinks, relative segments, case folding on Windows)
//! is the platform layer's job; the functions here take the already
//! normalized path so the result is identical on every platform.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GameId(String);

impl GameId {
    pub fn steam(app_id: &str) -> Self {
        Self(format!("steam:{app_id}"))
    }

    pub fn epic(app_name: &str) -> Self {
        Self(format!("epic:{app_name}"))
    }

    pub fn gog(product_id: &str) -> Self {
        Self(format!("gog:{product_id}"))
    }

    pub fn battlenet(uid: &str) -> Self {
        Self(format!("battlenet:{uid}"))
    }

    pub fn msstore(package_family: &str) -> Self {
        Self(format!("msstore:{package_family}"))
    }

    /// `normalized_path` must come from `Platform::normalize_path`.
    pub fn exe(normalized_path: &str) -> Self {
        Self(format!("exe:{}", path_hash(normalized_path)))
    }

    /// `normalized_path` must come from `Platform::normalize_path`.
    pub fn rom(normalized_path: &str) -> Self {
        Self(format!("rom:{}", path_hash(normalized_path)))
    }

    /// Wraps an ID read back from the database.
    pub fn from_stored(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 64-bit FNV-1a over the UTF-8 bytes, as 16 lowercase hex digits.
///
/// Chosen because it is stable across Rust versions and platforms (unlike
/// `std::hash`), needs no dependency, and collisions within one user's
/// library are not a realistic concern at 64 bits.
pub fn path_hash(normalized_path: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in normalized_path.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(PRIME);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_ids() {
        assert_eq!(GameId::steam("620").as_str(), "steam:620");
        assert_eq!(GameId::epic("Fortnite").as_str(), "epic:Fortnite");
        assert_eq!(GameId::gog("1207658924").as_str(), "gog:1207658924");
        assert_eq!(GameId::battlenet("s2").as_str(), "battlenet:s2");
        assert_eq!(
            GameId::msstore("Microsoft.ForteBaseGame_8wekyb3d8bbwe").as_str(),
            "msstore:Microsoft.ForteBaseGame_8wekyb3d8bbwe"
        );
    }

    #[test]
    fn path_ids_are_stable_and_distinct() {
        let a = GameId::exe("c:\\games\\a.exe");
        assert_eq!(a, GameId::exe("c:\\games\\a.exe"));
        assert_ne!(a, GameId::exe("c:\\games\\b.exe"));
        assert!(a.as_str().starts_with("exe:"));
        assert_eq!(a.as_str().len(), "exe:".len() + 16);
        // Same path registered as a ROM is a different game.
        assert_ne!(a, GameId::rom("c:\\games\\a.exe"));
    }

    #[test]
    fn fnv_reference_values() {
        // Published FNV-1a 64 test vectors.
        assert_eq!(path_hash(""), "cbf29ce484222325");
        assert_eq!(path_hash("a"), "af63dc4c8601ec8c");
        assert_eq!(path_hash("foobar"), "85944171f73967e8");
    }
}
