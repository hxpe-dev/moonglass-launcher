//! The Steam account switcher: the accounts remembered on this computer,
//! the one Steam is signed in with, and which installed Steam games it can
//! play (see [`moonglass_scan::steam_accounts`]).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use moonglass_core::{Game, Source};
use moonglass_scan::steam_accounts::{self, Access, SteamAccount};

/// How long Steam gets to sign in after it restarts.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteamAccounts {
    /// Most recently used first.
    pub accounts: Vec<SteamAccount>,
    /// SteamID64 of the account Steam is signed in with, or, while Steam
    /// is closed, the one it will sign in with.
    pub active: Option<String>,
    access: Access,
    /// Whose license each installed app uses, by AppID.
    owners: HashMap<String, String>,
}

/// SteamID64 of the account Steam is signed in with right now, or, while
/// Steam is closed, the one it will sign in with. Blocking.
pub fn signed_in(steam_root: &Path) -> Option<String> {
    moonglass_platform::steam_active_account()
        .map(steam_accounts::steam_id64)
        .or_else(|| moonglass_scan::steam::current_user(steam_root))
}

impl SteamAccounts {
    /// Reads Steam's files. Blocking.
    pub fn load(steam_root: &Path) -> Self {
        let accounts = steam_accounts::accounts(steam_root);
        let active = signed_in(steam_root);
        let access = active
            .as_deref()
            .map(|id| Access::load(steam_root, id))
            .unwrap_or_default();
        Self {
            accounts,
            active,
            access,
            owners: moonglass_scan::steam::app_owners(steam_root),
        }
    }

    pub fn active_account(&self) -> Option<&SteamAccount> {
        let id = self.active.as_deref()?;
        self.accounts.iter().find(|a| a.steam_id == id)
    }

    /// Whether there is anything to choose between. With one account, the
    /// library is never filtered: nothing could be gained, and a game
    /// whose license Steam's files do not show would be hidden for nothing.
    pub fn can_switch(&self) -> bool {
        self.accounts.len() > 1
    }

    /// Whether the signed-in account can play `game`. Games from other
    /// stores, and every game when there is no choice of account, count as
    /// playable.
    pub fn can_play(&self, game: &Game) -> bool {
        if !self.can_switch() || self.active.is_none() || game.source != Source::Steam {
            return true;
        }
        let Some(app) = game.source_id.as_deref() else {
            return true;
        };
        self.access
            .can_play(app, self.owners.get(app).map(String::as_str))
    }
}

/// Signs Steam in as `account` and reads the result. Blocking: Steam is
/// closed and started again.
pub fn switch(steam_root: &Path, account: &SteamAccount) -> (Result<(), String>, SteamAccounts) {
    let result = match steam_accounts::account_id(&account.steam_id) {
        Some(id) => {
            moonglass_platform::switch_steam_account(&account.account_name, id, SIGN_IN_TIMEOUT)
                .map_err(|e| e.to_string())
        }
        None => Err("not a Steam account".into()),
    };
    (result, SteamAccounts::load(steam_root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use moonglass_core::GameId;

    fn game(source: Source, app: &str) -> Game {
        Game {
            id: GameId::steam(app),
            title: "G".into(),
            sort_title: "g".into(),
            search_title: "g".into(),
            source,
            source_id: Some(app.into()),
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

    fn steam_game(app: &str) -> Game {
        game(Source::Steam, app)
    }

    fn account(id: &str) -> SteamAccount {
        SteamAccount {
            steam_id: id.into(),
            account_name: "a".into(),
            persona_name: "A".into(),
            remembers_password: true,
            last_login: 0,
        }
    }

    #[test]
    fn one_account_never_filters() {
        let s = SteamAccounts {
            accounts: vec![account("76561197960265729")],
            active: Some("76561197960265729".into()),
            ..Default::default()
        };
        assert!(s.can_play(&steam_game("620")));
    }

    #[test]
    fn other_accounts_copies_are_filtered() {
        let me = "76561197960265729";
        let s = SteamAccounts {
            accounts: vec![account(me), account("76561197960265730")],
            active: Some(me.into()),
            access: Access::own_only(me),
            owners: HashMap::from([
                ("620".into(), me.into()),
                ("400".into(), "76561197960265730".into()),
            ]),
        };
        assert!(s.can_play(&steam_game("620")));
        assert!(!s.can_play(&steam_game("400")));
        // Not a Steam game: never filtered.
        assert!(s.can_play(&game(Source::Exe, "400")));
    }
}
