# Project Specification: **Moonglass** (Console-Style PC Game Launcher)

*Spec revision 1.4.*

## 1. Vision & Core Philosophy

**Moonglass** is a lightweight, visually refined, 10-foot (TV-optimized) PC game launcher that turns a Windows or Linux gaming PC into a living-room console experience.

* **Zero Clutter:** No storefronts, news feeds, ads, or dense metadata walls on the home screen.
* **Premium Aesthetic:** Fluid GPU-rendered animation at the display's refresh rate (up to 120 Hz), ambient lighting derived from the selected game's artwork, and clean typography.
* **Native Integrity:** Launching is delegated to the official clients (Steam, Epic) so achievements, cloud saves, Steam Input profiles, and the client's own playtime tracking keep working. Moonglass never replaces or patches a client. On Linux, where Epic has no official client, Epic games are launched through Heroic Games Launcher (section 2).
* **One Codebase, Two Platforms:** Windows and Linux are both first-class. Everything that differs between them sits behind one platform layer (section 4.3); the UI, database, scanners, and artwork pipeline are shared.
* **Controller-First:** Every Moonglass screen, including first-run setup, is navigable by gamepad, and equally usable with keyboard and mouse (section 3, Keyboard and Mouse). Dialogs owned by other programs (Steam EULA prompts, cloud-save conflicts, UAC) are outside Moonglass's control; see section 9.

### Non-Goals (v1.0)

* No store browsing, purchasing, downloading, installing, or updating of games.
* No launchers other than Steam and Epic (GOG, EA, Ubisoft, Xbox app are post-1.0).
* No replacement of the Windows shell or the Linux desktop session. Moonglass is a normal fullscreen app that can start at login.
* No support for running inside a gamescope session (SteamOS Game Mode and similar). Linux support targets regular X11 and Wayland desktop sessions.
* No Flatpak or Snap package of Moonglass itself. A sandbox would block reading other launchers' files and observing game processes.
* No macOS build. The platform layer should not prevent one, but nothing is tested there.

## 2. Scope (v1.0)

**Supported platforms:** Windows 10 and 11, and Linux desktop sessions on X11 or Wayland. x86-64 only.

v1.0 ships exactly this, on both platforms:

1. Four launch sources (below).
2. Automatic artwork with a manual override.
3. The carousel home screen with ambient lighting and built-in customization.
4. Reliable launch, exit detection, and focus recovery.
5. Controller-navigable settings, including a file browser and on-screen keyboard.
6. Library search by title.
7. Optional start at login.

Deferred to post-1.0: UI sound effects and ambient audio, animated cover art, importing playtime from Steam, additional launchers.

### Supported Game Sources

1. **Steam**
   * Steam root, Windows: read from the registry (`HKCU\Software\Valve\Steam\SteamPath`).
   * Steam root, Linux: the first that exists of `~/.local/share/Steam`, `~/.steam/steam`, the Flatpak location (`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`), and the Snap location. The user can set it manually.
   * Libraries come from `libraryfolders.vdf`; games from each library's `steamapps/appmanifest_*.acf`. The file formats are identical on both platforms.
   * A game's folder is `<library>/steamapps/common/<installdir>`. `installdir` in the manifest is a folder name, not a full path.
   * Only fully installed apps are listed. Tools and runtimes (Steamworks Common Redistributables, Proton, Steam Linux Runtime, and similar) are excluded by a built-in AppID deny-list that the user can extend.
   * Launched via `steam://rungameid/<AppID>`, handed to the OS URI handler. On Linux this covers native and Proton games alike; Steam chooses the runtime.
2. **Epic Games Store**
   * **On Linux:** Epic has no official client, so this source is backed by **Heroic Games Launcher**. Games are read from the installed-games list Heroic keeps for its Epic backend (Legendary), in either the native or the Flatpak config location. Launch is `heroic --no-gui "heroic://launch?appName=<AppName>&runner=legendary"` (or the Flatpak equivalent). If Heroic is not installed, the Epic source and its filter pill do not appear. The rest of this item describes Windows.
   * Discovered from `.item` manifests in `C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests`.
   * Only entries that are applications in a games category are listed. DLC, add-ons, and Unreal Engine installs are skipped.
   * Launched via `com.epicgames.launcher://apps/<AppName>?action=launch&silent=true`. If a title fails with the bare `AppName`, the fallback is the `<CatalogNamespace>:<CatalogItemId>:<AppName>` form; both IDs are in the manifest.
3. **Standalone Executables**
   * Windows: `.exe` files. Linux: any file with the executable bit, including native binaries, AppImages, and shell scripts. Running a Windows `.exe` on Linux is not handled directly; the user points Moonglass at a wrapper script that calls Wine or Proton.
   * Added through Moonglass's own controller-navigable file browser, or by drag-and-drop when a mouse is present.
   * Launched directly as a child process. The working directory defaults to the executable's folder and can be overridden per game.
   * Optional launch arguments.
4. **Emulated ROMs**
   * The user registers ROM directories. Each directory is bound to one emulator profile and one platform label (for example "GameCube"). This removes ambiguity when several emulators accept the same extension, such as `.iso`.
   * Emulator profiles hold an executable path, an argument template, and accepted extensions. Presets ship for RetroArch, Dolphin, PCSX2, and RPCS3; any emulator with a command-line interface can be added. On Linux an emulator installed as a Flatpak is supported by setting the executable to `flatpak` and starting the template with `run <app-id>`; presets detect this automatically.
   * Scanning rules: one entry per `.m3u` playlist when present (its discs are hidden); `.cue` hides its `.bin` files; folder-based games are matched by a marker file defined in the profile (for example `EBOOT.BIN` for RPCS3).

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
  * Left: filter pills `All`, `Recent`, `Favorites`, `Steam`, `Epic`, `Apps`, `Emulators`, switched with `LB / RB`. Pills for sources with no games are hidden.
  * Right: the search box, controller connection and battery, clock, and a settings gear (clickable). The battery comes from `gilrs` where the OS reports it there, and from XInput on Windows (four levels, or "wired").

* **Hero Carousel**
  * A single horizontal row of **2:3 box-art cards** (or the user-selected aspect ratio).
  * **Default view and ordering:** the `All` pill, sorted by `last_played` descending, then `date_added` descending, then title. Recently played games are therefore first, and the screen is never empty on first run. `Recent` shows games played within a time window: the last 30 days by default, adjustable in Settings to 7, 30, or 90 days. The pill is hidden when nothing falls inside the window.
  * **Active card:** clearly the focus of the screen: about half the screen height, roughly `1.45x` the size of the other cards. It gains a crisp border and the glow, and shows the game's transparent logo (or its title in text) with a small source badge in the hero zone above it.
  * **Inactive cards:** smaller and slightly darkened, so attention goes to the active card while every other game stays recognizable. Covers are not blurred. Size and darkening transition smoothly as focus moves.
  * **Corners:** every card, its cover image included, has rounded corners; the image is clipped to the card's radius (inside the border on the active card) so no corner shows past the outline.
  * **Empty state:** if no games exist at all, the carousel is replaced by a single "Add your games" card that opens setup.

* **Ambient Background & Lighting**
  * Behind the carousel: a blurred version of the focused game's hero art, or a dark obsidian canvas.
  * A soft, slowly pulsing glow behind the active card, colored from the game's artwork, and a matching tint on the background gradient. Colors crossfade as focus moves.

* **Bottom Action Bar**
  * `[A] Play`, `[X] Search`, `[Y] Options`, `[LB/RB] Filter`, `[Menu] Settings`. Glyphs match the connected controller family where it can be detected.
  * The hints follow the last input device: controller glyphs after gamepad input, key names (`Enter`, `Esc`, `Tab`, …) after keyboard or mouse input. Every hint is also a clickable button.

* **Keyboard and Mouse**
  * Keyboard: arrow keys move, `Enter` plays, `Esc` goes back or clears the search, `Tab` / `Shift+Tab` (or `PgUp` / `PgDn`) switch pills, `Home` / `End` jump to the first or last game, typing searches, `F11` toggles full screen. In the restore overlay, `Enter` returns to the game and `Del` force-quits.
  * Mouse: clicking a card focuses it and clicking the focused card plays it; the wheel (or a touchpad swipe) moves one card per notch; pills, action-bar hints, the search clear button and overlay buttons are clickable. Hovered elements are highlighted.

* **Search**
  * The search box sits at the top right of the top bar. `[X]` focuses it and opens the on-screen keyboard as a sheet at the bottom of the screen; typing on a physical keyboard or clicking the box focuses it directly. A focused box is outlined and shows a blinking caret.
  * The carousel filters live as characters are entered. Matching is case-insensitive on any part of the title and ignores punctuation and accents.
  * Search covers every installed, non-hidden game regardless of the active filter pill. Results use the default ordering.
  * `[A]` on the keyboard's "Done" key (or `Enter`) moves focus to the results; `[B]` (or `Esc`, or the box's × button) clears the search and restores the previous pill and position.
  * No matches shows "No games match" with the query, not an empty row.

### Customization (in-app, no config files)

* **Carousel Geometry:** Vertical Posters (2:3), Wide Banners (16:9, using the hero art), or Square Tiles (1:1); card spacing (Compact, Normal, Wide); focused card size (Normal, Large, Extra large).
* **Ambient Mode:** Dynamic Color Bloom, Full-Screen Hero Fanart, or Pure OLED Dark; glow (Off, Subtle, Normal, Bright); background blur strength (chosen from pre-computed levels).
* **Card Metadata:** Art Only, Art + Title, or Art + Playtime & Source Badge, shown under each card. Favorites carry a star and hidden games (when shown) a "Hidden" tag.
* **Startup Animation:** on by default; the wordmark, then the cards rise in one after another (about 1.5 s). Off skips it.
* **Reduced Motion:** disables the glow pulse, the startup animation and every transition (highlights, panels, the on-screen keyboard, list changes).
* **Clock:** 24-hour or 12-hour.

All of these live in Settings → Appearance, apply immediately, and are saved.

### First-Run Flow

1. Scan Steam and Epic automatically and show the count found.
2. Offer to add ROM directories and emulators (skippable).
3. Offer to enter a SteamGridDB API key (skippable; see 5.2).
4. Land on the carousel. Artwork fills in as it downloads.

## 4. Technical Architecture (Rust + GPUI)

Moonglass is one self-contained Rust binary with no WebView or JavaScript runtime. The whole UI is rendered on the GPU through **GPUI**, Zed's UI framework.

### 4.1 Rendering Constraints That Shape the Design

GPUI renders through DirectX 11 on Windows and through `wgpu` (Vulkan, with an OpenGL fallback) on Linux, where it supports both X11 and Wayland. Moonglass uses only GPUI's cross-platform primitives, so the look is identical on both. Upstream GPUI provides solid fills, two-stop linear gradients, blurred box shadows, images, and paths. It does not provide radial gradients, image blur filters, or spring physics. The visual design is built from what exists:

| Effect | Implementation |
| --- | --- |
| Glow behind active card | Two or three stacked box shadows with large blur radii in the game's accent and dominant colors; the pulse animates their opacity and spread |
| Blurred hero background | Blurred **at ingestion** on the CPU (downscale, Gaussian blur, save as a small texture), then stretched by the GPU. Three strengths are pre-computed |
| Background tint | Two-stop linear gradient from the dominant color to near-black |
| Color and art crossfade | Two stacked layers with animated opacity |
| Blurred backdrop (Settings, launching, Now Playing) | The strongest pre-blurred hero of the focused game, tinted with its color, with no cards drawn, so the whole screen is one even surface. The carousel crossfades into it |
| Interface motion | Highlights, panels, sheets, toggles and list changes glide on small critically-damped springs keyed by what is drawn; a value that is no longer drawn is forgotten |
| Card motion | A small critically-damped spring implemented in Moonglass and stepped once per frame |

GPUI is pre-1.0 and its API changes between releases. The dependency is pinned to an exact revision and upgraded deliberately.

### 4.2 Technology Stack

* **UI:** `gpui`, pinned.
* **Async:** `tokio` for I/O (network, database, process monitoring), connected to GPUI through the same bridge pattern Zed uses (`gpui_tokio`: a small tokio runtime whose tasks are returned to GPUI as GPUI tasks). UI state is touched only on GPUI's foreground executor.
* **Persistence:** `sqlx` with SQLite, compile-time checked queries in offline mode (`cargo sqlx prepare`) so builds need no live database, and versioned migrations.
* **Gamepad (foreground):** `gilrs` on a dedicated thread, using its default backend on each platform, for broad controller support while Moonglass has focus.
* **Gamepad (background hotkey):**
  * Windows: a separate minimal XInput poller via `windows-rs`. This exists because the default `gilrs` backend there (Windows Gaming Input) delivers input only to a focused window, and the restore hotkey is needed precisely when Moonglass is not focused.
  * Linux: the same `gilrs` thread. Its Linux backend reads controllers through evdev, which does not depend on window focus, so no second poller is needed.
* **OS integration:** `sysinfo` for process enumeration on both platforms. Windows: `windows` (`windows-rs`) for `ShellExecuteW`, window focus, Job Objects, registry, and global hotkeys. Linux: `rustix` or `nix` for process groups and signals, `/proc` for process environment, and `xdg-open` for URIs.
* **Paths:** the `directories` crate, so each platform's conventions are followed.
* **Images and color:** `image` for decoding, resizing, and blurring; a k-means palette extractor (the `kmeans_colors` crate or equivalent) for dominant and accent colors.
* **Parsing and scanning:** `keyvalues-parser` (Steam VDF/ACF), `serde_json` (Epic), `walkdir` and `notify` (directories).
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
| Start at login | `HKCU\...\Run` registry value | `.desktop` file in `~/.config/autostart/` |
| Single-instance guard | Named mutex | Lock file plus a Unix socket in the runtime directory |
| Steam and Epic discovery | Registry and `ProgramData` | Known directories, including Flatpak and Snap locations |

### 4.4 File Locations

| | Windows | Linux |
| --- | --- | --- |
| Database and settings | `%APPDATA%\Moonglass\` | `~/.config/moonglass/` and `~/.local/share/moonglass/` |
| Artwork and derived textures | `%LOCALAPPDATA%\Moonglass\cache\` | `~/.cache/moonglass/` |
| Logs | `%LOCALAPPDATA%\Moonglass\logs\` | `~/.local/state/moonglass/` |

The Linux paths honor the XDG environment variables when they are set.

Moonglass is single-instance. A second launch brings the existing window forward and exits.

### 4.5 Distribution

* Windows: a single `.exe`, plus a zip.
* Linux: a tarball and an AppImage. Distribution packages and a Flatpak are out of scope for v1.0 (see Non-Goals).

## 5. Core Subsystems

### 5.1 Library Scanner

**When scans run**

* At startup.
* When Moonglass resumes after a game exits.
* On file-system events (debounced by 2 seconds) from watchers on every Steam `steamapps` folder, the Epic manifests folder, and every ROM directory.
* On demand from Settings.

**Reconciliation rules**

* Every game has a deterministic ID (section 6), so a rescan updates rows in place.
* A game that disappears is marked `is_installed = 0` and hidden. It is not deleted, so favorites, playtime, and custom artwork survive a reinstall.
* `last_played` for Steam games is seeded from the manifest's `LastPlayed` value when Moonglass has no value of its own, so the first-run order matches the user's real history.

### 5.2 Artwork Pipeline

Sources are tried in this order. The first hit wins per asset type (cover, hero, logo).

1. **User override.** Chosen through `[Y] → Change Artwork`. Overrides are flagged and never replaced by a rescan.
2. **Steam's local library cache** (Steam games only). Steam already stores cover, hero, and logo images on disk; using them needs no network and no API key.
3. **SteamGridDB**, queried by Steam AppID for Steam games and by title search for everything else. Requires the user's own free API key, entered in Settings. Without a key this step is skipped.
4. **Generated placeholder.** A title card with the game's name on a gradient derived from a hash of the title.

**On ingestion, once per asset:**

* Resize the cover to the largest size the carousel can display at the current resolution.
* Extract dominant and accent colors. Both are clamped to a minimum saturation and a lightness band so that dark or grey covers still produce a visible glow.
* Produce the three blurred hero variants.
* Store paths and colors in SQLite.

**At runtime:** only visible cards plus a few neighbors on each side are decoded and uploaded as GPU textures; others are released. Decoding happens off the UI thread.

v1.0 uses static images only.

### 5.3 Launch & Focus Lifecycle

Each launch moves through an explicit state machine:

```
Idle → Launching → Running → Exiting → Idle
            ↓ (timeout or cancel)        ↑
            └────────────────────────────┘
```

**Launching**

* Show the "Launching *Game*…" overlay and stop all animation.
* Start the game (below) and wait for evidence that it is running.
* After 20 seconds the overlay adds "Still waiting… `[B]` Cancel". After 3 minutes (configurable) Moonglass gives up and returns to Idle. A long wait is normal when Steam updates a game first.

**Detecting start and exit**

| Source | Windows | Linux |
| --- | --- | --- |
| Executable, ROM | The spawned child, placed in a **Job Object** so its children are tracked and can be terminated together | The spawned child, started in its own **process group** for the same purpose |
| Steam | Any process whose executable lives under the game's install folder | Any process whose environment contains `SteamAppId=<AppID>`. Steam sets this on the game's own processes, native or Proton, which avoids the problem that a Proton game's executable is Wine rather than a file in the install folder |
| Epic | Same as Steam on Windows | Any process whose executable, working directory, or command line points inside the install folder (the game runs under Wine, started by Heroic) |

Polling is once per second with `sysinfo` on both platforms.

Rules that make the folder signal reliable:

* **Ignore list.** Helper processes such as crash handlers and redistributable installers are not counted as the game. The list is built in and extendable.
* **Exit grace period.** When the last matching process ends, Moonglass waits 5 seconds. If another matching process appears (a launcher handing off to the real game), the state returns to Running.
* **Per-game override.** Options allow naming the exact process to watch, for games whose real executable lives outside their install folder.
* **Launcher-style executables.** A per-game toggle switches a standalone executable from child-process tracking to folder tracking, for stubs that start the real game and exit.
* **Folders too broad to be a game.** A drive root, the home, Desktop, Documents or Downloads folder, `Program Files`, the application-data folders, and anything inside the Windows folder (on Linux: `/usr/bin`, `/opt` and similar) hold unrelated programs. Such a folder never matches a process, so Stop and Force stop cannot reach anything but the game; a launcher-style executable in one falls back to child-process tracking.
* **Steam registry signal (Windows, to validate in Phase 0).** Steam appears to record per-app running state in the registry under `HKCU\Software\Valve\Steam\Apps\<AppID>`. If the spike confirms it is dependable, it becomes the primary Steam signal on Windows and the folder watch becomes the fallback. Steam keeps a `Running` value (0/1) per AppID there. To gather evidence, Moonglass logs that value next to its own process detection whenever it changes during a Steam session, and `moonglass steam-signal <game>` prints both once per second while the player starts and quits the game.

**Running**

* Windows: the window is minimized. Linux: the window stays fullscreen underneath the game. Wayland gives an application no way to un-minimize or raise itself, so Moonglass never minimizes there; when the game's window closes, the compositor returns focus to the fullscreen window beneath it.
* On both platforms Moonglass renders zero frames in this state.
* **Now Playing screen.** Whenever Moonglass is visible while a game runs (after Alt-Tab, the restore hotkey, or on Linux when it is on top), it shows "Now playing *Game*" over the blurred backdrop (4.1), how long the game has been running, and two buttons: **Return to game** (`[A]`/`Enter`) and **Stop game** (`[X]`/`Del`). The screen disappears as soon as the game exits, whether the player quit it or it was stopped.
* **Stop game.** The first press asks the game to close normally (a window close request on Windows, `SIGTERM` on Linux) so it can save. If it is still running after 8 seconds, the button becomes **Force stop**, which terminates it (the Job Object or process group for executables and ROMs; the detected processes for Steam and Epic games). Moonglass never stops a game without the player asking.
* Polling drops to once per second. Gamepad navigation is suspended. The only input still observed is the restore hotkey (5.4).

**Exiting → Idle**

1. **Regain focus, Windows.** Restore the window (`ShowWindow(SW_RESTORE)`), then `SetForegroundWindow`. Confirm with `GetForegroundWindow`. If focus was not obtained, retry with the `AttachThreadInput` technique, then with a synthetic `Alt` key event, up to three attempts in total.
2. **Regain focus, Linux.** Check whether the window is active. If it is not, request activation through GPUI (an `_NET_ACTIVE_WINDOW` request on X11; an activation request on Wayland, which the compositor may decline).
3. If focus still fails on either platform, raise an attention request (taskbar flash or the desktop's "demands attention" hint), show a desktop notification saying how to return, and log the failure. Never loop indefinitely.
4. **Dismiss the launcher window.** If the Steam, Epic, or Heroic window is in front after a game from that client exits, Moonglass asks the client to hide it. On Windows this is a normal window-close request, which sends the client to the system tray. On Linux, Moonglass cannot close another application's window on Wayland, so it avoids the window appearing in the first place: Heroic is always started with `--no-gui`, and Steam is started minimized to the tray if Moonglass is the one starting it. On X11 a close request is also sent. In all cases the client process keeps running so cloud saves finish syncing, and dialogs such as a cloud-save conflict prompt are left alone. On by default; can be turned off in Settings → System.
5. Set `last_played`, add the session length to `playtime_seconds`, rescan, and re-render.

Playtime counts from the Running state to exit, so update and launcher time is excluded.

### 5.4 Gamepad Navigation and the Restore Hotkey

**Foreground navigation (`gilrs`)**

* A dedicated thread blocks on controller events and sends typed actions (`NavLeft`, `NavRight`, `NavUp`, `NavDown`, `Confirm`, `Back`, `Options`, `Search`, `FilterPrev`, `FilterNext`, `OpenSettings`) to GPUI through a channel.
* The left stick uses a radial deadzone. Holding a direction repeats with acceleration.
* Dispatch is suspended whenever the state is not Idle, and whenever Moonglass's window is not focused.

**Restore hotkey**

* Active only in the Launching and Running states.
* Default: hold `View + Menu` (Back + Start) for 2 seconds. The hold prevents collisions with games and emulators that use the same pair as a tap. Configurable.
  * Xbox: View (two overlapping rectangles, left of the Xbox button) + Menu (three lines, right of it).
  * PlayStation: Share (DualShock 4) or Create (DualSense) + Options.
  * Nintendo Switch Pro: − (Minus) + + (Plus).
  * Settings → System shows the names for the connected controller.
* The `Guide` button is not offered: the public XInput API does not report it, and Windows Game Bar and Steam claim it on both platforms.
* **Windows:** read by the XInput poller, about ten times per second. Keyboard equivalent, registered as a global hotkey: `Ctrl + Alt + Home`. This also covers controllers that XInput cannot see.
* **Linux:** read by the `gilrs` thread, which keeps receiving events in the background and works with any controller the kernel recognizes. The keyboard equivalent is registered directly on X11 and through the desktop's global-shortcuts portal on Wayland; where the desktop offers neither, only the gamepad combination is available and Settings says so.
* Action: bring Moonglass forward on the Now Playing screen (5.3), with **Return to game** and **Stop game**. On Wayland, if the compositor declines to bring Moonglass forward, the desktop notification from 5.3 is shown instead.

### 5.5 Settings, File Browser, and Text Entry

* **Settings** is a full-screen view of glass panels over the blurred backdrop (4.1), opened with `[Menu]`, `Ctrl+,` or the gear, and navigable by gamepad, keyboard and mouse. It opens with the focus on the list of sections; right or `[A]`/`Enter` moves into the chosen one. Sections: Appearance (section 3, Customization), Library (add a game, ROM directories and emulators, `Recent` window, show hidden games, rescan), Artwork (SteamGridDB key, refresh all artwork), System (start at login, send the launcher to the tray after a game, the restore hotkey for the connected controller, logs, version, Exit Moonglass). `Ctrl+Q` also exits when no game is running. `LB/RB` or `Tab` switch sections, left/right change values, `[A]`/`Enter` selects, `[B]`/`Esc` goes back.
* **File browser:** a built-in list view of folders, used wherever a path is needed. It starts from the drive list on Windows and from the home folder plus mounted volumes on Linux. Filters by extension when picking a file, or by the executable bit when adding a Linux executable.
* **On-screen keyboard:** a bottom sheet used for library search and the few text fields (API key, launch arguments, process to watch). Gamepad: `[A]` types, `[X]` deletes, `[Y]` space, `[Menu]` done, `[B]` cancel. Keys are clickable, and a physical keyboard types into the same fields (`Enter` done, `Esc` cancel). In the file browser, typing a letter jumps to the next entry starting with it.
* **Per-game options (`[Y]`, `Ctrl+O`, or right-click on a card):** Play, Favorite, Hide, Change Artwork (and back to automatic artwork once changed), Launch Arguments and Remove (standalone executables and ROM entries), Working Directory and the launcher-style toggle (standalone executables), Process to Watch. Remove asks for a second press.
* **Start at login:** a toggle. Windows: writes or removes a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Linux: writes or removes `~/.config/autostart/moonglass.desktop`.

## 6. Data Schema (SQLite)

IDs are deterministic so rescans update in place:

* Steam: `steam:<AppID>`
* Epic: `epic:<AppName>`
* Executable: `exe:<hash of normalized path>`
* ROM: `rom:<hash of normalized path>`

Path normalization resolves symlinks and relative segments on both platforms, and additionally lowercases on Windows, where paths are case-insensitive. The schema is identical on both platforms; `source = 'exe'` means "standalone executable" regardless of file type, and `source = 'epic'` covers Epic games whether they are launched by the Epic client or by Heroic.

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
    source            TEXT NOT NULL CHECK (source IN ('steam', 'epic', 'exe', 'rom')),
    source_id         TEXT,                    -- Steam AppID or Epic AppName; NULL for exe/rom
    launch_target     TEXT,                    -- .exe path or ROM path; NULL for steam/epic (URI is built from source_id)
    launch_args       TEXT,
    working_dir       TEXT,                    -- exe only; NULL = executable's folder
    rom_directory_id  TEXT REFERENCES rom_directories(id) ON DELETE CASCADE,
    platform          TEXT,                    -- copied from the ROM directory, for badges
    install_dir       TEXT,                    -- absolute path; watched for steam/epic
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
* `playtime_seconds` is Moonglass's own count and is labeled as such in the UI. Steam's playtime keeps accumulating inside Steam independently.
* The SteamGridDB API key is stored in `settings`. It is a low-sensitivity key for a free service; it is never logged.

## 7. Performance Targets

These are acceptance criteria for v1.0, measured on a mid-range gaming PC with a library of 500 games. They apply to Windows and to Linux (verified on one X11 session and one Wayland session).

| Measure | Target |
| --- | --- |
| Cold start to interactive carousel | Under 2 seconds |
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
  * UI sounds and ambient audio (`rodio`), animated covers, Steam playtime import, more launchers.

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
| Dialogs owned by Steam, Epic, or Windows | May need a mouse or keyboard | Documented limitation; the restore hotkey always returns to Moonglass |
| Dismissing a launcher window could close one the user wanted | Minor annoyance | Only the main client window is closed, never dialogs; setting to disable |
| Games that start their own launcher (EA, Ubisoft, Rockstar) | Real game process may sit outside the install folder | Per-game process override |
| On Windows the background hotkey works only for XInput controllers | Other pads cannot summon Moonglass mid-game | Global keyboard hotkey; note in Settings |
| SteamGridDB needs a per-user API key | Non-Steam games show placeholders without it | Steam local cache needs no key; first-run prompt; manual artwork override |
| Epic URI format may vary by title | Some games may not launch from the bare `AppName` | Fallback to the namespaced form |
