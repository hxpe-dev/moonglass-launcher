<div align="center">

<img src="assets/logo.svg" alt="Moonglass logo" width="112">

# Moonglass

**A game launcher for the couch.**

One full-screen shelf for your Steam, Epic, GOG, Battle.net, Microsoft Store
and emulated games, made to be browsed with a controller.
It starts in under a second, and while you play, it does nothing at all.

[![Release](https://img.shields.io/github/v/release/hxpe-dev/moonglass-launcher?include_prereleases&label=release)](https://github.com/hxpe-dev/moonglass-launcher/releases)
[![CI](https://github.com/hxpe-dev/moonglass-launcher/actions/workflows/ci.yml/badge.svg)](https://github.com/hxpe-dev/moonglass-launcher/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
![Platforms: Windows, Linux](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-lightgrey)

[**Download**](https://github.com/hxpe-dev/moonglass-launcher/releases) ·
[User guide](site/docs.html) ·
[Benchmarks](docs/BENCH.md) ·
[Design](docs/SPEC.md)

</div>

> [!NOTE]
> Moonglass is in **beta**. It is used daily on Windows; the Linux build is
> a preview that compiles and is packaged but has not yet been tried on real
> hardware. See [what has been tested](docs/STATUS.md).

## Features

- **Every store, one shelf.** Steam, Epic, GOG, Battle.net and Microsoft
  Store games are found on their own, along with your emulators' ROM folders
  and any program you add. Turn off the stores you don't want to see.
- **Made for a controller.** Every screen works with a gamepad, setup and
  search included, with an on-screen keyboard. Keyboard and mouse work too,
  and the button hints follow what you hold.
- **The stores stay in charge.** Games start through Steam, Epic, GOG Galaxy
  or Battle.net, so achievements, cloud saves and Steam Input keep working.
- **Quiet while you play.** Nothing is drawn while a game runs. Quit and
  Moonglass is back in about a second, or call it up mid-game with a held
  button combination for the Quick Menu: clips, screenshots, the game's
  volume, stop.
- **Artwork without effort.** Covers come from your stores' own caches and
  catalogs, Switch and retro game databases, or SteamGridDB with a free key.
  Set your own for any game.
- **Steam extras.** See which friends are in your games and join them,
  switch Steam accounts, and keep Steam's own playtime.
- **Set it up your way.** Posters, banners or squares; each game's colors,
  its art behind everything, or pure black for OLED screens. Interface
  sounds and background music from your own folder.
- **No account, nothing left running.** Use the installer or the single
  portable file; nothing keeps running once you close it. Updates install
  from inside the app.

## Install

Download the latest build from the
[releases page](https://github.com/hxpe-dev/moonglass-launcher/releases).

| Platform | Download | |
| --- | --- | --- |
| Windows 10 and 11, 64-bit | `moonglass-…-windows-x86_64-setup.exe` | The installer: Start menu entry, optional desktop shortcut, uninstall from Windows settings. No administrator rights needed. |
| Windows, portable | `moonglass-…-windows-x86_64.exe` | The same program as one file: run it from anywhere. A `.zip` with it is also attached. |
| Linux, 64-bit, X11 and Wayland (preview) | `moonglass-…-linux-x86_64.AppImage` | `chmod +x` it and run it. A `.tar.gz` with a desktop entry is also attached. |

The first start scans your stores and walks you through optional extras
(emulators, a SteamGridDB key). Updates are offered in
**Settings → System → Updates**, and checked for shortly after each start.

Moonglass keeps its library and settings in `%APPDATA%\Moonglass` on Windows
and `~/.local/share/moonglass` on Linux; run `moonglass paths` to see every
folder it uses.

## Controls

| | Controller | Keyboard | Mouse |
| --- | --- | --- | --- |
| Move between games | D-pad or left stick | ← → | Wheel, or click a card |
| Play | A | Enter | Click the focused card |
| Search | X | Just start typing | Click the search box |
| Game options | Y | Ctrl + O | Right-click a card |
| Switch filter | LB / RB | Tab / Shift + Tab | Click a filter |
| Settings | Menu | Ctrl + , | Click the gear |
| Back | B | Esc | |
| Exit | Hold B | Hold Esc, or Ctrl + Q | |
| Back to Moonglass during a game | Hold both sticks in (configurable) | Ctrl + Alt + Home | |

The [user guide](site/docs.html) covers everything else: filters, per-game
options, emulators, the Quick Menu and every setting.

## Performance

Measured against other multi-store launchers on the same Windows PC
(medians of 5 runs; [method and raw data](docs/BENCH.md)):

| | Moonglass | One Game Launcher | Playnite (Desktop) |
| --- | --- | --- | --- |
| Time until loaded | **1.7 s** | 2.1 s | 3.6 s |
| Memory in use, idle | **79 MB** | 172 MB | 668 MB |
| Install size | **28 MB** | 41 MB | 433 MB |

Moonglass is written in Rust and drawn on the GPU with
[GPUI](https://www.gpui.rs/), the UI framework behind the Zed editor; there
is no web view or JavaScript runtime.

## Building from source

You need a recent stable Rust (1.94 or later). On Linux, also the system
libraries GPUI and the gamepad library build against:

```bash
sudo apt install libudev-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-xcb-dev libxcb1-dev libvulkan-dev libfontconfig-dev libfreetype-dev \
  libasound2-dev pkg-config
```

Then:

```bash
cargo build --release      # target/release/moonglass(.exe)
cargo test --workspace
```

Database queries are checked at compile time against the metadata in
`.sqlx/`, so no database is needed to build. After changing a query or a
migration, install `sqlx-cli` 0.9 and run `scripts/sqlx-prepare.sh`.

<details>
<summary><b>Development tips and the debug command line</b></summary>

Environment variables:

| Variable | Effect |
| --- | --- |
| `MOONGLASS_WINDOWED=1` | Open in a window instead of full screen |
| `MOONGLASS_DEV_SECOND_INSTANCE=1` | Let a test window run next to a normal one (they share the database) |
| `MOONGLASS_DEV_SCREEN=settings` / `search` / `quick` | Open on that screen; `quick` opens the Quick Menu over the first game without starting it |
| `MOONGLASS_DEV_PIDS=1234,5678` | Processes the Quick Menu's volume row controls in that mode |
| `MOONGLASS_LOG=debug` | Verbose logs (the log also records startup timings) |

With arguments, `moonglass` is a command line that drives the same library
and launch code without the window:

```bash
moonglass list                       # scan, then list in carousel order
moonglass list --filter steam --search "half life"
moonglass launch 3                   # by list number or id, e.g. steam:620
moonglass add-exe "C:\Games\Foo\Foo.exe"
moonglass add-emulator dolphin Dolphin "C:\Emu\Dolphin.exe" "-b -e \"{rom}\"" ".iso,.rvz,.gcm"
moonglass add-rom-dir "D:\ROMs\GameCube" dolphin GameCube
moonglass options 3 --watch RealGame.exe --track-folder
moonglass steam-accounts             # remembered Steam accounts, what the signed-in one can play
moonglass update --check             # is there a newer release?
moonglass paths                      # where the database, cache and logs are
moonglass --help                     # everything else
```

</details>

### Project layout

```
crates/
  moonglass-core      IDs, title normalization, ordering, filters and search,
                      launch templates, the launch state machine (no OS code)
  moonglass-scan      Steam, Epic, GOG, Battle.net, Microsoft Store, Heroic
                      and ROM folder scanners
  moonglass-db        SQLite through sqlx: migrations, reconciliation, settings
  moonglass-platform  everything OS-specific (windows.rs and linux.rs)
  moonglass-art       artwork sources and processing
  moonglass-launcher  the app: the GPUI interface (src/ui) and the command line
docs/                 the design (SPEC.md), feature status (STATUS.md), benchmarks (BENCH.md)
site/                 the project website and user guide
scripts/              packaging and sqlx helpers
```

Only `moonglass-platform` contains platform conditionals; everything else is
shared between Windows and Linux.

### Releasing

Change `version` in the root `Cargo.toml` (for example `0.1.0-beta.2` or
`0.2.0`) and push to `main`. The release workflow notices a version that has
no `v<version>` tag yet, runs the tests, builds both platforms and publishes
the release with the executables attached. A version with a suffix such as
`-beta.2` is published as a pre-release, which only beta builds are offered.

## License

Copyright 2026 hxpe-dev. Licensed under the [Apache License, Version 2.0](LICENSE).

Release downloads also contain `THIRD-PARTY-NOTICES.html`, the licenses of
the libraries and fonts built into Moonglass.

Moonglass is not affiliated with Valve, Epic Games, GOG, Blizzard or
Microsoft. Store names are used only to say which games Moonglass can find.
