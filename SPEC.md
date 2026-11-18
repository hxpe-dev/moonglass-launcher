# Project Specification: **Moonglass** (Console-Style PC Game Launcher)

*Spec revision 1.8.*

## 1. Vision & Core Philosophy

**Moonglass** is a lightweight, visually refined, 10-foot (TV-optimized) PC game launcher that turns a Windows or Linux gaming PC into a living-room console experience.

* **Zero Clutter:** No storefronts, news feeds, ads, or dense metadata walls on the home screen.
* **Premium Aesthetic:** Fluid GPU-rendered animation at the display's refresh rate (up to 120 Hz), ambient lighting derived from the selected game's artwork, and clean typography.
* **Native Integrity:** Launching is delegated to the official clients (Steam, Epic, GOG Galaxy) so achievements, cloud saves, Steam Input profiles, and the client's own playtime tracking keep working. Moonglass never replaces or patches a client. On Linux, where Epic and GOG have no official client, their games are launched through Heroic Games Launcher (section 2).
* **One Codebase, Two Platforms:** Windows and Linux are both first-class. Everything that differs between them sits behind one platform layer (section 4.3); the UI, database, scanners, and artwork pipeline are shared.
* **Controller-First:** Every Moonglass screen, including first-run setup, is navigable by gamepad, and equally usable with keyboard and mouse (section 3, Keyboard and Mouse). Dialogs owned by other programs (Steam EULA prompts, cloud-save conflicts, UAC) are outside Moonglass's control; see section 9.

### Non-Goals (v1.0)

* No store browsing, purchasing, downloading, installing, or updating of games.
* No launchers other than Steam, Epic and GOG (EA, Ubisoft, Xbox app, Battle.net are post-1.0).
* No replacement of the Windows shell or the Linux desktop session. Moonglass is a normal fullscreen app that can start at login.
* No support for running inside a gamescope session (SteamOS Game Mode and similar). Linux support targets regular X11 and Wayland desktop sessions.
* No Flatpak or Snap package of Moonglass itself. A sandbox would block reading other launchers' files and observing game processes.
* No macOS build. The platform layer should not prevent one, but nothing is tested there.

## 2. Scope (v1.0)

**Supported platforms:** Windows 10 and 11, and Linux desktop sessions on X11 or Wayland. x86-64 only.

v1.0 ships exactly this, on both platforms:

1. Five launch sources (below).
2. Automatic artwork with a manual override.
3. The carousel home screen with ambient lighting and built-in customization.
4. Reliable launch, exit detection, and focus recovery.
5. Controller-navigable settings, including a file browser and on-screen keyboard.
6. Library search by title.
7. Optional start at login.

Deferred to post-1.0: UI sound effects and ambient audio, animated cover art, additional launchers.

Added in revision 1.7: GOG as a fifth source, Steam's own playtime shown alongside Moonglass's (5.1), the time and controller battery on the Now Playing screen (5.3), turning stores off (section 3, Stores), and a library that checks and repairs itself (section 6, Keeping the library sound).

Added in revision 1.8: the Quick Menu on the Now Playing screen, with clips and screenshots through the player's capture tool and the game's own volume (5.8).

### Supported Game Sources

1. **Steam**
   * Steam root, Windows: read from the registry (`HKCU\Software\Valve\Steam\SteamPath`).
   * Steam root, Linux: the first that exists of `~/.local/share/Steam`, `~/.steam/steam`, the Flatpak location (`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`), and the Snap location. The user can set it manually.
   * Libraries come from `libraryfolders.vdf`; games from each library's `steamapps/appmanifest_*.acf`. The file formats are identical on both platforms.
   * A game's folder is `<library>/steamapps/common/<installdir>`. `installdir` in the manifest is a folder name, not a full path.
   * Only fully installed apps are listed. Tools and runtimes (Steamworks Common Redistributables, Proton, Steam Linux Runtime, and similar) are excluded by a built-in AppID deny-list that the user can extend.
   * Launched via `steam://rungameid/<AppID>`, handed to the OS URI handler. On Linux this covers native and Proton games alike; Steam chooses the runtime.
   * Steam has one account signed in at a time; the others it remembers are offered by the account switcher (5.7), which can also limit the library to the Steam games the signed-in account can play.
   * Optional per-game launch options. When a game has them it is started with Steam's own command line instead (`steam -applaunch <AppID> <options>`), because the URI cannot carry options without Steam asking the player to confirm. They are added to whatever launch options the game has inside Steam.
2. **Epic Games Store**
   * **On Linux:** Epic has no official client, so this source is backed by **Heroic Games Launcher**. Games are read from the installed-games list Heroic keeps for its Epic backend (Legendary), in either the native or the Flatpak config location. Launch is `heroic --no-gui "heroic://launch?appName=<AppName>&runner=legendary"` (or the Flatpak equivalent). If Heroic is not installed, the Epic source and its filter pill do not appear. The rest of this item describes Windows.
   * Discovered from `.item` manifests in `C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests`.
   * Only entries that are applications in a games category are listed. DLC, add-ons, and Unreal Engine installs are skipped.
   * Launched via `com.epicgames.launcher://apps/<AppName>?action=launch&silent=true`. If a title fails with the bare `AppName`, the fallback is the `<CatalogNamespace>:<CatalogItemId>:<AppName>` form; both IDs are in the manifest.
3. **GOG**
   * **Windows:** every GOG install, through GOG Galaxy or one of GOG's offline installers, registers itself under `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<product ID>` (`gameName`, `path`, `exe`, and `dependsOn` for add-ons). Add-ons, and keys whose folder no longer exists, are skipped.
   * **Linux:** backed by Heroic, like Epic: games are read from Heroic's `gog_store/installed.json` (native or Flatpak config location); DLC is skipped. If Heroic is not installed, the GOG source and its pill do not appear.
   * On both platforms the install folder holds `goggame-<product ID>.info`, GOG's own description of the game. Its `name` is preferred as the title, and an entry whose `rootGameId` is another product is an add-on and skipped.
   * Launched, on Windows, through GOG Galaxy: `GalaxyClient.exe /command=runGame /gameId=<id> /path=<install folder>`, so Galaxy's cloud saves and playtime keep working. Without Galaxy (an offline-installer game), the registered `exe` is started directly as Moonglass's own child. On Linux: `heroic --no-gui "heroic://launch?appName=<id>&runner=gog"`.
   * Detected while running by its install folder, like Epic.
4. **Standalone Executables**
   * Windows: `.exe` files. Linux: any file with the executable bit, including native binaries, AppImages, and shell scripts. Running a Windows `.exe` on Linux is not handled directly; the user points Moonglass at a wrapper script that calls Wine or Proton.
   * Added through Moonglass's own controller-navigable file browser, or by drag-and-drop when a mouse is present.
   * Launched directly as a child process. The working directory defaults to the executable's folder and can be overridden per game.
   * Optional launch arguments.
5. **Emulated ROMs**
   * The user registers ROM directories. Each directory is bound to one emulator profile and one platform label (for example "GameCube"). This removes ambiguity when several emulators accept the same extension, such as `.iso`.
   * Emulator profiles hold an executable path, an argument template, and accepted extensions. Presets ship for RetroArch, Dolphin, PCSX2, RPCS3, Ryujinx, and the yuzu family (Eden, Citron); any emulator with a command-line interface can be added. On Linux an emulator installed as a Flatpak is supported by setting the executable to `flatpak` and starting the template with `run <app-id>`; presets detect this automatically: a preset whose Flatpak is installed says so in the list, and choosing it creates the profile with `flatpak run <app-id>` without asking for a program.
   * Scanning rules: one entry per `.m3u` playlist when present (its discs are hidden); `.cue` hides its `.bin` files; folder-based games are matched by a marker file defined in the profile (for example `EBOOT.BIN` for RPCS3); Switch updates and add-on content (`.nsp` files whose title ID does not end in `000`, or tagged `[UPD]` / `[DLC]`) are not games and are left out. A title is the file name without the tags at its end ("Hades [0100535012974000][v0]" is "Hades"); disc numbers are kept.

### Launch Template Syntax

One syntax everywhere. The template contains **arguments only**; the executable always comes from the emulator profile.

| Placeholder | Meaning |
| --- | --- |
| `{rom}` | Full path to the ROM file or game folder |
| `{rom_dir}` | Directory containing the ROM |
| `{rom_name}` | File name without extension |

Example (Dolphin): `-b -e "{rom}"`

The template is split into an argument vector and passed to the process directly. It never goes through a shell, so paths with spaces or special characters cannot break or inject commands.

## 3. Main Screen Design & Visual Experience

The interface should feel calm, tactile, and responsive on a 4K television viewed from a couch.

### Layout & Visual Hierarchy

* **Top Bar**
  * Left: filter pills `All`, `Recent`, `Favorites`, `Steam`, `Epic`, `GOG`, `Apps`, `Emulators`, switched with `LB / RB`. Pills for sources with no games are hidden.
  * Right: the search box, controller connection and battery, clock, and a settings gear (clickable). The battery comes from `gilrs` where the OS reports it there, and from XInput on Windows (four levels, or "wired").

* **Hero Carousel**
  * A single horizontal row of **2:3 box-art cards** (or the user-selected aspect ratio).
  * **Default view and ordering:** the `All` pill, sorted by `last_played` descending, then `date_added` descending, then title. Recently played games are therefore first, and the screen is never empty on first run. `Recent` shows games played within a time window: the last 30 days by default, adjustable in Settings to 7, 30, or 90 days. The pill is hidden when nothing falls inside the window.
  * **Active card:** clearly the focus of the screen: about half the screen height, roughly `1.45x` the size of the other cards. It gains a crisp border and the glow, and shows the game's transparent logo (or its title in text) with a small source badge in the hero zone above it.
  * **Inactive cards:** smaller and slightly darkened, so attention goes to the active card while every other game stays recognizable. Covers are not blurred. Size and darkening transition smoothly as focus moves.
  * **Corners:** every card, its cover image included, has rounded corners (inside the border on the active card). The renderer rounds an image's own corners and cannot clip one to a rounded shape, so each card shape has its own image, cropped to that shape at ingestion (5.2); no corner shows past the outline whatever the source artwork's proportions.
  * **Empty state:** if no games exist at all, the carousel is replaced by a single "Add your games" card that opens setup (selecting or clicking it).

* **Ambient Background & Lighting**
  * Behind the carousel: a blurred version of the focused game's hero art, or a dark obsidian canvas.
  * A soft, slowly pulsing glow behind the active card, colored from the game's artwork, and a matching tint on the background gradient. Colors crossfade as focus moves.

* **Bottom Action Bar**
  * `[A] Play`, `[X] Search`, `[Y] Options`, `[LB/RB] Filter`, `[Menu] Settings`, and `[B] Hold to exit` when no search is open. Glyphs match the connected controller family where it can be detected. Every glyph and icon in the interface (controller buttons, arrows, the gear, the search and keyboard icons) is a small SVG drawn on a 24-unit grid and embedded in the binary, so it is centered in its box on every font.
  * The hints follow the last input device: controller glyphs after gamepad input, key names (`Enter`, `Esc`, `Tab`, …) after keyboard or mouse input. Every hint is also a clickable button.

* **Keyboard and Mouse**
  * Keyboard: arrow keys move, `Enter` plays, `Esc` goes back or clears the search, `Tab` / `Shift+Tab` (or `PgUp` / `PgDn`) switch pills, `Home` / `End` jump to the first or last game, typing searches, `F11` toggles full screen. Holding `Esc` (or `[B]`) for about 1.4 seconds on the home screen exits Moonglass, with a filling bar after a quarter of a second; a short press does nothing more than before, and no game may be running. In the Quick Menu (5.8), the arrow keys move and change the volume, `Enter` chooses, `Esc` returns to the game and `Del` stops it.
  * Mouse: a program file dropped on the window is added as a standalone game (other files are explained in a message); clicking a card focuses it and clicking the focused card plays it; the wheel (or a touchpad swipe) moves one card per notch; pills, action-bar hints, the search clear button and overlay buttons are clickable. Hovered elements are highlighted.

* **Search**
  * The search box sits at the top right of the top bar. `[X]` focuses it and opens the on-screen keyboard as a sheet at the bottom of the screen; typing on a physical keyboard or clicking the box focuses it directly. A focused box is outlined and shows a blinking caret.
  * The carousel filters live as characters are entered. Matching is case-insensitive on any part of the title and ignores punctuation and accents.
  * Search covers every installed, non-hidden game regardless of the active filter pill. Results use the default ordering.
  * `[A]` on the keyboard's "Done" key (or `Enter`) moves focus to the results; `[B]` (or `Esc`, or the box's × button) clears the search and restores the previous pill and position.
  * No matches shows "No games match" with the query, not an empty row.

### Customization (in-app, no config files)

* **Carousel Geometry:** Vertical Posters (2:3), Wide Banners (16:9, using the hero art), or Square Tiles (1:1); card spacing (Compact, Normal, Wide); focused card size (Normal, Large, Extra large).
* **Card Art:** how art sits on a card whose shape it does not have (a square Switch icon on a poster card): Fit Whole Art (shown whole on a blurred fill made from itself; the default), Zoom to Fill (a center crop that fills the card), or Original Shape (the card takes the art's own proportions, as large as fits in its place in the row, so the row keeps its rhythm).
* **Ambient Mode:** Dynamic Color Bloom, Full-Screen Hero Fanart, Full-Screen Art + Motion Blur (the art with sideways streaks at rest; when the focus moves the new art slides in from the direction of travel, fully smeared, and sharpens as it settles), or Pure OLED Dark; glow (Off, Subtle, Normal, Bright); background blur strength (chosen from pre-computed levels).
* **Card Metadata:** Art Only, Art + Title, or Art + Playtime & Source Badge, shown under each card. Favorites carry a star and hidden games (when shown) a "Hidden" tag.
* **Startup Animation:** on by default; the wordmark, then the cards rise in one after another (about 1.5 s). Off skips it.
* **Reduced Motion:** disables the glow pulse, the startup animation and every transition (highlights, panels, the on-screen keyboard, list changes).
* **Clock:** 24-hour or 12-hour.

All of these live in Settings → Appearance, apply immediately, and are saved.

### Stores

Steam, Epic and GOG can each be turned off, in first-time setup or in Settings → Library ("Show Steam games", and so on). A store turned off is still scanned, so turning it back on is instant and its games keep their favorites, playtime and artwork; but its games are not shown in the carousel, under any pill, or in search, and its pill disappears. Standalone executables and emulated games are the player's own additions and are always shown. All stores are on by default.

### First-Run Flow

Shown on the first start when the library is empty, from the "Add your games" card, and from Settings → System → First-time setup. A glass panel over the blurred backdrop, one step at a time, with dots showing progress; `[A]` chooses, `[B]` goes back (on the first step it skips setup). Finishing or skipping is remembered, so it never opens by itself again.

1. Scan Steam, Epic and GOG automatically and show the count found. Each store found on this computer (its client, or some of its games) is listed with its number of games and a switch, all on by default; the player can turn off the stores whose games they do not want to see (Stores, below). The step opens with Continue selected, so a quick `[A]` never turns a store off; up/down move, `[A]` or left/right flip a switch.
2. Offer to add emulators and ROM folders (skippable): the emulator presets, then the usual profile menu.
3. Offer to enter a SteamGridDB API key (skippable; see 5.2).
4. Land on the carousel. Artwork fills in as it downloads.

## 4. Technical Architecture (Rust + GPUI)

Moonglass is one self-contained Rust binary with no WebView or JavaScript runtime. The whole UI is rendered on the GPU through **GPUI**, Zed's UI framework.

### 4.1 Rendering Constraints That Shape the Design

GPUI renders through DirectX 11 on Windows and through `wgpu` (Vulkan, with an OpenGL fallback) on Linux, where it supports both X11 and Wayland. Moonglass uses only GPUI's cross-platform primitives, so the look is identical on both. Upstream GPUI provides solid fills, two-stop linear gradients, blurred box shadows, images, and paths. It does not provide radial gradients, image blur filters, or spring physics. The visual design is built from what exists:

| Effect | Implementation |
| --- | --- |
| Glow behind active card | Two or three stacked box shadows with large blur radii in the game's accent and dominant colors; the pulse animates their opacity and spread, and pauses where it is while Moonglass does not have the focus |
| Blurred hero background | Blurred **at ingestion** on the CPU (downscale, Gaussian blur, save as a small texture), then stretched by the GPU. Three strengths are pre-computed |
| Background tint | Two-stop linear gradient from the dominant color to near-black |
| Color and art crossfade | Two stacked layers with animated opacity |
| Motion-blurred background | A sideways-smeared copy of the hero art made at ingestion (three box blurs along each row), laid over the sharp art with an opacity that follows scrolling speed; both layers are wider than the screen and slide with the focus |
| Blurred backdrop (Settings, launching, Now Playing) | The strongest pre-blurred hero of the focused game, tinted with its color, with no cards drawn, so the whole screen is one even surface. The carousel crossfades into it |
| Interface motion | Highlights, panels, sheets, toggles and list changes glide on small critically-damped springs keyed by what is drawn; a value that is no longer drawn is forgotten |
| Card motion | A small critically-damped spring implemented in Moonglass and stepped once per frame |

GPUI is pre-1.0 and its API changes between releases. The dependency is pinned to an exact revision and upgraded deliberately.

### 4.2 Technology Stack

* **UI:** `gpui`, pinned.
* **Async:** `tokio` for I/O (network, database, process monitoring), connected to GPUI through the same bridge pattern Zed uses (`gpui_tokio`: a small tokio runtime whose tasks are returned to GPUI as GPUI tasks). UI state is touched only on GPUI's foreground executor.
* **Persistence:** `sqlx` 0.9 with its bundled SQLite (3.51.3 or later, which fixes SQLite's WAL-reset corruption bug), compile-time checked queries in offline mode (`cargo sqlx prepare`) so builds need no live database, and versioned migrations.
* **Gamepad (foreground):** `gilrs` on a dedicated thread, using its default backend on each platform, for broad controller support while Moonglass has focus.
* **Gamepad (background hotkey):**
  * Windows: a separate minimal XInput poller via `windows-rs`. This exists because the default `gilrs` backend there (Windows Gaming Input) delivers input only to a focused window, and the restore hotkey is needed precisely when Moonglass is not focused.
  * Linux: the same `gilrs` thread. Its Linux backend reads controllers through evdev, which does not depend on window focus, so no second poller is needed.
* **OS integration:** `sysinfo` for process enumeration on both platforms. Windows: `windows` (`windows-rs`) for `ShellExecuteW`, window focus, Job Objects, registry, and global hotkeys. Linux: `rustix` for process groups and signals, `/proc` for process environment, and `xdg-open` for URIs.
* **Paths:** the `directories` crate, so each platform's conventions are followed.
* **Images and color:** `image` for decoding, resizing, and blurring; a small k-means palette extractor of Moonglass's own for dominant and accent colors.
* **Parsing and scanning:** `keyvalues-parser` (Steam VDF/ACF), `serde_json` (Epic, GOG), `walkdir` and `notify` (directories).
* **HTTP:** `reqwest`.
* **Diagnostics:** `tracing` with a rotating log file.

### 4.3 Platform Layer

Everything OS-specific is behind one internal interface with a Windows and a Linux implementation. No other module contains platform conditionals. Both implementations are built and tested in CI from Phase 1.

| Capability | Windows | Linux |
| --- | --- | --- |
| Open a launcher URI | `ShellExecuteW` | `xdg-open`, or the launcher's own command |
| Track and kill a spawned game | Job Object | New process group; the group is signalled to terminate |
| Detect a launcher-started game | Process executable under the install folder | `SteamAppId` in the process environment; otherwise executable, working directory, or command line under the install folder |
| Hide Moonglass while a game runs | Minimize the window | Stay fullscreen behind the game and stop rendering |
| Regain focus after a game | `SetForegroundWindow` with fallbacks | Usually automatic; activation request as a fallback (5.3) |
| Background restore hotkey | XInput poller; `RegisterHotKey` for the keyboard | `gilrs` via evdev; keyboard shortcut on X11 or through the desktop's global-shortcuts portal |
| Ask a capture tool to save a clip or screenshot (5.8) | The tool's default shortcut, sent with `SendInput` | `SIGUSR1` to `gpu-screen-recorder` |
| A game's own volume (5.8) | Core Audio sessions (`ISimpleAudioVolume`) of the game's processes and their children, on every active output | PulseAudio / PipeWire streams of those processes, through `pactl` |
| Start at login | `HKCU\...\Run` registry value | `.desktop` file in `~/.config/autostart/` |
| Single-instance guard | Named mutex | Lock file plus a Unix socket in the runtime directory |
| Steam, Epic and GOG discovery | Registry and `ProgramData` | Known directories, including Flatpak and Snap locations |

### 4.4 File Locations

| | Windows | Linux |
| --- | --- | --- |
| Database (which holds the settings) | `%APPDATA%\Moonglass\` | `~/.local/share/moonglass/` |
| Artwork and derived textures | `%LOCALAPPDATA%\Moonglass\cache\` | `~/.cache/moonglass/` |
| Logs | `%LOCALAPPDATA%\Moonglass\logs\` | `~/.local/state/moonglass/` |

The Linux paths honor the XDG environment variables when they are set.

Moonglass is single-instance. A second launch brings the existing window forward and exits.

### 4.5 Distribution

* Windows: a single `.exe`, plus a zip (`scripts/package-windows.ps1`).
* Linux: a tarball and an AppImage, both with a desktop entry and icon (`scripts/package-linux.sh`). Distribution packages and a Flatpak are out of scope for v1.0 (see Non-Goals).
* A `v*` tag builds both on CI and attaches them to a GitHub release.

## 5. Core Subsystems

### 5.1 Library Scanner

**When scans run**

* At startup, behind the startup animation: the library as stored is read while the UI toolkit starts, so the first frame already shows it, and the scan then brings it up to date.
* When Moonglass resumes after a game exits.
* On file-system events (debounced by 2 seconds) from watchers on every Steam `steamapps` folder, the Epic manifests folder, Heroic's GOG folder on Linux, and every ROM directory. GOG on Windows is in the registry, which is not watched: a GOG game installed while Moonglass runs appears after the next game or a rescan from Settings.
* On demand from Settings.

**Reconciliation rules**

* Every game has a deterministic ID (section 6), so a rescan updates rows in place.
* A game that disappears is marked `is_installed = 0` and hidden. It is not deleted, so favorites, playtime, and custom artwork survive a reinstall.
* `last_played` for Steam games is seeded from the manifest's `LastPlayed` value when Moonglass has no value of its own, so the first-run order matches the user's real history.

**Playtime**

* Moonglass counts its own sessions in `playtime_seconds` (5.3), for every source.
* Steam keeps its own count per account, which includes play outside Moonglass and on other computers: `Software/Valve/Steam/apps/<AppID>/Playtime`, in minutes, in `userdata/<account ID>/config/localconfig.vdf`. Each scan copies it for the signed-in account into `store_playtime_seconds`, replacing the previous figures, so switching account (5.7) switches them too.
* The playtime shown is the larger of the two. Steam's count already includes the sessions Moonglass recorded, so they are never added.
* Epic and GOG keep their counts on their servers only; for those games Moonglass's own count is shown.

### 5.2 Artwork Pipeline

Sources are tried in this order. The first hit wins per asset type (cover, hero, logo).

1. **User override.** Chosen through `[Y] → Change Artwork`. Overrides are flagged and never replaced by a rescan.
2. **Steam's local library cache** (Steam games only). Steam already stores cover, hero, and logo images on disk; using them needs no network and no API key.
3. **The Epic Games Launcher's catalog cache** (Epic games on Windows). The launcher keeps a list of the player's library with links to each game's artwork on Epic's image host (`Data\Catalog\catcache.bin`, next to the manifests folder); the cover and hero are downloaded from there, with no API key. A game left with a placeholder is tried again on later refreshes.
4. **GOG's public catalog** (GOG games). `api.gog.com/v2/games/<product ID>` needs no key and links a box-art cover (`boxArtImage`), a wide background (`galaxyBackgroundImage`, else `backgroundImage`) and a transparent logo (`logo`) on GOG's image host; only `https` links to GOG's own hosts are followed. A game left with a placeholder is tried again on later refreshes.
5. **SteamGridDB**, queried by Steam AppID for Steam games and by title search for everything else. Requires the user's own free API key, entered in Settings. Without a key this step is skipped.
6. **Keyless game databases, for emulated games.** Switch games are looked up by title ID on nlib.cc, which serves each title's eShop icon (used as the cover) and its 16:9 banner (the hero art); the ID comes from the dump's file name (`[0100535012974000]`) or, failing that, from the games the Ryujinx family of emulators has already run (`games/<title id>/gui/metadata.json` holds the name). Other systems use the libretro thumbnail collection's box art, named after No-Intro / Redump dump names; the system is chosen from the emulator and the file type, and the dump's own name is tried before the clean title with the usual region tags. A game left with a placeholder is tried again on later refreshes.
7. **Generated placeholder.** A title card with the game's name on a gradient derived from a hash of the title.

**On ingestion, once per asset:**

* Resize the cover to the largest size the carousel can display (the focused card on a 2160p screen).
* Make one image per card shape: 2:3 and 1:1 from the cover, 16:9 from the hero art. A cover about as wide as the card or taller is center-cropped; one much wider (a square icon or a landscape box on a poster card) is shown whole on a blurred, darkened fill made from itself. A center crop of every cover is kept too, for the Zoom to Fill card art setting.
* Extract dominant and accent colors. Both are clamped to a minimum saturation and a lightness band so that dark or grey covers still produce a visible glow.
* Produce the three blurred hero variants and the motion-blurred one (4.1). Images added to the pipeline later are made for existing artwork on the next refresh, without downloading anything again.
* Store paths and colors in SQLite.

**At runtime:** only visible cards plus a few neighbors on each side are drawn. Decoding happens off the UI thread. GPUI keeps every decoded image, in memory and in the GPU's sprite atlas, until told otherwise; so each card and background image is noted when drawn, and those not drawn for 20 seconds are released from both (decoded again if they come back). When a game starts, everything but the last moment's images is released.

v1.0 uses static images only.

### 5.3 Launch & Focus Lifecycle

Each launch moves through an explicit state machine:

```
Idle → Launching → Running → Exiting → Idle
            ↓ (timeout or cancel)        ↑
            └────────────────────────────┘
```

**Launching**

* Show the "Launching *Game*…" overlay and stop the carousel's animation. (The overlay itself fades in and then draws no more frames.)
* Start the game (below) and wait for evidence that it is running.
* After 20 seconds the overlay adds "Still waiting… `[B]` Cancel". After 3 minutes (1, 3, 5 or 10 in Settings → System) Moonglass gives up and returns to Idle. A long wait is normal when Steam updates a game first.

**Detecting start and exit**

| Source | Windows | Linux |
| --- | --- | --- |
| Executable, ROM | The spawned child, placed in a **Job Object** so its children are tracked and can be terminated together | The spawned child, started in its own **process group** for the same purpose |
| Steam | Any process whose executable lives under the game's install folder | Any process whose environment contains `SteamAppId=<AppID>`. Steam sets this on the game's own processes, native or Proton, which avoids the problem that a Proton game's executable is Wine rather than a file in the install folder |
| Epic | Same as Steam on Windows | Any process whose executable, working directory, or command line points inside the install folder (the game runs under Wine, started by Heroic) |
| GOG | Same as Epic. Started without Galaxy, the child is watched too, since GOG's programs are often small launchers | Same as Epic |

Polling is once per second with `sysinfo` on both platforms.

Rules that make the folder signal reliable:

* **Ignore list.** Helper processes such as crash handlers and redistributable installers are not counted as the game. The list is built in and extendable.
* **Exit grace period.** When the last matching process ends, Moonglass waits 5 seconds. If another matching process appears (a launcher handing off to the real game), the state returns to Running.
* **Per-game override.** Options allow naming the exact process to watch, for games whose real executable lives outside their install folder.
* **Launcher-style executables.** A per-game toggle switches a standalone executable from child-process tracking to folder tracking, for stubs that start the real game and exit.
* **Folders too broad to be a game.** A drive root, the home, Desktop, Documents or Downloads folder, `Program Files`, the application-data folders, and anything inside the Windows folder (on Linux: `/usr/bin`, `/opt` and similar) hold unrelated programs. Such a folder never matches a process, so Stop and Force stop cannot reach anything but the game; a launcher-style executable in one falls back to child-process tracking.
* **Steam registry signal (Windows, to validate in Phase 0).** Steam appears to record per-app running state in the registry under `HKCU\Software\Valve\Steam\Apps\<AppID>`. If the spike confirms it is dependable, it becomes the primary Steam signal on Windows and the folder watch becomes the fallback. Steam keeps a `Running` value (0/1) per AppID there. To gather evidence, Moonglass logs that value next to its own process detection whenever it changes during a Steam session, and `moonglass steam-signal <game>` prints both once per second while the player starts and quits the game.

**Running**

* Windows: the window is not minimized, which would show the desktop and every other window while the game is still opening its own; the game comes in front of it. A game started through its store's client may not be allowed to take the foreground, so while Moonglass is still in front it hands the foreground to the game's window once it appears (up to a minute). Return to game on the Quick Menu brings the game's window back in front, restoring it if it minimized itself (exclusive fullscreen does on losing focus); only if that window cannot be found is Moonglass minimized instead. Linux: the window stays fullscreen underneath the game. Wayland gives an application no way to un-minimize or raise itself, so Moonglass never minimizes there; when the game's window closes, the compositor returns focus to the fullscreen window beneath it.
* On both platforms Moonglass renders zero frames in this state.
* **Now Playing screen.** Whenever Moonglass is visible while a game runs (after Alt-Tab, the restore hotkey, or on Linux when it is on top), it shows "Now playing *Game*" over the blurred backdrop (4.1), how long the game has been running and the game's total playtime with this session, the time and the controller's battery (which a full-screen game hides), and the Quick Menu (5.8). The screen disappears as soon as the game exits, whether the player quit it or it was stopped.
* **Stop game.** The first press asks the game to close normally (a window close request on Windows, `SIGTERM` on Linux) so it can save. If it is still running after 8 seconds, the button becomes **Force stop**, which terminates it (the Job Object or process group for executables and ROMs; the detected processes for Steam, Epic and GOG games). Moonglass never stops a game without the player asking.
* Polling drops to once per second, and the process list is read only for games found through it (Steam, Epic, GOG, folder tracking); a game started as Moonglass's own child is watched through its handle alone. Background work that has nothing to show (friends, the Steam account watcher) rests. Gamepad navigation is suspended. The only input still observed is the restore hotkey (5.4).

**Exiting → Idle**

1. **Regain focus, Windows.** Restore the window (`ShowWindow(SW_RESTORE)`), then `SetForegroundWindow`. Confirm with `GetForegroundWindow`. If focus was not obtained, retry with the `AttachThreadInput` technique, then with a synthetic `Alt` key event, up to three attempts in total.
2. **Regain focus, Linux.** Check whether the window is active. If it is not, request activation through GPUI (an `_NET_ACTIVE_WINDOW` request on X11; an activation request on Wayland, which the compositor may decline).
3. If focus still fails on either platform, raise an attention request (taskbar flash or the desktop's "demands attention" hint), show a desktop notification saying how to return, and log the failure. Never loop indefinitely. Windows: the notification is a tray balloon, which Windows 10 and 11 show as a regular notification; the tray icon goes away after a while. Linux: `notify-send`, two seconds after the activation request if the window is still not active.
4. **Dismiss the launcher window.** If the Steam, Epic, GOG Galaxy or Heroic window is in front after a game from that client exits, Moonglass asks the client to hide it. On Windows this is a normal window-close request, which sends the client to the system tray. On Linux, Moonglass cannot close another application's window on Wayland, so it avoids the window appearing in the first place: Heroic is always started with `--no-gui`, and Steam is started minimized to the tray if Moonglass is the one starting it. On X11 a close request is also sent. In all cases the client process keeps running so cloud saves finish syncing, and dialogs such as a cloud-save conflict prompt are left alone. On by default; can be turned off in Settings → System.
5. Set `last_played`, add the session length to `playtime_seconds`, rescan (which also refreshes Steam's playtime, 5.1), and re-render.

Playtime counts from the Running state to exit, so update and launcher time is excluded.

### 5.4 Gamepad Navigation and the Restore Hotkey

**Foreground navigation (`gilrs`)**

* A dedicated thread blocks on controller events and sends typed actions (`NavLeft`, `NavRight`, `NavUp`, `NavDown`, `Confirm`, `Back`, `Options`, `Search`, `FilterPrev`, `FilterNext`, `OpenSettings`) to GPUI through a channel.
* The left stick uses a radial deadzone. Holding a direction repeats with acceleration.
* Carousel navigation is suspended whenever the state is not Idle; the Launching screen and the Quick Menu (5.8) take only their own controls. Nothing is dispatched while Moonglass's window is not focused.

**Restore hotkey**

* Active only in the Launching and Running states.
* Default: hold both sticks pressed in (L3 + R3) for 2 seconds. The hold prevents collisions with games and emulators that use the same pair as a tap. `View + Menu` is not the default because Windows can treat that pair as the Xbox button (Game Bar's "Use View + Menu as Guide button in apps"), and holding the Xbox button opens Task View over the game. Configurable in Settings → System: the buttons (both sticks pressed in, or View + Menu) and the hold time (1, 2 or 3 seconds), applied to the watching threads at once.
  * Xbox: View (two overlapping rectangles, left of the Xbox button) + Menu (three lines, right of it).
  * PlayStation: Share (DualShock 4) or Create (DualSense) + Options.
  * Nintendo Switch Pro: − (Minus) + + (Plus).
  * Settings → System shows the names for the connected controller.
* The `Guide` button is not offered: the public XInput API does not report it, and Windows Game Bar and Steam claim it on both platforms.
* **Windows:** read by the XInput poller, about ten times per second, for connected controllers only (asking an empty slot is slow); empty slots are checked every two seconds. Keyboard equivalent, registered as a global hotkey: `Ctrl + Alt + Home`. This also covers controllers that XInput cannot see.
* **Linux:** read by the `gilrs` thread, which keeps receiving events in the background and works with any controller the kernel recognizes. The keyboard equivalent is registered directly on X11 and through the desktop's global-shortcuts portal on Wayland; where the desktop offers neither, only the gamepad combination is available and Settings says so.
* Action: bring Moonglass forward on the Now Playing screen (5.3), on the Quick Menu (5.8). On Wayland, if the compositor declines to bring Moonglass forward, the desktop notification from 5.3 is shown instead.

### 5.5 Settings, File Browser, and Text Entry

* **Settings** is a full-screen view of glass panels over the blurred backdrop (4.1), opened with `[Menu]`, `Ctrl+,` or the gear, and navigable by gamepad, keyboard and mouse. It opens with the focus on the list of sections; right or `[A]`/`Enter` moves into the chosen one. Sections: Appearance (section 3, Customization), Library (add a game, `Recent` window, show hidden games, rescan, which stores to show (section 3, Stores; each row says how many games the store has, or that it was not found), the Steam account and whether to show only the Steam games it can play (5.7), the Steam folder (found automatically, or chosen), the Steam apps marked as not games (listed again from here), the Steam Web API key for friends, 5.6), Emulators (one row per emulator profile, and "Add an emulator" which offers the presets and then asks for the program; a profile's menu changes its program, arguments and file types, adds a ROM folder with its platform name, and removes folders or the profile, each removal asking for a second press), Artwork (SteamGridDB key, refresh all artwork), System (start at login, send the launcher to the tray after a game, the restore hotkey for the connected controller with its buttons and hold time, the tool for clips and screenshots with what it needs (5.8), how long to wait for a game to start, first-time setup, logs, version, Exit Moonglass). `Ctrl+Q`, or holding `Esc` / `[B]` on the home screen, also exits when no game is running. A section with more rows than fit shows a window of them that follows the selection, with arrows where there are more. `LB/RB` or `Tab` switch sections, left/right change values, `[A]`/`Enter` selects, `[B]`/`Esc` goes back.
* **File browser:** a built-in list view of folders, used wherever a path is needed. It starts from the home folder plus the drive list on Windows, and from the home folder plus mounted volumes on Linux. Filters by extension when picking a file, or by the executable bit when adding a Linux executable.
* **On-screen keyboard:** a bottom sheet used for library search and the few text fields (API key, launch arguments, process to watch, emulator arguments and file types, platform name). Its last row has the characters arguments and paths need (`{ } " / \ : _ = + @`). Gamepad: `[A]` types, `[X]` deletes, `[Y]` space, `[Menu]` done, `[B]` cancel. Keys are clickable, and a physical keyboard types into the same fields (`Enter` done, `Esc` cancel). In the file browser, typing a letter jumps to the next entry starting with it.
* **Per-game options (`[Y]`, `Ctrl+O`, or right-click on a card):** Play, Favorite, Hide, Change Artwork (and back to automatic artwork once changed), Launch Arguments (standalone executables, ROM entries, and Steam games as launch options; the Epic launcher and GOG Galaxy accept none), Remove (standalone executables and ROM entries), Working Directory and the launcher-style toggle (standalone executables), Process to Watch, and Not a Game for Steam apps (adds it to the deny-list, 2). Remove and Not a Game ask for a second press.
* **Start at login:** a toggle. Windows: writes or removes a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Linux: writes or removes `~/.config/autostart/moonglass.desktop`.

### 5.6 Steam Friends

Optional, and off until the player enters their own Steam Web API key in Settings → Library (created at `steamcommunity.com/dev/apikey`). Epic and GOG have no public friends API, so this covers Steam games only.

* **Source.** Once a minute while no game is running, Moonglass asks Steam's Web API for the player's friends list and for which game each friend is in. The player's own account is the one Steam is signed in with (5.7), or, while Steam is closed, the one `config/loginusers.vdf` marks; nothing but the key is entered. The key is stored like the SteamGridDB key, sent only to `api.steampowered.com`, and never logged.
* **In the carousel.** A card shows a green dot with the number of friends playing that game; the hero zone names them ("Alex and Sam are playing").
* **Join.** When a friend's session can be joined (Steam reports a lobby), the game's Options menu has "Join *Name*" after Play. It opens `steam://joinlobby/<AppID>/<lobby>/<friend>` and the launch is tracked like any other.
* **Borrowed games.** A game's install manifest records the account whose license it uses (`LastOwner`). When that is not the player's account and its owner is playing the game, the card is tagged "In use" and the hero zone says "In use by *Name*". The first press of Play explains this; a second press starts the game anyway, because the family may hold another copy, which Moonglass cannot see. Who belongs to the player's Steam Family is not available through the official API, so only owners who are also friends are noticed here (the account switcher reads the family from Steam's local files instead, 5.7).
* Friends who appear offline or keep their profile private are not shown. Settings shows how many friends were found and how many are in a game, or why the request failed.

### 5.7 Steam Accounts

Steam has one account signed in at a time; the others it remembers (`config/loginusers.vdf`) appear in its own account switcher. On a shared computer, Moonglass offers the same choice.

* **Choosing.** Settings → Library → Steam account lists the remembered accounts, marking the signed-in one and those whose password Steam did not save. Choosing one closes Steam (`steam -shutdown`), makes it the account Steam signs in with automatically once Steam has closed (`HKCU\Software\Valve\Steam\AutoLoginUser` on Windows; the same value in `registry.vdf` on Linux), starts Steam again in the background, and waits up to 90 seconds until `ActiveUser` names that account. No password is involved: Steam uses the sign-in it kept; if that has expired, Steam shows its own sign-in window and Moonglass says the switch did not finish. Not while a game runs.
* **Following Steam.** Every few seconds Moonglass reads which account Steam is signed in with, so a switch made in Steam itself is followed too.
* **What an account can play.** Pieced together from Steam's local files: an installed game's `LastOwner`; the account's app tickets (`userdata/<account id>/config/localconfig.vdf`, `apptickets`), kept for every app it has a license for and has run or installed, free-to-play games included; and its Steam Families group (`FamilyGroup` in the same file), whose members share their libraries. An account that never signed in on this computer has only its own copies. With "Only Steam games this account can play" on (the default), other installed Steam games are left out of the carousel. Never applied when only one account is remembered. A free-to-play game the account has never run here is not recognized; turning the setting off shows everything.
* The login names stay inside Moonglass; only display names are shown, and the login name is checked to be letters, digits and underscores before it is written anywhere.

### 5.8 Quick Menu

The Now Playing screen (5.3) holds a short menu: what a console's guide button offers, without drawing anything inside the game. Moonglass comes forward over the game, which keeps running behind it; without the focus, the game stops taking the controller, so the menu needs no input hooks of its own.

* **Why not an in-game overlay.** Drawing inside the game means injecting a library into its process and hooking its rendering. Anti-cheat software (Easy Anti-Cheat, BattlEye, Vanguard) blocks or flags unknown injected code, every graphics API needs its own hooks, and a crash in the overlay is a crash of the game. Moonglass stays outside the game, as it stays outside the store clients (section 1, Native Integrity).
* **Rows**, top to bottom, moved through with up and down (or the mouse); `[A]`/`Enter` chooses:
  * **Return to game** (also `[B]`/`Esc`). Moonglass hides and the game is in front again.
  * **Save a clip**, when a capture tool is chosen. Asks the tool to save the moments just played.
  * **Take a screenshot**, when the tool takes screenshots.
  * **Game volume.** Left and right change the game's own volume in 10% steps (the OS mixer's per-program volume, not the system volume); `[A]` mutes or unmutes; raising it unmutes. The volume belongs to every audio stream of the game's processes and the processes they started, so a game behind its own launcher is covered. "No sound from the game" while it has none. Read whenever the menu appears.
  * **Stop game** (also `[X]`/`Del`), then Force stop, as in 5.3.
* **Capture tools.** Moonglass never records. It asks a tool that already records the game, chosen in Settings → System → Clips and screenshots, under which a row says what the tool needs:

  | Tool | Platform | Clip | Screenshot | Needs |
  | --- | --- | --- | --- | --- |
  | Xbox Game Bar (default on Windows) | Windows | `Win+Alt+G` | `Win+Alt+PrtScn` | "Record what happened" turned on in Windows Settings → Gaming → Captures |
  | NVIDIA Instant Replay | Windows | `Alt+F10` | `Alt+F1` | Instant Replay turned on in the NVIDIA overlay |
  | AMD Instant Replay | Windows | `Ctrl+Shift+S` | `Ctrl+Shift+I` | Instant Replay turned on in AMD Software |
  | gpu-screen-recorder | Linux | `SIGUSR1` to its process | none | Running in replay mode (`-r`) |
  | Off (default on Linux) | | | | Save a clip and Take a screenshot are left out |

  The Windows tools are driven by their default shortcuts, which act on the window in front: Moonglass first returns to the game, waits 0.6 seconds, then presses the shortcut, and the tool shows its own confirmation over the game. If Windows does not pass the shortcut on (a game running as administrator when Moonglass is not), a desktop notification says so. gpu-screen-recorder is signalled from the menu itself, which then says it was asked or why it could not be (not running).
* **Not yet:** choosing the audio output device (Windows has no public API for it), performance figures (CPU, GPU, frame rate), OBS's replay buffer through obs-websocket, and shortcuts the player changed in their tool.

## 6. Data Schema (SQLite)

IDs are deterministic so rescans update in place:

* Steam: `steam:<AppID>`
* Epic: `epic:<AppName>`
* GOG: `gog:<product ID>`
* Executable: `exe:<hash of normalized path>`
* ROM: `rom:<hash of normalized path>`

Path normalization resolves symlinks and relative segments on both platforms, and additionally lowercases on Windows, where paths are case-insensitive. The schema is identical on both platforms; `source = 'exe'` means "standalone executable" regardless of file type, `source = 'epic'` covers Epic games whether they are launched by the Epic client or by Heroic, and `source = 'gog'` covers GOG games from GOG Galaxy, GOG's offline installers, or Heroic.

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE emulators (
    id               TEXT PRIMARY KEY,
    name             TEXT NOT NULL,            -- "Dolphin", "PCSX2", "RetroArch"
    executable_path  TEXT NOT NULL,
    launch_template  TEXT NOT NULL,            -- arguments only, e.g. -b -e "{rom}"
    file_extensions  TEXT NOT NULL,            -- ".iso,.rvz,.gcm"
    folder_marker    TEXT                      -- e.g. "EBOOT.BIN" for folder-based games
);

CREATE TABLE rom_directories (
    id           TEXT PRIMARY KEY,
    path         TEXT NOT NULL UNIQUE,
    emulator_id  TEXT NOT NULL REFERENCES emulators(id) ON DELETE CASCADE,
    platform     TEXT NOT NULL,                -- display label, e.g. "GameCube"
    recursive    INTEGER NOT NULL DEFAULT 1 CHECK (recursive IN (0, 1))
);

CREATE TABLE games (
    id                TEXT PRIMARY KEY,        -- see ID rules above
    title             TEXT NOT NULL,
    sort_title        TEXT NOT NULL,           -- lowercased, leading articles removed
    search_title      TEXT NOT NULL,           -- lowercased, accents and punctuation stripped; matched by search
    source            TEXT NOT NULL CHECK (source IN ('steam', 'epic', 'gog', 'exe', 'rom')),
    source_id         TEXT,                    -- Steam AppID, Epic AppName or GOG product ID; NULL for exe/rom
    launch_target     TEXT,                    -- .exe or ROM path; for GOG, the game's own program (used without Galaxy); NULL for steam/epic
    launch_args       TEXT,
    working_dir       TEXT,                    -- exe only; NULL = executable's folder
    rom_directory_id  TEXT REFERENCES rom_directories(id) ON DELETE CASCADE,
    platform          TEXT,                    -- copied from the ROM directory, for badges
    install_dir       TEXT,                    -- absolute path; watched for steam/epic/gog
    watch_process     TEXT,                    -- optional exact process name override
    track_by_folder   INTEGER NOT NULL DEFAULT 0 CHECK (track_by_folder IN (0, 1)),

    cover_path        TEXT,
    hero_path         TEXT,
    logo_path         TEXT,
    dominant_color    TEXT,                    -- "#RRGGBB"
    accent_color      TEXT,                    -- "#RRGGBB"
    artwork_locked    INTEGER NOT NULL DEFAULT 0 CHECK (artwork_locked IN (0, 1)),

    date_added        INTEGER NOT NULL,        -- Unix seconds
    last_played       INTEGER,                 -- Unix seconds; NULL = never
    playtime_seconds  INTEGER NOT NULL DEFAULT 0,   -- sessions launched from Moonglass only
    store_playtime_seconds INTEGER NOT NULL DEFAULT 0, -- the store's own count (Steam, signed-in account)
    is_favorite       INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0, 1)),
    is_hidden         INTEGER NOT NULL DEFAULT 0 CHECK (is_hidden IN (0, 1)),
    is_installed      INTEGER NOT NULL DEFAULT 1 CHECK (is_installed IN (0, 1)),

    UNIQUE (source, source_id)
);

CREATE INDEX idx_games_order ON games (is_installed, is_hidden, last_played DESC, date_added DESC);

CREATE TABLE settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL                       -- JSON-encoded
);
```

Notes:

* The emulator for a ROM is reached through its ROM directory, so there is one place to change it.
* `playtime_seconds` is Moonglass's own count; `store_playtime_seconds` is Steam's, refreshed on every scan (5.1). The UI shows the larger, never their sum, because Steam's includes Moonglass's sessions.
* The SteamGridDB API key is stored in `settings`. It is a low-sensitivity key for a free service; it is never logged.

### Keeping the library sound

The library is one SQLite file in write-ahead-log (WAL) mode. On 17 November 2026 it was found damaged twice in a day of development: pages were written from out-of-date copies of themselves, and a table and its index disagreed. The exact trigger was not proven. Damage like this comes from SQLite's own WAL-reset bug (present in every SQLite before 3.51.3, Moonglass bundled 3.46.0), from several connections or processes writing at once, or from a log that is never folded back into the file (Moonglass never closed the library, so one log grew across a dozen runs). The rules below remove each of those.

* **A fixed SQLite.** `sqlx` 0.9 bundles SQLite 3.51.3, which contains the WAL-reset fix; a test fails the build if the bundled version is older.
* **One connection per process.** Every read and write goes through it, one at a time; the library is small enough that nothing waits noticeably. Two processes may still share the file (a debug command next to the carousel, a development second instance); SQLite's locking handles that.
* **A clean close.** The carousel closes the library when it quits (waiting up to 3 seconds), and every debug command does when it finishes, so SQLite copies the log into the file and deletes it. A process that is killed leaves its log behind; the next start folds it in at once (`PRAGMA wal_checkpoint(TRUNCATE)`).
* **A check on every start.** `PRAGMA integrity_check` runs before migrations touch the file (a few milliseconds for a library this size).
* **A last good copy.** After a clean check, the library is copied to `moonglass.backup.db` next to it (`VACUUM INTO`, a consistent copy, then a rename over the previous backup).
* **Recovery.** A damaged file, with its `-wal` and `-shm`, is moved to `moonglass.damaged-<unix time>.db` (never deleted) and the backup takes its place; if there is no usable backup, an empty library is created and the next scan finds the store games again. Either way the carousel opens with a message saying what happened, and the log records SQLite's findings.

## 7. Performance Targets

These are acceptance criteria for v1.0, measured on a mid-range gaming PC with a library of 500 games. They apply to Windows and to Linux (verified on one X11 session and one Wayland session).

| Measure | Target |
| --- | --- |
| Cold start to interactive carousel | Under 2 seconds. On Windows a plain black window covers the screen from the first moment, and the startup animation, which begins on black, continues from it. |
| Carousel scrolling | No dropped frames at the display's refresh rate |
| Frames rendered while a game is running | Zero |
| CPU while a game is running | Under 0.5% of one core on average |
| Memory while a game is running | Under 150 MB |
| Return to a focused Moonglass after game exit | Under 2 seconds after the grace period |
| Search results update after a keystroke | Within one frame |

## 8. Phased Roadmap

* **Phase 0: Risk Spike (about one week)**
  * Fullscreen GPUI window with five hard-coded cards, gamepad navigation, the stacked-shadow glow, and a pre-blurred background, on Windows, Linux X11, and Linux Wayland.
  * Launch one Steam game on each platform (on Linux, one native and one Proton title), detect its exit, and recover focus.
  * Windows: validate the Steam registry running-state signal and the background XInput hotkey.
  * Linux: validate `SteamAppId` detection, background gamepad input through evdev, and focus return on GNOME and KDE Wayland sessions.
  * **Exit criterion:** the three highest risks in section 9 are confirmed or the design is adjusted before further work.

* **Phase 1: Core Without UI Polish**
  * Workspace, database, migrations, logging, single-instance guard.
  * The platform layer (4.3) with both implementations, and CI that builds and tests Windows and Linux on every change.
  * Steam scanner for both platforms; Epic scanner on Windows and Heroic-backed Epic scanner on Linux; filtering and reconciliation rules.
  * Launch state machine with Job Objects or process groups, process detection per platform, grace period, timeouts, focus recovery, and launcher-window dismissal.
  * A plain debug list view to launch games and exercise the lifecycle.

* **Phase 2: Carousel, Artwork, and Lighting**
  * Artwork pipeline in source order: Steam local cache, SteamGridDB, placeholder.
  * Ingestion: resize, color extraction, blurred variants.
  * Carousel with spring motion, texture windowing, filter pills, ambient glow and tint.

* **Phase 3: Setup, Settings, and Polish**
  * Controller-navigable settings, file browser, and on-screen keyboard.
  * Standalone-executable adder, emulator profiles (including Flatpak emulators on Linux), ROM directories and scanning rules.
  * Library search.
  * Per-game options, customization, reduced motion, first-run flow, start at login.
  * Packaging: Windows `.exe` and zip; Linux tarball and AppImage.
  * Verification against the section 7 targets on both platforms.

* **Post-1.0**
  * UI sounds and ambient audio (`rodio`), animated covers, more launchers (EA app, Ubisoft Connect, Xbox app, Battle.net, Heroic's Amazon games).
  * Done in revision 1.7: GOG, Steam playtime import.
  * Done in revision 1.8: the Quick Menu (5.8).
  * Later for the Quick Menu: audio output choice, performance figures, OBS's replay buffer.

## 9. Risks and Known Limitations

| Risk or limitation | Impact | Mitigation |
| --- | --- | --- |
| GPUI is pre-1.0, and Windows is its newest platform | API churn; rendering or fullscreen bugs | Pin the revision; Phase 0 spike; keep rendering code behind a thin internal layer |
| Wayland does not let an application take focus for itself | Moonglass may stay behind another window after a game, or when the hotkey is used | Never minimize on Linux, so focus normally falls back to Moonglass; activation request; notification fallback; Phase 0 test on GNOME and KDE |
| Epic on Linux depends on Heroic, a third-party project | Its file locations or launch URI may change; no Heroic means no Epic games | Isolated in one scanner module; source hidden when Heroic is absent; Phase 0 check of the installed-games file |
| Linux install variety (native, Flatpak, Snap; many desktops) | Steam, Heroic, or emulators not found; behavior differs by desktop | Probe all known locations; manual path override; state the tested desktops in the release notes |
| Flatpak-sandboxed launchers on Linux | Game processes may hide their environment or paths from Moonglass | Fall back from `SteamAppId` to path matching; per-game process override |
| Global keyboard shortcuts are inconsistent on Wayland | Keyboard restore hotkey may be unavailable | Gamepad combination works everywhere through evdev; Settings shows what is available |
| Windows restricts which process may take foreground focus | Moonglass may not regain focus after a game | Three-step fallback in 5.3; taskbar flash; restore hotkey as a manual path |
| Folder-based detection can misjudge start or exit | Moonglass returns too early or too late | Grace period, ignore list, per-game override, Steam registry signal if validated |
| Dialogs owned by Steam, Epic, GOG Galaxy or Windows | May need a mouse or keyboard | Documented limitation; the restore hotkey always returns to Moonglass |
| Dismissing a launcher window could close one the user wanted | Minor annoyance | Only the main client window is closed, never dialogs; setting to disable |
| Games that start their own launcher (EA, Ubisoft, Rockstar) | Real game process may sit outside the install folder | Per-game process override |
| On Windows the background hotkey works only for XInput controllers | Other pads cannot summon Moonglass mid-game | Global keyboard hotkey; note in Settings |
| SteamGridDB needs a per-user API key | Non-Steam games show placeholders without it | Steam local cache needs no key; first-run prompt; manual artwork override |
| GOG on Linux depends on Heroic, like Epic | Its file locations or launch URI may change | One scanner module; source hidden when Heroic is absent |
| GOG Galaxy's command line is not documented by GOG | A Galaxy update could change `/command=runGame` | Isolated in the platform layer; the registered `exe` remains a fallback |
| GOG's catalog API is public but undocumented | Artwork lookups may fail | One small module; SteamGridDB and the placeholder remain |
| Epic URI format may vary by title | Some games may not launch from the bare `AppName` | Fallback to the namespaced form |
| GPUI's Windows vsync thread wakes the main thread at the display refresh rate, even while the window is minimized | The section 7 CPU target while a game runs is missed (about 1.4-2.4% of one core measured, all of it GPUI's) | Moonglass's own polling is minimal; stopping the thread for minimized windows needs a GPUI change, to be taken up when GPUI is next upgraded |
| Switching Steam accounts relies on Steam's saved sign-in and its registry values | A switch may stop at Steam's sign-in window, or break if Steam changes how it stores them | Moonglass waits for the account to appear and reports when it does not; Steam's own switcher still works |
| Capture tools are driven by their default shortcuts | A player who changed a shortcut, or a game running as administrator, gets no clip | Settings names the shortcuts used; a failed send is reported in a notification |
| Keyless artwork for emulated games comes from third-party services (nlib.cc, libretro thumbnails) | They may change or go away | Each is one small module; SteamGridDB and the placeholder remain |
