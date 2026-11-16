# Moonglass

<img src="assets/logo.svg" alt="" width="96">

A console-style, controller-first PC game launcher for Windows and Linux.
The repository and workspace are named `moonglass-launcher`; the app and its
executable are `Moonglass` / `moonglass`.
The full design is in [SPEC.md](SPEC.md).

## Status

Phases 1 and 2 are implemented and Phase 3 is nearly complete. Everything
runs on Windows; the Linux code compiles but has not been run yet. Running
`moonglass` with no arguments opens the carousel.

| Area | State |
| --- | --- |
| Database, logs, single instance, scanners, file watchers | Done |
| Launch, exit detection, focus return, Now Playing with Stop / Force stop | Done on Windows |
| Restore hotkey (controller hold, `Ctrl+Alt+Home`) | Done; the controller hold is untested |
| Artwork: Steam's cache, Epic's catalog (Windows), SteamGridDB, placeholder | Done |
| Carousel, ambient lighting, startup animation, interface animations | Done |
| Gamepad, keyboard and mouse, with hints that follow the device | Done |
| Search and on-screen keyboard | Done |
| Settings: Appearance, Library, Emulators, Artwork, System | Done |
| File browser, Add a game, per-game Options | Done |
| Emulator profiles, presets (Switch included) and ROM folders | Done; not yet tried with a real emulator |
| Launch options for Steam games | Done; not yet tried with a game |
| Steam friends in your games, Join, borrowed-game warning (5.6) | Done; not yet tried with a key |
| First-run flow, packaging, performance checks (spec 7) | Not yet |

Not done yet:

* Checks that need hardware: Linux X11 and Wayland, focus on GNOME and KDE,
  Proton detection, and Steam's registry running-state signal (use
  `moonglass steam-signal <game>` while playing).
* Linux: the keyboard restore hotkey, the X11 launcher-window close request,
  the desktop notification when focus cannot be regained, Epic artwork
  without a SteamGridDB key, and Flatpak detection in the emulator presets.
* Settings for the manual Steam folder, extra Steam apps to leave out, extra
  helper processes to ignore, and the launch timeout. The values are read
  but nothing in the interface writes them.
* A ROM folder added in Settings is scanned at once but only watched for
  changes from the next start.
* Images far from the focused card are not released: only nearby cards are
  drawn, but GPUI's image cache keeps what it decoded.
* Epic's namespaced-URI fallback is manual (`moonglass launch --epic-fallback`).
* Drag-and-drop to add a game, and the "Add your games" card opening setup.

### Development

`MOONGLASS_WINDOWED=1` opens a window instead of full screen.
`MOONGLASS_DEV_SECOND_INSTANCE=1` lets a test window run next to a normal
one (it shares the database).
`MOONGLASS_DEV_SCREEN=settings` (or `search`) opens on that screen.
The log records how long startup took (`startup: first frame ms=…`).

## Layout

```
crates/
  moonglass-core      IDs, title normalization, ordering/filters/search,
                      launch templates, launch state machine (pure, no OS)
  moonglass-scan      Steam / Epic / Heroic / ROM file parsing
  moonglass-db        sqlx + SQLite, migrations, reconciliation, settings
  moonglass-platform  everything OS-specific (windows.rs / linux.rs)
  moonglass-art       artwork sources and ingestion
  moonglass-launcher  the app: GPUI interface (src/ui) plus a debug CLI
```

Only `moonglass-platform` contains platform conditionals.

## Building

```bash
cargo build --release
cargo test --workspace
```

Queries are compile-time checked against the metadata in `.sqlx/`, so no
database is needed to build (`SQLX_OFFLINE=true` is set in
`.cargo/config.toml`). After changing a query or migration, install
`sqlx-cli` (`cargo install sqlx-cli --no-default-features --features sqlite`)
and run `scripts/sqlx-prepare.sh`.

## Using the debug build

```bash
moonglass list                       # scan, then list in carousel order
moonglass list --filter steam
moonglass list --search "half life"
moonglass launch 3                   # by list number or id, e.g. steam:620
moonglass add-exe "C:\Games\Foo\Foo.exe"
moonglass add-emulator dolphin Dolphin "C:\Emu\Dolphin.exe" "-b -e \"{rom}\"" ".iso,.rvz,.gcm"
moonglass add-rom-dir "D:\ROMs\GameCube" dolphin GameCube
moonglass options 3 --watch RealGame.exe --track-folder
moonglass autostart on
```

Set `MOONGLASS_LOG=debug` for verbose logs; `moonglass paths` shows where they are.
