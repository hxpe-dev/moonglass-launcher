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
