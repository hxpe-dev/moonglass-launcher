//! Steam friends who are in a game right now, from Steam's Web API with the
//! player's own key (Settings → Library).
//!
//! Two questions are answered for a Steam game in the library: which
//! friends are playing it (and can they be joined), and, for a game borrowed
//! through family sharing, whether its owner is playing it, in which case
//! their copy is taken.
//!
//! The key is sent to `api.steampowered.com` only and never logged: request
//! errors are stripped of their URL, which contains it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moonglass_core::{Game, Source};
use moonglass_db::Db;
use serde::Deserialize;

pub const SETTING_STEAM_API_KEY: &str = "steam_web_api_key";

const API: &str = "https://api.steampowered.com/ISteamUser";
const POLL: Duration = Duration::from_secs(60);
/// `GetPlayerSummaries` takes at most this many IDs per request.
const SUMMARY_BATCH: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    pub steam_id: String,
    pub name: String,
    /// Set when the friend is in a session that can be joined.
    pub lobby: Option<String>,
}

/// What is known at one moment. Empty when the feature is off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The Steam account signed in on this computer.
    me: String,
    friends: usize,
    /// Friends in a game, by Steam AppID.
    playing: HashMap<String, Vec<Friend>>,
    /// The account that owns each installed game, by AppID.
    owners: HashMap<String, String>,
}

impl Snapshot {
    fn app_id(game: &Game) -> Option<&str> {
        (game.source == Source::Steam)
            .then_some(game.source_id.as_deref())
            .flatten()
    }

    /// Friends playing `game` right now.
    pub fn playing(&self, game: &Game) -> &[Friend] {
        Self::app_id(game)
            .and_then(|id| self.playing.get(id))
            .map_or(&[], Vec::as_slice)
    }

    /// For a borrowed game: its owner, if they are playing it. Their copy
    /// is then in use, though the family may have another.
    pub fn in_use_by(&self, game: &Game) -> Option<&Friend> {
        let owner = self.owners.get(Self::app_id(game)?)?;
        if *owner == self.me {
            return None;
        }
        self.playing(game).iter().find(|f| f.steam_id == *owner)
    }

    fn in_game(&self) -> usize {
        self.playing.values().map(Vec::len).sum()
    }
}

/// Shown in Settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Status {
    /// No key entered.
    #[default]
    Off,
    Ok {
        friends: usize,
        in_game: usize,
    },
    Error(String),
}

impl Status {
    pub fn text(&self) -> String {
        match self {
            Status::Off => "Enter a key from steamcommunity.com/dev/apikey".into(),
            Status::Ok { friends, in_game } => format!("{friends} friends, {in_game} in a game"),
            Status::Error(e) => e.clone(),
        }
    }
}

/// "Alex is playing", "Alex and Sam are playing", "Alex, Sam and 2 more are
/// playing".
pub fn playing_line(friends: &[Friend]) -> Option<String> {
    let name = |i: usize| friends[i].name.as_str();
    Some(match friends.len() {
        0 => return None,
        1 => format!("{} is playing", name(0)),
        2 => format!("{} and {} are playing", name(0), name(1)),
        n => format!("{}, {} and {} more are playing", name(0), name(1), n - 2),
    })
}

#[derive(Deserialize)]
struct FriendListReply {
    friendslist: FriendList,
}

#[derive(Deserialize)]
struct FriendList {
    #[serde(default)]
    friends: Vec<FriendId>,
}

#[derive(Deserialize)]
struct FriendId {
    steamid: String,
}

#[derive(Deserialize)]
struct SummariesReply {
    response: Summaries,
}

#[derive(Deserialize)]
struct Summaries {
    #[serde(default)]
    players: Vec<Player>,
}

#[derive(Deserialize)]
struct Player {
    steamid: String,
    #[serde(default)]
    personaname: String,
    /// Present while the player is in a game.
    #[serde(default)]
    gameid: Option<String>,
    #[serde(default)]
    lobbysteamid: Option<String>,
}

/// Groups the players who are in a game by that game.
fn group_by_game(players: Vec<Player>) -> HashMap<String, Vec<Friend>> {
    let mut out: HashMap<String, Vec<Friend>> = HashMap::new();
    for p in players {
        let Some(game) = p.gameid else { continue };
        out.entry(game).or_default().push(Friend {
            steam_id: p.steamid,
            name: p.personaname,
            lobby: p.lobbysteamid.filter(|l| !l.is_empty() && l != "0"),
        });
    }
    for friends in out.values_mut() {
        friends.sort_by_key(|f| f.name.to_lowercase());
    }
    out
}

/// A request error without its URL (the URL holds the key).
fn describe(e: reqwest::Error) -> String {
    match e.status().map(|s| s.as_u16()) {
        Some(401 | 403) => "Steam rejected the key, or your friends list is private".into(),
        Some(429) => "Steam is limiting requests; trying again shortly".into(),
        _ => format!("Steam friends: {}", e.without_url()),
    }
}

async fn get<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    method: &str,
    query: &[(&str, &str)],
) -> Result<T, String> {
    http.get(format!("{API}/{method}/"))
        .query(query)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(describe)?
        .json()
        .await
        .map_err(describe)
}

async fn fetch(
    http: &reqwest::Client,
    key: &str,
    me: String,
    owners: HashMap<String, String>,
) -> Result<Snapshot, String> {
    let list: FriendListReply = get(
        http,
        "GetFriendList/v1",
        &[("key", key), ("steamid", &me), ("relationship", "friend")],
    )
    .await?;
    let ids: Vec<String> = list
        .friendslist
        .friends
        .into_iter()
        .map(|f| f.steamid)
        .collect();
    let mut players = Vec::new();
    for batch in ids.chunks(SUMMARY_BATCH) {
        let reply: SummariesReply = get(
            http,
            "GetPlayerSummaries/v2",
            &[("key", key), ("steamids", &batch.join(","))],
        )
        .await?;
        players.extend(reply.response.players);
    }
    Ok(Snapshot {
        me,
        friends: ids.len(),
        playing: group_by_game(players),
        owners,
    })
}

/// One refresh: the current snapshot and what Settings should say.
async fn refresh(http: &reqwest::Client, db: &Db) -> (Snapshot, Status) {
    let key = match db.setting::<String>(SETTING_STEAM_API_KEY).await {
        Ok(Some(k)) if !k.trim().is_empty() => k.trim().to_string(),
        _ => return (Snapshot::default(), Status::Off),
    };
    let root: Option<PathBuf> = crate::library::steam_root(db).await.ok().flatten();
    let local = tokio::task::spawn_blocking(move || {
        let root = root?;
        let me = moonglass_scan::steam::current_user(&root)?;
        Some((me, moonglass_scan::steam::app_owners(&root)))
    })
    .await
    .ok()
    .flatten();
    let Some((me, owners)) = local else {
        return (
            Snapshot::default(),
            Status::Error("No Steam account is signed in on this computer".into()),
        );
    };
    match fetch(http, &key, me, owners).await {
        Ok(s) => {
            let status = Status::Ok {
                friends: s.friends,
                in_game: s.in_game(),
            };
            (s, status)
        }
        Err(e) => {
            tracing::warn!("{e}");
            (Snapshot::default(), Status::Error(e))
        }
    }
}

/// Refreshes once a minute, and at once when `wake` is notified (the key
/// changed). Nothing is asked of Steam while `paused` (a game is running).
/// Ends when `send` returns false.
pub fn spawn_poll(
    rt: &tokio::runtime::Handle,
    db: Db,
    paused: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    send: impl Fn(Snapshot, Status) -> bool + Send + 'static,
) {
    rt.spawn(async move {
        let http = match reqwest::Client::builder()
            .user_agent(concat!("Moonglass/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "Steam friends unavailable");
                return;
            }
        };
        loop {
            if !paused.load(Ordering::Relaxed) {
                let (snapshot, status) = refresh(&http, &db).await;
                if !send(snapshot, status) {
                    return;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = wake.notified() => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use moonglass_core::GameId;

    fn game(app_id: &str) -> Game {
        Game {
            id: GameId::steam(app_id),
            title: "G".into(),
            sort_title: "g".into(),
            search_title: "g".into(),
            source: Source::Steam,
            source_id: Some(app_id.into()),
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
            is_favorite: false,
            is_hidden: false,
            is_installed: true,
        }
    }

    fn snapshot() -> Snapshot {
        let reply: SummariesReply = serde_json::from_str(
            r#"{"response": {"players": [
                {"steamid": "10", "personaname": "Sam", "gameid": "620", "lobbysteamid": "999"},
                {"steamid": "11", "personaname": "alex", "gameid": "620"},
                {"steamid": "12", "personaname": "Idle", "personastate": 1},
                {"steamid": "13", "personaname": "Kim", "gameid": "400", "lobbysteamid": "0"}
            ]}}"#,
        )
        .unwrap();
        Snapshot {
            me: "1".into(),
            friends: 4,
            playing: group_by_game(reply.response.players),
            owners: HashMap::from([
                ("620".to_string(), "1".to_string()),
                ("400".to_string(), "13".to_string()),
                ("70".to_string(), "13".to_string()),
            ]),
        }
    }

    #[test]
    fn friends_are_grouped_by_game() {
        let s = snapshot();
        let names: Vec<_> = s.playing(&game("620")).iter().map(|f| &f.name).collect();
        assert_eq!(names, ["alex", "Sam"], "sorted by name");
        assert_eq!(s.playing(&game("620"))[1].lobby.as_deref(), Some("999"));
        assert_eq!(s.playing(&game("400"))[0].lobby, None, "0 is no lobby");
        assert!(s.playing(&game("1")).is_empty());
        assert_eq!(s.in_game(), 3);
    }

    #[test]
    fn a_borrowed_game_is_in_use_only_while_its_owner_plays_it() {
        let s = snapshot();
        // Mine: friends playing it do not take my copy.
        assert_eq!(s.in_use_by(&game("620")), None);
        // Kim's, and Kim is playing it.
        assert_eq!(
            s.in_use_by(&game("400")).map(|f| f.name.as_str()),
            Some("Kim")
        );
        // Kim's, but Kim is playing something else.
        assert_eq!(s.in_use_by(&game("70")), None);
    }

    #[test]
    fn only_steam_games_have_friends() {
        let mut g = game("620");
        g.source = Source::Epic;
        assert!(snapshot().playing(&g).is_empty());
    }

    #[test]
    fn lines() {
        let s = snapshot();
        assert_eq!(
            playing_line(s.playing(&game("620"))).as_deref(),
            Some("alex and Sam are playing")
        );
        assert_eq!(
            playing_line(s.playing(&game("400"))).as_deref(),
            Some("Kim is playing")
        );
        assert_eq!(playing_line(&[]), None);
    }
}
