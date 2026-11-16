# Moonglass

A console-style, controller-first PC game launcher for Windows and Linux.
The repository and workspace are named `moonglass-launcher`; the app and its
executable are `Moonglass` / `moonglass`.
The full design is in [SPEC.md](SPEC.md).

## Status

Phases 1 and 2 are implemented, and Phase 3 is under way. Everything runs on
Windows; the Linux code compiles but has not been run yet. Running
`moonglass` with no arguments opens the carousel.

| Spec area | State |
| --- | --- |
| Core: database, migrations, logs, single instance, scanners, reconciliation | Done |
| Launch state machine, detection (Job Object / process group / folder / `SteamAppId`) | Done; launch, exit and focus return confirmed on Windows |
| File-system watchers, 2 s debounce | Done |
| Artwork (5.2): Steam local cache, SteamGridDB, placeholder, palette, blurred heroes | Done |
| Carousel: spring motion, focus styling, glow, ambient modes, hero zone, startup animation | Done |
| Gamepad (`gilrs`), keyboard and mouse, input-aware hints | Done; gamepad confirmed with a real controller |
| Controller battery (gilrs, XInput on Windows) | Done; XInput reading not yet seen on a real pad |
| Restore hotkey and Now Playing screen with Stop / Force stop | Done; the controller combination is untested |
| Search box (top right, caret), on-screen keyboard | Done |
| Settings (Appearance, Library, Artwork, System) on a blurred backdrop, saved preferences, Exit | Done |
| Animated highlights, panels, sheets, search and list changes (off with Reduced Motion) | Done |
| File browser, Add a game (executable), per-game Options menu | Done |
| Emulator profiles and presets, ROM directory setup in Settings | Not yet (CLI only: `add-emulator`, `add-rom-dir`) |
| First-run flow, packaging, section 7 performance checks | Not yet |

Measured on Windows (1616×909 window, 32 games, release build): the window
appears in 1.85 s and uses about 89 MB. While idle on the carousel it uses
about 8% of one core: the slow glow pulse is redrawn at about 30 Hz, and
scrolling and crossfades run at the display rate.

Not done yet:

* Phase 0 checks that need hardware: Linux X11/Wayland, focus on GNOME/KDE,
  Proton detection, Steam's registry running-state signal (use
  `moonglass steam-signal <game>` while playing).
* Linux keyboard restore hotkey (X11 grab / Wayland portal), X11
  launcher-window close request, desktop notification when focus fails.
* GPU textures far from the focused card are not released yet. Only nearby
  cards are drawn, but GPUI's image cache keeps decoded images.
* Epic's namespaced-URI fallback is manual (`--epic-fallback`).

### Development

`MOONGLASS_WINDOWED=1` opens a window instead of full screen.
`MOONGLASS_DEV_SECOND_INSTANCE=1` lets a test window run next to a normal
one (it shares the database).
`MOONGLASS_DEV_SCREEN=settings` (or `search`) opens on that screen.

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
