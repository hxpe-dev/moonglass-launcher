//! The Steam accounts remembered on this computer, and which installed
//! games each can play.
//!
//! Steam has one account signed in at a time; the others are remembered
//! in `config/loginusers.vdf` for its account switcher. What an account
//! can play is not written down in one place, so it is pieced together
//! from Steam's local files:
//!
//! * an installed game's `LastOwner` (whose license the install uses);
//! * the account's app tickets (`userdata/<id>/config/localconfig.vdf`,
//!   `apptickets`): Steam keeps one for every app the account has a
//!   license for and has run or installed, free-to-play games included;
//! * the account's Steam Families group (`FamilyGroup` in the same file):
//!   family members share their libraries.
//!
//! An account that never signed in on this computer has no `userdata`;
//! only the games whose install uses its own license count for it.

use std::collections::HashSet;
use std::path::Path;

use keyvalues_parser::Obj;

use crate::read_to_string;
use crate::steam::{get_str, parse_vdf};

/// SteamID64 of account ID 0, for individual accounts in the public
/// universe: SteamID64 = this + the 32-bit account ID.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

/// The 32-bit account ID Steam uses for `userdata` folders.
pub fn account_id(steam_id64: &str) -> Option<u32> {
    let id: u64 = steam_id64.parse().ok()?;
    u32::try_from(id.checked_sub(STEAM_ID64_BASE)?).ok()
}

pub fn steam_id64(account_id: u32) -> String {
    (STEAM_ID64_BASE + account_id as u64).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamAccount {
    pub steam_id: String,
    /// The login name: what Steam signs in with. Never shown.
    pub account_name: String,
    /// The public display name.
    pub persona_name: String,
    /// Steam keeps a sign-in token for it, so it can be switched to
    /// without typing a password.
    pub remembers_password: bool,
    /// Unix time of the last sign-in on this computer.
    pub last_login: i64,
}

/// The accounts in `config/loginusers.vdf`, most recently used first.
pub fn accounts(steam_root: &Path) -> Vec<SteamAccount> {
    let path = steam_root.join("config").join("loginusers.vdf");
    let Ok(text) = read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(users) = parse_vdf(&path, &text) else {
        return Vec::new();
    };
    let mut out: Vec<SteamAccount> = users
        .iter()
        .filter_map(|(id, values)| {
            let o = values.first()?.get_obj()?;
            account_id(id)?;
            Some(SteamAccount {
                steam_id: id.to_string(),
                account_name: get_str(o, "AccountName")?.to_string(),
                persona_name: get_str(o, "PersonaName")
                    .filter(|n| !n.is_empty())
                    .or_else(|| get_str(o, "AccountName"))?
                    .to_string(),
                remembers_password: get_str(o, "RememberPassword") == Some("1"),
                last_login: get_str(o, "timestamp")
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(0),
            })
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.last_login));
    out
}

/// What one account can play, from its local files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    steam_id: String,
    /// AppIDs with an app ticket.
    tickets: HashSet<String>,
    /// SteamID64s of the account's Steam Families members (itself too).
    family: HashSet<String>,
}

impl Access {
    /// Only the account's own licenses: no tickets or family known.
    pub fn own_only(steam_id: &str) -> Self {
        Access {
            steam_id: steam_id.to_string(),
            ..Default::default()
        }
    }

    /// Reads the account's `localconfig.vdf`. A missing or unreadable file
    /// leaves only the account's own licenses.
    pub fn load(steam_root: &Path, steam_id: &str) -> Self {
        let mut access = Access::own_only(steam_id);
        let Some(id) = account_id(steam_id) else {
            return access;
        };
        let path = steam_root
            .join("userdata")
            .join(id.to_string())
            .join("config")
            .join("localconfig.vdf");
        let Ok(text) = read_to_string(&path) else {
            return access;
        };
        match parse_vdf(&path, &text) {
            Ok(store) => access.read(&store),
            Err(e) => tracing::warn!(error = %e, "Steam account settings unreadable"),
        }
        access
    }

    fn read(&mut self, store: &Obj<'_>) {
        if let Some(tickets) = store.get("apptickets").and_then(|v| v.first()?.get_obj()) {
            self.tickets = tickets.keys().map(|k| k.to_string()).collect();
        }
        let members = store
            .get("FamilyGroup")
            .and_then(|v| v.first()?.get_obj())
            .and_then(|g| g.get("members")?.first()?.get_obj());
        if let Some(members) = members {
            self.family = members
                .values()
                .filter_map(|v| v.first()?.get_obj())
                .filter_map(|m| get_str(m, "accountid")?.parse::<u32>().ok())
                .map(steam_id64)
                .collect();
        }
    }

    /// Whether the account can play an installed app whose install uses
    /// `owner`'s license (`None` when the manifest does not say).
    pub fn can_play(&self, app_id: &str, owner: Option<&str>) -> bool {
        self.tickets.contains(app_id)
            || owner.is_some_and(|o| o == self.steam_id || self.family.contains(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn ids() {
        assert_eq!(account_id("76561197960265729"), Some(1));
        assert_eq!(steam_id64(1), "76561197960265729");
        assert_eq!(account_id("12"), None);
        assert_eq!(account_id("nope"), None);
    }

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn accounts_most_recent_first() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("config/loginusers.vdf"),
            r#""users"
{
    "76561197960265729" { "AccountName" "old" "PersonaName" "Old" "RememberPassword" "1" "timestamp" "100" }
    "76561197960265730" { "AccountName" "new" "PersonaName" "" "RememberPassword" "0" "timestamp" "200" }
}"#,
        );
        let a = accounts(tmp.path());
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].account_name, "new");
        // No display name: the login name stands in.
        assert_eq!(a[0].persona_name, "new");
        assert!(!a[0].remembers_password);
        assert_eq!(a[1].persona_name, "Old");
    }

    #[test]
    fn access_from_tickets_family_and_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("userdata/1/config/localconfig.vdf"),
            r#""UserLocalConfigStore"
{
    "apptickets" { "620" "AAAA" "730" "BBBB" }
    "FamilyGroup"
    {
        "groupid" "9"
        "members"
        {
            "0" { "accountid" "1" "role" "1" }
            "1" { "accountid" "5" "role" "2" }
        }
    }
}"#,
        );
        let me = steam_id64(1);
        let access = Access::load(tmp.path(), &me);
        assert!(access.can_play("620", None));
        // Owned by a family member.
        assert!(access.can_play("400", Some(&steam_id64(5))));
        // Its own copy.
        assert!(access.can_play("70", Some(&me)));
        // Someone else's game, no ticket.
        assert!(!access.can_play("400", Some(&steam_id64(7))));

        // An account that never signed in here: only its own copies.
        let other = steam_id64(2);
        let none = Access::load(tmp.path(), &other);
        assert!(none.can_play("70", Some(&other)));
        assert!(!none.can_play("620", None));
    }
}
