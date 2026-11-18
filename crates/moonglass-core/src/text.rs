//! Title normalization for sorting and search (spec sections 3 and 6).

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

const LEADING_ARTICLES: [&str; 3] = ["the ", "a ", "an "];

/// `sort_title`: lowercased, leading article removed.
///
/// "The Witcher 3" sorts under "w". A title that is only an article
/// ("The") is left alone.
pub fn sort_title(title: &str) -> String {
    let lower = title.trim().to_lowercase();
    for article in LEADING_ARTICLES {
        if let Some(rest) = lower.strip_prefix(article) {
            let rest = rest.trim_start();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    lower
}

/// `search_title`: lowercased, accents and punctuation stripped, words
/// separated by single spaces.
///
/// Punctuation is removed rather than turned into a space, so "Assassin's"
/// becomes "assassins". Word boundaries are kept so the stored value stays
/// readable; [`matches()`] ignores spaces when comparing.
pub fn search_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_space = false;
    for c in title.nfd().filter(|c| !is_combining_mark(*c)) {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            pending_space = true;
        }
        // Anything else is punctuation or a symbol: dropped.
    }
    out
}

/// Normalizes a search query once per keystroke. Spaces are removed, so
/// "half life", "half-life" and "halflife" all find "Half-Life".
pub fn search_key(query: &str) -> String {
    search_title(query).replace(' ', "")
}

/// Case-, accent-, punctuation- and space-insensitive substring match.
/// `key` comes from [`search_key`]; `stored` is a game's `search_title`.
pub fn matches(stored: &str, key: &str) -> bool {
    if key.is_empty() {
        return true;
    }
    if !stored.contains(' ') {
        return stored.contains(key);
    }
    stored.replace(' ', "").contains(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_titles() {
        assert_eq!(sort_title("The Witcher 3"), "witcher 3");
        assert_eq!(sort_title("A Plague Tale"), "plague tale");
        assert_eq!(sort_title("An Untitled Story"), "untitled story");
        assert_eq!(sort_title("Anthem"), "anthem");
        assert_eq!(sort_title("The"), "the");
        assert_eq!(sort_title("  Celeste "), "celeste");
    }

    #[test]
    fn search_titles() {
        assert_eq!(search_title("Pokémon Café Mix"), "pokemon cafe mix");
        assert_eq!(search_title("Assassin's Creed® II"), "assassins creed ii");
        assert_eq!(search_title("Half-Life: Alyx"), "halflife alyx");
        assert_eq!(search_title("  NieR:Automata™  "), "nierautomata");
    }

    #[test]
    fn matching() {
        let stored = search_title("Half-Life 2: Episode One");
        for q in [
            "half",
            "HALF-LIFE",
            "half life",
            "life 2",
            "épisode",
            "e one",
            "",
        ] {
            assert!(matches(&stored, &search_key(q)), "query {q:?}");
        }
        assert!(!matches(&stored, &search_key("portal")));
        assert!(matches(&search_title("Pokémon"), &search_key("pokemon")));
    }
}
