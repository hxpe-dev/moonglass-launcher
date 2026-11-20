use super::*;

fn steam(app: &str, title: &str, last: Option<i64>) -> ScannedGame {
    ScannedGame {
        id: GameId::steam(app),
        title: title.into(),
        source: Source::Steam,
        source_id: Some(app.into()),
        launch_target: None,
        install_dir: Some(format!("/lib/steamapps/common/{title}")),
        rom_directory_id: None,
        platform: None,
        last_played_hint: last,
    }
}

fn rom(dir: &str, path: &str) -> ScannedGame {
    ScannedGame {
        id: GameId::rom(path),
        title: path.rsplit('/').next().unwrap().into(),
        source: Source::Rom,
        source_id: None,
        launch_target: Some(path.into()),
        install_dir: None,
        rom_directory_id: Some(dir.into()),
        platform: Some("GameCube".into()),
        last_played_hint: None,
    }
}

async fn db() -> Db {
    Db::open_in_memory().await.unwrap()
}

#[tokio::test]
async fn reconcile_updates_in_place_and_keeps_user_data() {
    let db = db().await;
    let r = db
        .reconcile(
            &ScanScope::Steam,
            &[
                steam("620", "Portal 2", Some(1000)),
                steam("400", "Portal", None),
            ],
            50,
        )
        .await
        .unwrap();
    assert_eq!(
        r,
        ReconcileReport {
            added: 2,
            updated: 0,
            uninstalled: 0
        }
    );

    let portal2 = GameId::steam("620");
    db.set_favorite(&portal2, true).await.unwrap();
    db.record_session(&portal2, 2000, 300).await.unwrap();

    // Portal uninstalled, Portal 2 renamed, Steam's LastPlayed changed.
    let r = db
        .reconcile(
            &ScanScope::Steam,
            &[steam("620", "Portal 2 (Renamed)", Some(1500))],
            60,
        )
        .await
        .unwrap();
    assert_eq!(
        r,
        ReconcileReport {
            added: 0,
            updated: 1,
            uninstalled: 1
        }
    );

    let g = db.game(&portal2).await.unwrap().unwrap();
    assert_eq!(g.title, "Portal 2 (Renamed)");
    assert_eq!(g.search_title, "portal 2 renamed");
    assert!(g.is_favorite);
    assert_eq!(g.playtime_seconds, 300);
    assert_eq!(
        g.last_played,
        Some(2000),
        "Moonglass's own value wins over Steam's"
    );
    assert_eq!(g.date_added, 50, "date_added is kept");

    let portal = db.game(&GameId::steam("400")).await.unwrap().unwrap();
    assert!(
        !portal.is_installed,
        "missing games are hidden, not deleted"
    );

    // Reinstall brings it back with nothing lost.
    let r = db
        .reconcile(
            &ScanScope::Steam,
            &[steam("620", "Portal 2", None), steam("400", "Portal", None)],
            70,
        )
        .await
        .unwrap();
    assert_eq!(
        r,
        ReconcileReport {
            added: 0,
            updated: 2,
            uninstalled: 0
        }
    );
    assert!(
        db.game(&GameId::steam("400"))
            .await
            .unwrap()
            .unwrap()
            .is_installed
    );
}

#[tokio::test]
async fn steam_last_played_seeds_only_when_empty() {
    let db = db().await;
    db.reconcile(&ScanScope::Steam, &[steam("1", "A", None)], 0)
        .await
        .unwrap();
    db.reconcile(&ScanScope::Steam, &[steam("1", "A", Some(777))], 0)
        .await
        .unwrap();
    assert_eq!(
        db.game(&GameId::steam("1"))
            .await
            .unwrap()
            .unwrap()
            .last_played,
        Some(777)
    );
}

#[tokio::test]
async fn scopes_are_independent() {
    let db = db().await;
    let dolphin = Emulator {
        id: "dolphin".into(),
        name: "Dolphin".into(),
        executable_path: "/usr/bin/dolphin-emu".into(),
        launch_template: r#"-b -e "{rom}""#.into(),
        file_extensions: ".iso,.rvz".into(),
        folder_marker: None,
    };
    db.upsert_emulator(&dolphin).await.unwrap();
    db.add_rom_directory("d1", "/roms/gc", "dolphin", "GameCube", true)
        .await
        .unwrap();
    db.add_rom_directory("d2", "/roms/gc2", "dolphin", "GameCube", false)
        .await
        .unwrap();

    db.reconcile(&ScanScope::Steam, &[steam("1", "A", None)], 0)
        .await
        .unwrap();
    db.reconcile(
        &ScanScope::RomDirectory("d1".into()),
        &[rom("d1", "/roms/gc/a.iso")],
        0,
    )
    .await
    .unwrap();
    db.reconcile(
        &ScanScope::RomDirectory("d2".into()),
        &[rom("d2", "/roms/gc2/b.iso")],
        0,
    )
    .await
    .unwrap();

    // Emptying d1 does not touch d2 or Steam.
    let r = db
        .reconcile(&ScanScope::RomDirectory("d1".into()), &[], 0)
        .await
        .unwrap();
    assert_eq!(r.uninstalled, 1);
    let installed: Vec<_> = db
        .games()
        .await
        .unwrap()
        .into_iter()
        .filter(|g| g.is_installed)
        .map(|g| g.title)
        .collect();
    assert_eq!(installed.len(), 2);
    assert!(installed.contains(&"A".to_string()));
    assert!(installed.contains(&"b.iso".to_string()));

    let dirs = db.rom_directories().await.unwrap();
    assert_eq!(dirs.len(), 2);
    let d2 = dirs.iter().find(|d| d.id == "d2").unwrap();
    assert!(!d2.recursive);
    assert_eq!(d2.extensions, ["iso", "rvz"]);
    assert_eq!(
        db.emulator_for_rom_directory("d2").await.unwrap(),
        Some(dolphin)
    );
}

#[tokio::test]
async fn executables_and_options() {
    let db = db().await;
    let id = db
        .add_executable("c:\\games\\x\\x.exe", "C:\\Games\\X\\X.exe", "X", 5)
        .await
        .unwrap();
    assert!(id.as_str().starts_with("exe:"));
    db.set_launch_options(&id, Some("-windowed"), None, Some("XGame.exe"), true)
        .await
        .unwrap();
    let g = db.game(&id).await.unwrap().unwrap();
    assert_eq!(g.source, Source::Exe);
    assert_eq!(g.launch_args.as_deref(), Some("-windowed"));
    assert_eq!(g.watch_process.as_deref(), Some("XGame.exe"));
    assert!(g.track_by_folder);

    db.set_installed(&id, false).await.unwrap();
    assert!(!db.game(&id).await.unwrap().unwrap().is_installed);
    db.remove_game(&id).await.unwrap();
    assert!(db.game(&id).await.unwrap().is_none());

    // Steam games cannot be removed, only hidden.
    db.reconcile(&ScanScope::Steam, &[steam("1", "A", None)], 0)
        .await
        .unwrap();
    db.remove_game(&GameId::steam("1")).await.unwrap();
    db.set_hidden(&GameId::steam("1"), true).await.unwrap();
    assert!(
        db.game(&GameId::steam("1"))
            .await
            .unwrap()
            .unwrap()
            .is_hidden
    );
}

#[tokio::test]
async fn settings_round_trip() {
    let db = db().await;
    assert_eq!(db.setting::<String>("steamgriddb_key").await.unwrap(), None);
    db.set_setting("steamgriddb_key", &"abc").await.unwrap();
    db.set_setting("recent_days", &30).await.unwrap();
    db.set_setting("recent_days", &90).await.unwrap();
    assert_eq!(
        db.setting::<String>("steamgriddb_key")
            .await
            .unwrap()
            .as_deref(),
        Some("abc")
    );
    assert_eq!(db.setting::<i32>("recent_days").await.unwrap(), Some(90));
}

#[tokio::test]
async fn deleting_emulator_cascades() {
    let db = db().await;
    db.upsert_emulator(&Emulator {
        id: "e".into(),
        name: "E".into(),
        executable_path: "e".into(),
        launch_template: "{rom}".into(),
        file_extensions: ".x".into(),
        folder_marker: None,
    })
    .await
    .unwrap();
    db.add_rom_directory("d", "/r", "e", "P", true)
        .await
        .unwrap();
    db.reconcile(
        &ScanScope::RomDirectory("d".into()),
        &[rom("d", "/r/a.x")],
        0,
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM emulators")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(db.games().await.unwrap().is_empty());
}

#[tokio::test]
async fn artwork_respects_lock() {
    let db = db().await;
    db.reconcile(&ScanScope::Steam, &[steam("1", "A", None)], 0)
        .await
        .unwrap();
    let id = GameId::steam("1");
    assert!(db
        .set_artwork(&id, "c1", "h1", None, "#111111", "#222222", false)
        .await
        .unwrap());
    assert!(db
        .set_artwork(&id, "user", "h2", Some("l"), "#333333", "#444444", true)
        .await
        .unwrap());
    // Automatic refresh cannot replace the user's choice.
    assert!(!db
        .set_artwork(&id, "c3", "h3", None, "#555555", "#666666", false)
        .await
        .unwrap());
    let g = db.game(&id).await.unwrap().unwrap();
    assert_eq!(g.cover_path.as_deref(), Some("user"));
    assert!(g.artwork_locked);
    db.unlock_artwork(&id).await.unwrap();
    assert!(db
        .set_artwork(&id, "c4", "h4", None, "#777777", "#888888", false)
        .await
        .unwrap());
}

#[tokio::test]
async fn removing_an_emulator_removes_its_folders_and_games() {
    let db = db().await;
    for (id, name) in [("pcsx2", "PCSX2"), ("dolphin", "Dolphin")] {
        db.upsert_emulator(&Emulator {
            id: id.into(),
            name: name.into(),
            executable_path: format!("/emu/{id}"),
            launch_template: "\"{rom}\"".into(),
            file_extensions: ".iso".into(),
            folder_marker: None,
        })
        .await
        .unwrap();
    }
    db.add_rom_directory("romdir:gc", "/roms/gc", "dolphin", "GameCube", true)
        .await
        .unwrap();
    db.add_rom_directory("romdir:ps2", "/roms/ps2", "pcsx2", "PS2", true)
        .await
        .unwrap();
    let names: Vec<_> = db
        .emulators()
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["Dolphin", "PCSX2"], "sorted by name");

    let game = ScannedGame {
        id: GameId::rom("/roms/gc/game.iso"),
        title: "Game".into(),
        source: Source::Rom,
        source_id: None,
        launch_target: Some("/roms/gc/game.iso".into()),
        install_dir: None,
        rom_directory_id: Some("romdir:gc".into()),
        platform: Some("GameCube".into()),
        last_played_hint: None,
    };
    db.reconcile(&ScanScope::RomDirectory("romdir:gc".into()), &[game], 0)
        .await
        .unwrap();
    assert_eq!(db.games().await.unwrap().len(), 1);

    db.remove_emulator("dolphin").await.unwrap();
    let folders = db.rom_folders().await.unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].emulator_id, "pcsx2");
    assert!(db.games().await.unwrap().is_empty());

    db.remove_rom_directory("romdir:ps2").await.unwrap();
    assert!(db.rom_folders().await.unwrap().is_empty());
}

#[tokio::test]
async fn gog_games_are_their_own_scope() {
    let db = db().await;
    let gog = ScannedGame {
        id: GameId::gog("1207658924"),
        title: "The Witcher".into(),
        source: Source::Gog,
        source_id: Some("1207658924".into()),
        launch_target: Some("C:/GOG Games/The Witcher/witcher.exe".into()),
        install_dir: Some("C:/GOG Games/The Witcher".into()),
        rom_directory_id: None,
        platform: None,
        last_played_hint: None,
    };
    db.reconcile(&ScanScope::Gog, &[gog], 0).await.unwrap();
    db.reconcile(&ScanScope::Steam, &[steam("1", "A", None)], 0)
        .await
        .unwrap();
    // An empty Steam scan leaves GOG alone, and the other way round.
    db.reconcile(&ScanScope::Steam, &[], 0).await.unwrap();
    let g = db.game(&GameId::gog("1207658924")).await.unwrap().unwrap();
    assert_eq!(g.source, Source::Gog);
    assert!(g.is_installed);
    db.reconcile(&ScanScope::Gog, &[], 0).await.unwrap();
    let g = db.game(&GameId::gog("1207658924")).await.unwrap().unwrap();
    assert!(!g.is_installed);
}

#[tokio::test]
async fn battlenet_and_msstore_games_are_stored() {
    let db = db().await;
    let game = |id: GameId, source: Source, source_id: &str, dir: &str| ScannedGame {
        id,
        title: "Game".into(),
        source,
        source_id: Some(source_id.into()),
        launch_target: None,
        install_dir: Some(dir.into()),
        rom_directory_id: None,
        platform: None,
        last_played_hint: None,
    };
    let sc2 = game(
        GameId::battlenet("s2"),
        Source::BattleNet,
        "s2",
        "C:/Program Files (x86)/StarCraft II",
    );
    let fh = game(
        GameId::msstore("Microsoft.ForteBaseGame_8wekyb3d8bbwe"),
        Source::MsStore,
        "Microsoft.ForteBaseGame_8wekyb3d8bbwe!Forzahorizon6",
        "C:/XboxGames/Forza Horizon 6/Content",
    );
    db.reconcile(&ScanScope::BattleNet, &[sc2], 0)
        .await
        .unwrap();
    db.reconcile(&ScanScope::MsStore, &[fh], 0).await.unwrap();
    let g = db.game(&GameId::battlenet("s2")).await.unwrap().unwrap();
    assert_eq!(g.source, Source::BattleNet);
    // Each is its own scope: an empty Store scan leaves Battle.net alone.
    db.reconcile(&ScanScope::MsStore, &[], 0).await.unwrap();
    assert!(db.game(&g.id).await.unwrap().unwrap().is_installed);
    let fh = db
        .game(&GameId::msstore("Microsoft.ForteBaseGame_8wekyb3d8bbwe"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fh.source, Source::MsStore);
    assert!(!fh.is_installed);
}

#[tokio::test]
async fn store_playtime_is_kept_apart_and_replaced() {
    let db = db().await;
    db.reconcile(
        &ScanScope::Steam,
        &[steam("440", "TF2", None), steam("620", "Portal 2", None)],
        0,
    )
    .await
    .unwrap();
    let tf2 = GameId::steam("440");
    db.record_session(&tf2, 10, 600).await.unwrap();
    let mut map = std::collections::HashMap::new();
    map.insert("440".to_string(), 7260);
    assert_eq!(
        db.set_store_playtimes(Source::Steam, &map).await.unwrap(),
        1
    );
    let g = db.game(&tf2).await.unwrap().unwrap();
    assert_eq!(g.playtime_seconds, 600);
    assert_eq!(g.store_playtime_seconds, 7260);
    assert_eq!(g.total_playtime(), 7260);
    // Another account: its figures replace the first one's.
    map.clear();
    map.insert("620".to_string(), 60);
    db.set_store_playtimes(Source::Steam, &map).await.unwrap();
    let g = db.game(&tf2).await.unwrap().unwrap();
    assert_eq!(g.store_playtime_seconds, 0);
    assert_eq!(g.total_playtime(), 600);
}

/// A database whose migrations were recorded from CRLF files (a build from
/// a Windows checkout) opens in a build with LF files, and the other way
/// round; a migration that really changed is still refused.
#[tokio::test]
async fn migrations_recorded_with_other_line_endings() {
    use sha2::{Digest, Sha384};
    let db = Db::open_in_memory().await.unwrap();
    let record = |version: i64, checksum: Vec<u8>| {
        let pool = db.pool.clone();
        async move {
            sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                .bind(checksum)
                .bind(version)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let recorded = |version: i64| {
        let pool = db.pool.clone();
        async move {
            let c: Vec<u8> =
                sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                    .bind(version)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            c
        }
    };
    for m in MIGRATOR.iter() {
        let lf = m.sql.as_str().replace("\r\n", "\n");
        for variant in [lf.replace('\n', "\r\n"), lf] {
            record(m.version, Sha384::digest(variant.as_bytes()).to_vec()).await;
            migrate(&db.pool).await.unwrap();
            assert_eq!(recorded(m.version).await, *m.checksum);
        }
    }
    let first = MIGRATOR.iter().next().unwrap();
    record(first.version, Sha384::digest(b"something else").to_vec()).await;
    assert!(migrate(&db.pool).await.is_err());
}
