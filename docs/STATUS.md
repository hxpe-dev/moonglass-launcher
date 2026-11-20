# Status

Where each part of Moonglass stands, feature by feature: what is done, what
has been tried on real hardware, and what is known to be missing. The
design these refer to is in [SPEC.md](SPEC.md).

All three phases are implemented. Everything runs on Windows; the Linux code
compiles but has not been run yet. Running `moonglass` with no arguments
opens the carousel (or first-time setup on an empty library).

| Area | State |
| --- | --- |
| Database, logs, single instance, scanners, file watchers | Done |
| Launch, exit detection, focus return, Now Playing with Stop / Force stop | Done on Windows |
| Notification when Moonglass cannot come back to the front | Done; Linux untested |
| Restore hotkey (controller hold, `Ctrl+Alt+Home`), with its buttons and hold time in Settings | Done; the controller hold is untested |
| Artwork: Steam's cache, Epic's catalog (Windows), GOG's catalog, SteamGridDB, Switch and libretro art for emulated games, placeholder | Done |
| Carousel, ambient lighting (including the motion blur background), card art fitting, startup animation, interface animations | Done |
| Gamepad, keyboard and mouse, with hints that follow the device; hold Esc / B to exit | Done |
| Search and on-screen keyboard | Done |
| Settings: Appearance, Library, Stores, Emulators, Artwork, Sound, System | Done |
| Interface sounds and background music from a folder of your own, each with its own volume; the home screen shows the song with previous, pause and next (also View, LT and RT on a controller) | Done |
| API keys pasted with Ctrl+V or the keyboard's Paste key; a key already copied is filled in | Done |
| First-time setup | Done |
| File browser, Add a game (also by dropping a program on the window), per-game Options | Done; dropping is untested |
| Emulator profiles, presets (Switch included, Flatpaks found on Linux) and ROM folders | Done; Flatpak detection untested |
| Launch options for Steam games | Done; not yet tried with a game |
| Steam friends in your games, Join, borrowed-game warning (5.6) | Done; not yet tried with a key |
| Steam account switcher, and the library limited to what the account can play (5.7) | Done on Windows; Linux untested |
| Steam folder and "not a game" Steam apps in Settings | Done |
| GOG games: GOG Galaxy and offline installers (Windows), Heroic (Linux), keyless artwork | Done; not yet tried with a real GOG install on either platform |
| Battle.net and Microsoft Store / Xbox app games (Windows): found, started through Battle.net or Windows, watched by their folder | Done; found on a real install, not yet started from Moonglass |
| Steam's own playtime for the signed-in account, shown with Moonglass's (5.1) | Done on Windows |
| Now Playing shows the time, controller battery and total playtime | Done |
| Quick Menu on Now Playing: save a clip or screenshot through Xbox Game Bar, NVIDIA or AMD (Windows) or gpu-screen-recorder (Linux), the game's own volume, Stop (5.8) | Done on Windows; not yet tried with a game, Linux untested |
| Turning any store off, in Settings → Stores and in first-time setup | Done |
| Display: fullscreen with a resolution (Windows), borderless window, or a window | Done; resolution change not yet tried on a second display |
| Library database checked on every start, backed up, repaired from the backup when damaged, closed cleanly; SQLite 3.51.3 (spec 6) | Done on Windows |
| Releasing images that are no longer drawn | Done |
| Packaging: Windows `.exe` and zip, Linux tarball and AppImage, release workflow | Done; the Linux packaging and the workflow have not run yet |

## Performance against the targets (spec 7)

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

## Not done yet

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
* GOG with a real install: Galaxy's `/command=runGame` launch, the Galaxy
  window title used to send it to the tray ("GOG Galaxy"), and Heroic's GOG
  files. GOG games installed while Moonglass runs on Windows appear after the
  next game or a rescan (the registry is not watched).
* Epic, GOG, Battle.net and Microsoft Store playtime is not imported: they
  keep it on their servers only.
* Battle.net games start from a table of Battle.net's product codes; a game
  outside it starts through its own `<Game> Launcher.exe`. Starting a game
  through Battle.net and Battle.net's window title ("Battle.net"), used to
  send it to the tray, have not been tried yet.
* Only Store games built with Microsoft's game kit (`MicrosoftGame.config`)
  are found; older UWP games are not. Store and Battle.net games have no
  keyless artwork: they use SteamGridDB (with a key) or the placeholder.
