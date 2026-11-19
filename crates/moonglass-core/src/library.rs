//! The game model, carousel ordering, filter pills and search (spec section 3).

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::id::GameId;
use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Steam,
    Epic,
    Gog,
    BattleNet,
    /// Microsoft Store and Xbox app (PC Game Pass) games.
    MsStore,
    /// Standalone executable, whatever its file type.
    Exe,
    Rom,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Steam => "steam",
            Source::Epic => "epic",
            Source::Gog => "gog",
            Source::BattleNet => "battlenet",
            Source::MsStore => "msstore",
            Source::Exe => "exe",
            Source::Rom => "rom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "steam" => Source::Steam,
            "epic" => Source::Epic,
            "gog" => Source::Gog,
            "battlenet" => Source::BattleNet,
            "msstore" => Source::MsStore,
            "exe" => Source::Exe,
            "rom" => Source::Rom,
            _ => return None,
        })
    }

    /// Store games (Steam, Epic, GOG, Battle.net, Microsoft Store) are
    /// started by their store and never killed by Moonglass (spec 5.4);
    /// executables and ROMs are Moonglass's own children.
    pub fn is_launcher(self) -> bool {
        !matches!(self, Source::Exe | Source::Rom)
    }
}

/// One row of the `games` table.
#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    pub id: GameId,
    pub title: String,
    pub sort_title: String,
    pub search_title: String,
    pub source: Source,
    pub source_id: Option<String>,
    pub launch_target: Option<String>,
    pub launch_args: Option<String>,
    pub working_dir: Option<String>,
    pub rom_directory_id: Option<String>,
    pub platform: Option<String>,
    pub install_dir: Option<String>,
    pub watch_process: Option<String>,
    pub track_by_folder: bool,
    pub cover_path: Option<String>,
    pub hero_path: Option<String>,
    pub logo_path: Option<String>,
    pub dominant_color: Option<String>,
    pub accent_color: Option<String>,
    pub artwork_locked: bool,
    pub date_added: i64,
    pub last_played: Option<i64>,
    /// Sessions started from Moonglass.
    pub playtime_seconds: i64,
    /// What the store reports (Steam), including play elsewhere.
    pub store_playtime_seconds: i64,
    pub is_favorite: bool,
    pub is_hidden: bool,
    pub is_installed: bool,
}

impl Game {
    /// Shown in the carousel and search results.
    pub fn is_visible(&self) -> bool {
        self.is_installed && !self.is_hidden
    }

    /// Total time played, as shown. The store's count already includes the
    /// sessions Moonglass recorded, so the two are not added; whichever is
    /// larger is the better-informed one.
    pub fn total_playtime(&self) -> i64 {
        self.playtime_seconds.max(self.store_playtime_seconds)
    }
}

/// Default carousel order: `last_played` descending (never-played last),
/// then `date_added` descending, then `sort_title`.
pub fn default_order(a: &Game, b: &Game) -> Ordering {
    // `None < Some`, so reversing puts recent first and never-played last.
    b.last_played
        .cmp(&a.last_played)
        .then_with(|| b.date_added.cmp(&a.date_added))
        .then_with(|| a.sort_title.cmp(&b.sort_title))
        .then_with(|| a.id.cmp(&b.id))
}

/// Allowed values for the `Recent` window (Settings → Library).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RecentWindow {
    Days7,
    #[default]
    Days30,
    Days90,
}

impl RecentWindow {
    pub fn seconds(self) -> i64 {
        let days = match self {
            RecentWindow::Days7 => 7,
            RecentWindow::Days30 => 30,
            RecentWindow::Days90 => 90,
        };
        days * 24 * 60 * 60
    }
}

/// Top-bar filter pills, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    All,
    Recent,
    Favorites,
    Steam,
    Epic,
    Gog,
    BattleNet,
    MsStore,
    Apps,
    Emulators,
}

impl Filter {
    pub const ORDER: [Filter; 10] = [
        Filter::All,
        Filter::Recent,
        Filter::Favorites,
        Filter::Steam,
        Filter::Epic,
        Filter::Gog,
        Filter::BattleNet,
        Filter::MsStore,
        Filter::Apps,
        Filter::Emulators,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Recent => "Recent",
            Filter::Favorites => "Favorites",
            Filter::Steam => "Steam",
            Filter::Epic => "Epic",
            Filter::Gog => "GOG",
            Filter::BattleNet => "Battle.net",
            Filter::MsStore => "MS Store",
            Filter::Apps => "Apps",
            Filter::Emulators => "Emulators",
        }
    }

    /// Whether a visible game belongs under this pill.
    pub fn accepts(self, game: &Game, now: i64, recent: RecentWindow) -> bool {
        match self {
            Filter::All => true,
            Filter::Recent => game
                .last_played
                .is_some_and(|t| now - t <= recent.seconds()),
            Filter::Favorites => game.is_favorite,
            Filter::Steam => game.source == Source::Steam,
            Filter::Epic => game.source == Source::Epic,
            Filter::Gog => game.source == Source::Gog,
            Filter::BattleNet => game.source == Source::BattleNet,
            Filter::MsStore => game.source == Source::MsStore,
            Filter::Apps => game.source == Source::Exe,
            Filter::Emulators => game.source == Source::Rom,
        }
    }
}

/// Pills that should be shown. `All` is always present; any other pill that
/// would show an empty carousel is hidden (spec: "Pills for sources with no
/// games are hidden", and `Recent` is hidden when nothing falls inside the
/// window).
pub fn visible_filters(games: &[Game], now: i64, recent: RecentWindow) -> Vec<Filter> {
    Filter::ORDER
        .into_iter()
        .filter(|f| {
            *f == Filter::All
                || games
                    .iter()
                    .any(|g| g.is_visible() && f.accepts(g, now, recent))
        })
        .collect()
}

/// Indices into `games` for the carousel under `filter`, in default order.
pub fn carousel(games: &[Game], filter: Filter, now: i64, recent: RecentWindow) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..games.len())
        .filter(|&i| games[i].is_visible() && filter.accepts(&games[i], now, recent))
        .collect();
    idx.sort_by(|&a, &b| default_order(&games[a], &games[b]));
    idx
}

/// Search results: every visible game regardless of the active pill, in
/// default order (spec 3, Search).
pub fn search(games: &[Game], query: &str) -> Vec<usize> {
    let key = text::search_key(query);
    let mut idx: Vec<usize> = (0..games.len())
        .filter(|&i| games[i].is_visible() && text::matches(&games[i].search_title, &key))
        .collect();
    idx.sort_by(|&a, &b| default_order(&games[a], &games[b]));
    idx
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn game(id: &str, title: &str, source: Source) -> Game {
        Game {
            id: GameId::from_stored(id.to_string()),
            title: title.to_string(),
            sort_title: text::sort_title(title),
            search_title: text::search_title(title),
            source,
            source_id: None,
            launch_target: None,
            launch_args: None,
            working_dir: None,
            rom_directory_id: None,
            platform: None,
            install_dir: None,
            watch_process: None,
            track_by_folder: false,
            cover_path: None,
            hero_path: None,
            logo_path: None,
            dominant_color: None,
            accent_color: None,
            artwork_locked: false,
            date_added: 0,
            last_played: None,
            playtime_seconds: 0,
            store_playtime_seconds: 0,
            is_favorite: false,
            is_hidden: false,
            is_installed: true,
        }
    }

    const DAY: i64 = 86_400;

    fn sample() -> Vec<Game> {
        let mut a = game("steam:1", "The Alpha", Source::Steam);
        a.last_played = Some(100 * DAY);
        let mut b = game("steam:2", "Beta", Source::Steam);
        b.last_played = Some(50 * DAY);
        b.is_favorite = true;
        let mut c = game("exe:3", "Gamma", Source::Exe);
        c.date_added = 10;
        let mut d = game("exe:4", "Delta", Source::Exe);
        d.date_added = 10;
        let mut hidden = game("rom:5", "Hidden", Source::Rom);
        hidden.is_hidden = true;
        let mut gone = game("epic:6", "Uninstalled", Source::Epic);
        gone.is_installed = false;
        vec![c, hidden, b, d, a, gone]
    }

    fn titles(games: &[Game], idx: &[usize]) -> Vec<String> {
        idx.iter().map(|&i| games[i].title.clone()).collect()
    }

    #[test]
    fn default_ordering() {
        let g = sample();
        let order = carousel(&g, Filter::All, 101 * DAY, RecentWindow::Days30);
        assert_eq!(titles(&g, &order), ["The Alpha", "Beta", "Delta", "Gamma"]);
    }

    #[test]
    fn recent_window() {
        let g = sample();
        let now = 101 * DAY;
        assert_eq!(
            titles(&g, &carousel(&g, Filter::Recent, now, RecentWindow::Days30)),
            ["The Alpha"]
        );
        assert_eq!(
            titles(&g, &carousel(&g, Filter::Recent, now, RecentWindow::Days90)),
            ["The Alpha", "Beta"]
        );
    }

    #[test]
    fn pills_hide_when_empty() {
        let g = sample();
        let pills = visible_filters(&g, 101 * DAY, RecentWindow::Days30);
        assert_eq!(
            pills,
            [
                Filter::All,
                Filter::Recent,
                Filter::Favorites,
                Filter::Steam,
                Filter::Apps
            ]
        );
        let pills = visible_filters(&g, 500 * DAY, RecentWindow::Days7);
        assert!(!pills.contains(&Filter::Recent));
        assert_eq!(visible_filters(&[], 0, RecentWindow::Days30), [Filter::All]);
    }

    #[test]
    fn search_ignores_pill_and_hidden() {
        let g = sample();
        assert_eq!(
            titles(&g, &search(&g, "a")),
            ["The Alpha", "Beta", "Delta", "Gamma"]
        );
        assert!(search(&g, "hidden").is_empty());
        assert!(search(&g, "uninstalled").is_empty());
        assert_eq!(titles(&g, &search(&g, "ALPHA")), ["The Alpha"]);
    }
}
