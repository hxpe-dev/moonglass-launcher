# Moonglass

<img src="assets/logo.svg" alt="" width="96">

A console-style, controller-first PC game launcher for Windows and Linux.
The repository and workspace are named `moonglass-launcher`; the app and its
executable are `Moonglass` / `moonglass`.
The full design is in [SPEC.md](SPEC.md).

## Status

All three phases are implemented. Everything runs on Windows; the Linux code
compiles but has not been run yet. Running `moonglass` with no arguments
opens the carousel (or first-time setup on an empty library).

| Area | State |
| --- | --- |
| Database, logs, single instance, scanners, file watchers | Done |
| Launch, exit detection, focus return, Now Playing with Stop / Force stop | Done on Windows |
| Notification when Moonglass cannot come back to the front | Done; Linux untested |
| Restore hotkey (controller hold, `Ctrl+Alt+Home`), with its buttons and hold time in Settings | Done; the controller hold is untested |
| Artwork: Steam's cache, Epic's catalog (Windows), SteamGridDB, Switch and libretro art for emulated games, placeholder | Done |
| Carousel, ambient lighting (including the motion blur background), card art fitting, startup animation, interface animations | Done |
| Gamepad, keyboard and mouse, with hints that follow the device; hold Esc / B to exit | Done |
| Search and on-screen keyboard | Done |
| Settings: Appearance, Library, Emulators, Artwork, System | Done |
| First-time setup | Done |
| File browser, Add a game (also by dropping a program on the window), per-game Options | Done; dropping is untested |
| Emulator profiles, presets (Switch included, Flatpaks found on Linux) and ROM folders | Done; Flatpak detection untested |
| Launch options for Steam games | Done; not yet tried with a game |
| Steam friends in your games, Join, borrowed-game warning (5.6) | Done; not yet tried with a key |
| Steam account switcher, and the library limited to what the account can play (5.7) | Done on Windows; Linux untested |
| Steam folder and "not a game" Steam apps in Settings | Done |
| Releasing images that are no longer drawn | Done |
| Packaging: Windows `.exe` and zip, Linux tarball and AppImage, release workflow | Done; the Linux packaging and the workflow have not run yet |

### Performance (spec 7)

Measured on Windows 11 with the release build and a library of 36 games:

| Measure | Target | Measured |
| --- | --- | --- |
| Cold start to the first carousel frame | Under 2 s | About 0.62 s |
| Memory, idle on the carousel | | 83 MB |
| Memory after scrolling the whole library | | 169 MB, back to 100 MB once unused images are released |
| Memory while a game runs | Under 150 MB | 80-88 MB |
| CPU while a game runs | Under 0.5% of one core | 1.4-2.4%, all of it GPUI's vsync thread (spec 9) |
| Frames rendered while a game runs | Zero | None drawn (the window is minimized) |
| Back in front after the game exits | Under 2 s after the 5 s grace period | 0.2-1.1 s after it |

Scrolling smoothness and search latency have not been measured with a
frame-time tool. Linux has not been measured.

Not done yet:

* Checks that need hardware: Linux X11 and Wayland, focus on GNOME and KDE,
  Proton detection, and Steam's registry running-state signal (use
  `moonglass steam-signal <game>` while playing).
* Linux: the keyboard restore hotkey, the X11 launcher-window close request,
  and Epic artwork without a SteamGridDB key.
* Extra helper processes to ignore can be stored but not edited in Settings.
* A ROM folder added in Settings is scanned at once but only watched for
  changes from the next start.
* Epic's namespaced-URI fallback is manual (`moonglass launch --epic-fallback`).
* The CPU target while a game runs needs a change in GPUI (spec 9).

## Releases

```bash
cargo build --release -p moonglass-launcher
powershell -File scripts\package-windows.ps1   # Windows: dist\*.exe and *.zip
scripts/package-linux.sh                         # Linux: dist/*.tar.gz and *.AppImage
```

Pushing a tag such as `v0.2.0` builds both on GitHub and attaches them to a
release.

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
moonglass steam-accounts             # remembered Steam accounts, what the signed-in one can play
moonglass steam-switch 2             # sign Steam in with account 2 (restarts Steam)
```

Set `MOONGLASS_LOG=debug` for verbose logs; `moonglass paths` shows where they are.
