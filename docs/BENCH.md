# Launcher benchmark

Moonglass compared with two other multi-store launchers installed on the
same machine: Playnite (in both its Desktop and Fullscreen modes) and One Game
Launcher. Measured on 2026-11-17.

## Setup

| | |
| --- | --- |
| OS | Windows 11 Pro 10.0.26200 |
| CPU | AMD Ryzen 7 3700X (8 cores / 16 threads) |
| GPU | NVIDIA GeForce RTX 4060 |
| RAM | 32 GB |
| Display | 2560×1440 at 280 Hz |

| Launcher | Version | Built with | Library |
| --- | --- | --- | --- |
| Moonglass | 0.1.0, release build (`target/release/moonglass.exe`, built 17 Nov 20:14, just before commit `528d734`) | Rust, GPUI | 34 entries |
| Playnite Desktop | 10.64 | .NET Framework, WPF, CEF | 33 games |
| Playnite Fullscreen | 10.64 (same install and library) | .NET Framework, WPF, CEF | 33 games |
| One Game Launcher | 2.3.0.0 (Microsoft Store) | WinUI, packaged app | 47 games |

The libraries are close in size but not the same: each launcher found the
games on its own.

## Method

A PowerShell script launched each app 5 times, from a state where none of the
launchers was running, and waited 5 s between runs. Each run:

1. **Window**: time from `Start-Process` to the first visible top-level
   window (at least 200×200) owned by the launcher's processes.
2. **Settled**: time until the launcher's processes together used less than
   5% of one core for a full second. This is a rough "done loading" signal
   that includes any startup animation.
3. **Idle**: from 20 s after launch, 20 s with nobody touching the app,
   window in front. CPU is the change in process CPU time; GPU is the
   `GPU Engine … engtype_3D` utilisation counter for the launcher's processes,
   sampled every second; memory is sampled at the end.
4. **Minimised**: every window of the launcher minimised, 5 s wait, then the
   same 20 s measurement.
5. Close: Playnite with `--shutdown`, the others with a close message. A
   launcher still running after 15 s was killed.

Every process in the launcher's tree counts: for Playnite that is the main app
plus its four `Playnite.BrowserProcess` (CEF) helpers. One Game also keeps a
background process, `OneGameLauncherService`, running when the app is closed.
It is measured on its own below and is **not** in the One Game rows.

The tables below give the median of the 5 runs. All per-run numbers are at the
end of this file.

## Results

### Startup

The startup animation is not counted as loading. Moonglass was measured with
it turned off (Settings → Appearance → Startup animation). Moonglass also
gets the same load timings with the animation on, because the library loads
behind it.

| Launcher | First window | Fully loaded | How "loaded" was measured |
| --- | --- | --- | --- |
| Moonglass | 348 ms | **0.69 s** (0.68–0.74 s) | Moonglass's own log: library read, first carousel frame (629 ms), store scan done and folders watched |
| Moonglass | 411 ms | 1.7 s (1.7–2.0 s) | CPU settled, same rule as the others (needs the glow off, see below) |
| Playnite Desktop | 221 ms | 3.6 s (3.1–3.9 s) | CPU settled |
| Playnite Fullscreen | 206 ms | 4.1 s (3.7–12.3 s) | CPU settled; one run took 12.3 s |
| One Game | 238 ms | 2.1 s (2.0–2.2 s) | CPU settled |

All four show a window within about a third of a second. For the other
launchers, "loaded" can only be measured from the outside: the first moment
their processes used less than 5% of one core for a full second. Moonglass
with the glow on never meets that rule reliably, because the glow pulse alone
keeps it at 3–4% of a core (1.4–6.2 s in the 5 runs). With the glow off it
settles at 1.7 s, still the fastest of the four. The CPU rule adds about 1 s
to Moonglass's real load time, so it probably also overstates the others'.

In Moonglass's 0.63 s to the first frame, about 0.49 s goes to creating the
Direct3D device inside GPUI. The library itself is read in 30–40 ms.

### Idle, window in front

| Launcher | CPU (% of one core) | GPU 3D | Working set | Private bytes | GPU memory | Processes / threads / handles |
| --- | --- | --- | --- | --- | --- | --- |
| Moonglass | **3.98%** | **2.8%** | **79 MB** | 165 MB | 67 MB | 1 / 66 / 640 |
| Moonglass, glow off | 0.44% | 0% | 76 MB | 158 MB | 62 MB | 1 / 65 / 627 |
| Playnite Desktop | 0.07% | 0% | 668 MB | 255 MB | 56 MB | 5 / 138 / 3,437 |
| Playnite Fullscreen | 0.81% | 0% | 649 MB | 261 MB | 106 MB | 5 / 148 / 3,505 |
| One Game | 0.37% | 0% | 172 MB | 122 MB | 33 MB | 1 / 81 / 2,265 |

### Idle, minimised

| Launcher | CPU (% of one core) | GPU 3D | Working set | Private bytes |
| --- | --- | --- | --- | --- |
| Moonglass | **1.99%** (1.0–3.2%) | 0% | 78 MB | 165 MB |
| Moonglass, glow off | 1.10% (0.4–2.4%) | 0% | 76 MB | 158 MB |
| Playnite Desktop | 0.00% | 0% | 652 MB | 253 MB |
| Playnite Fullscreen | 1.40% | 0% | 634 MB | 258 MB |
| One Game | 0.15% | 0% | 172 MB | 122 MB |

### Moonglass: glow pulse paused when unfocused

The glow pulse now stops whenever another window has the focus, and carries
on from where it was when Moonglass gets the focus back. The benchmark
gained a third phase for this: a small window of the script's own takes the
focus for 20 s while Moonglass stays visible behind it. Both builds ran with
the startup animation off and the glow on Normal.

| Build | Runs | In front: CPU / GPU | Unfocused: CPU / GPU | Minimised: CPU / GPU |
| --- | --- | --- | --- | --- |
| Before (`528d734`) | 3 | 3.52% / 2.2% | **4.42% / 2.2%** | 2.80% / 0% |
| After | 5 | 3.80% / 2.1% | **1.77% / 0%** (0.4–2.8%) | 1.62% / 0% (0–2.7%) |

In front nothing changes, which is expected. Unfocused, Moonglass no longer
draws: CPU falls by about 60% and GPU to zero. What is left is GPUI's vsync
thread, the same 1–3% seen minimised, which varies a lot from run to run.

### One Game's background process

`OneGameLauncherService` was already running before the benchmark and stays up
when the app is closed. Measured on its own over 30 s: 0.68% of one core,
64 MB working set, 33 MB private bytes, 13 threads, 964 handles.

### Disk

| Launcher | Install | User data and cache |
| --- | --- | --- |
| Moonglass | 27.6 MB (one exe; 10.4 MB zipped) | 0.4 MB database + 43.4 MB artwork cache |
| Playnite | 432.5 MB | 123.7 MB |
| One Game | 41.0 MB | 4.0 MB |

## What this shows

* **Memory: Moonglass has the smallest resident footprint.** At 79 MB working
  set it is less than half of One Game (172 MB) and about an eighth of
  Playnite (650–670 MB across its five processes). Its private bytes
  (165 MB) are higher than its working set; that is committed but mostly
  untouched memory, likely the GPU and allocator reservations. On private
  bytes One Game (122 MB) is lower than Moonglass, and Playnite is higher
  (255–261 MB).
* **Idle CPU and GPU: Moonglass is the most expensive, because of the glow
  pulse.** With default settings it is the only launcher that keeps drawing
  while nothing happens: about 4% of a core and 2–3% of the GPU in front,
  where the others are at or near zero. Almost all of that is the focused
  card's glow, which breathes by redrawing the whole screen 30 times a
  second. With the glow off, Moonglass idles at 0.44% and 0% GPU, close to
  One Game (0.37%) and below Playnite Fullscreen (0.81%). The pulse now
  pauses while Moonglass is not focused (see above), so this cost is only
  paid while you are looking at the carousel.
* **Minimised, GPUI's vsync thread is the cost.** Moonglass draws nothing
  (0% GPU) but still uses 1–2% of a core, more than any other launcher here.
  This is the GPUI vsync thread already noted in spec 9 and the README: it
  asks every window to redraw at the display's refresh rate, 280 times a
  second on this screen, even while the window is minimised.
* **Loading: Moonglass is the fastest.** Every launcher has a window up in
  0.2–0.35 s. Moonglass has its library on screen and up to date in about
  0.7 s by its own log, or 1.7 s by the outside CPU rule, against 2.1 s for
  One Game and 3.6–4.1 s for Playnite.
* **Disk: Moonglass is the smallest install** (one 28 MB exe against 41 MB
  for One Game and 433 MB for Playnite). Its artwork cache is larger than One
  Game's whole data folder.
* **Weight of the process tree:** Playnite runs five processes with about
  3,400 handles. Moonglass runs one process with 640 handles, the fewest here.

## Limits

* Five runs per launcher, on one machine, with libraries of 33–47 games, not
  the 500 games spec 7 targets.
* The disk cache was warm after the first run. A real cold boot was not
  measured.
* One Game's window did not take the foreground when the script started it
  through the shell (`foreground: false` in the raw data), so its numbers in
  front are for a window that was visible but not focused. If it slows down
  when unfocused, its foreground cost is understated.
* Playnite's working set is a sum over five processes and counts DLL pages
  they share more than once. Private bytes are the fairer number there.
* The GPU 3D counter showed 0% for Playnite Fullscreen even though it holds
  106 MB of GPU memory. WPF may render on another engine or only on change.
* One Game (every run, probably because closing sends it to the tray) and one
  Playnite Fullscreen run did not close within 15 s and were killed. That does not change the measurements, which were taken
  before closing.
* Scrolling, search and in-game behaviour were not compared. The Moonglass
  in-game numbers are in the README.

## Raw data

Times in ms, memory in MB, CPU in % of one core.

| Launcher | Run | Window | Settled | Idle CPU | Idle GPU | Idle WS | Idle private | Min. CPU | Min. WS | Close |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Moonglass | 1 | 411 | 5536 | 4.88 | 2.80 | 78.7 | 165.1 | 3.17 | 78.1 | closed |
| Moonglass | 2 | 329 | 5826 | 4.05 | 2.07 | 79.4 | 167.5 | 1.99 | 79.3 | closed |
| Moonglass | 3 | 348 | 2732 | 3.76 | 2.86 | 78.3 | 165.5 | 2.28 | 78.1 | closed |
| Moonglass | 4 | 390 | 3389 | 3.83 | 3.30 | 78.8 | 163.8 | 1.62 | 78.5 | closed |
| Moonglass | 5 | 316 | 2759 | 3.98 | 2.74 | 78.2 | 164.7 | 1.03 | 77.9 | closed |
| Playnite Desktop | 1 | 273 | 3623 | 0.00 | 0 | 667.7 | 258.4 | 0.00 | 652.6 | closed |
| Playnite Desktop | 2 | 206 | 3921 | 0.07 | 0 | 669.6 | 255.2 | 0.22 | 654.4 | closed |
| Playnite Desktop | 3 | 204 | 3188 | 0.22 | 0 | 666.5 | 252.0 | 0.07 | 651.5 | closed |
| Playnite Desktop | 4 | 221 | 3054 | 0.07 | 0 | 663.4 | 248.9 | 0.00 | 648.3 | closed |
| Playnite Desktop | 5 | 221 | 3710 | 0.07 | 0 | 669.4 | 254.9 | 0.00 | 654.5 | closed |
| Playnite Fullscreen | 1 | 327 | 5217 | 0.81 | 0 | 652.8 | 259.4 | 1.54 | 637.6 | closed |
| Playnite Fullscreen | 2 | 225 | 4105 | 0.15 | 0 | 647.2 | 265.6 | 0.07 | 632.5 | closed |
| Playnite Fullscreen | 3 | 206 | 4117 | 2.65 | 0 | 648.0 | 264.0 | 0.29 | 632.9 | closed |
| Playnite Fullscreen | 4 | 206 | 12339 | 1.25 | 0 | 649.0 | 258.4 | 2.06 | 634.1 | killed |
| Playnite Fullscreen | 5 | 174 | 3650 | 0.66 | 0 | 648.7 | 260.8 | 1.40 | 633.2 | closed |
| One Game | 1 | 314 | 2198 | 0.44 | 0 | 171.7 | 121.5 | 0.22 | 171.6 | killed |
| One Game | 2 | 190 | 2163 | 0.37 | 0 | 173.2 | 122.9 | 0.15 | 172.6 | killed |
| One Game | 3 | 238 | 2126 | 0.22 | 0 | 173.6 | 124.5 | 0.30 | 173.4 | killed |
| One Game | 4 | 238 | 2032 | 0.44 | 0 | 171.8 | 122.2 | 0.15 | 171.4 | killed |
| One Game | 5 | 236 | 2115 | 0.15 | 0 | 168.3 | 118.9 | 0.07 | 168.1 | killed |

Moonglass with the startup animation off. "First frame" and "Loaded" come
from Moonglass's log (the time since the process started when it drew the
first carousel frame and when it finished the store scan and started
watching folders).

| Glow | Run | Window | Settled | First frame | Loaded | Idle CPU | Idle GPU | Idle WS | Idle private | Min. CPU |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Normal | 1 | 466 | 3559 | 643 | 739 | 2.94 | 2.20 | 75.9 | 157.5 | 0.88 |
| Normal | 2 | 353 | 1915 | 632 | 692 | 3.17 | 2.89 | 76.1 | 159.4 | 2.06 |
| Normal | 3 | 348 | 6153 | 629 | 688 | 3.61 | 2.96 | 77.1 | 160.5 | 3.24 |
| Normal | 4 | 332 | 1396 | 622 | 679 | 3.69 | 2.89 | 77.3 | 157.7 | 3.09 |
| Normal | 5 | 345 | 2439 | 626 | 687 | 4.12 | 3.16 | 76.8 | 156.9 | 2.14 |
| Off | 1 | 415 | 1684 | 619 | 678 | 2.30 | 0 | 75.6 | 158.3 | 2.36 |
| Off | 2 | 347 | 2008 | 650 | 708 | 0.22 | 0 | 75.8 | 157.9 | 0.37 |
| Off | 3 | 411 | 1689 | 622 | 679 | 0.44 | 0.06 | 76.2 | 158.9 | 1.10 |

With the animation on (the first 5 runs above), the log gives first frames of
616–647 ms and loaded at 673–706 ms, the same as with it off.
