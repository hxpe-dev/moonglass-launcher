//! Starting a game and driving the launch state machine (spec 5.3).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use moonglass_core::lifecycle::{Effect, Lifecycle, Timing};
use moonglass_core::template::{split_args, RomVars, Template};
use moonglass_core::{Game, Source};
use moonglass_db::Db;
use moonglass_platform::{GameMatcher, Launcher, ProcessWatcher, TrackedChild};

/// How a started game is recognized while it runs.
pub struct Running {
    pub child: Option<TrackedChild>,
    pub matcher: Option<GameMatcher>,
    pub launcher: Option<Launcher>,
}

/// Builds the emulator command line for a ROM: the profile's executable,
/// the expanded template, then any per-game arguments.
pub fn rom_command(
    executable: &str,
    template: &str,
    rom: &Path,
    extra_args: Option<&str>,
) -> anyhow::Result<(PathBuf, Vec<String>)> {
    let t = Template::parse(template).context("emulator launch template")?;
    let rom_s = rom.to_string_lossy();
    let dir = rom
        .parent()
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    let name = rom
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let mut args = t.expand(&RomVars {
        rom: &rom_s,
        rom_dir: &dir,
        rom_name: &name,
    });
    if let Some(extra) = extra_args.filter(|s| !s.trim().is_empty()) {
        args.extend(split_args(extra).context("launch arguments")?);
    }
    Ok((PathBuf::from(executable), args))
}

/// Starts `game` and says how to watch it.
pub async fn start(db: &Db, game: &Game, epic_fallback: bool) -> anyhow::Result<Running> {
    let override_matcher = game
        .watch_process
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|n| GameMatcher::ProcessName(n.to_string()));

    match game.source {
        Source::Steam => {
            let app_id = game
                .source_id
                .as_deref()
                .ok_or_else(|| anyhow!("Steam game without AppID"))?;
            moonglass_platform::launch_steam(app_id)?;
            Ok(Running {
                child: None,
                matcher: Some(override_matcher.unwrap_or(GameMatcher::Steam {
                    app_id: app_id.to_string(),
                    install_dir: PathBuf::from(game.install_dir.clone().unwrap_or_default()),
                })),
                launcher: Some(Launcher::Steam),
            })
        }
        Source::Epic => {
            let app_name = game
                .source_id
                .as_deref()
                .ok_or_else(|| anyhow!("Epic game without AppName"))?;
            let source = moonglass_platform::epic_source()
                .ok_or_else(|| anyhow!("Epic is not available"))?;
            let fallback = match (&source, epic_fallback) {
                (moonglass_platform::EpicSource::EpicManifests(dir), true) => {
                    moonglass_scan::epic::find_manifest(dir, app_name)?
                        .map(|m| m.fallback_launch_uri())
                }
                _ => None,
            };
            moonglass_platform::launch_epic(&source, app_name, fallback.as_deref())?;
            let install_dir = game
                .install_dir
                .clone()
                .ok_or_else(|| anyhow!("Epic game without install folder"))?;
            Ok(Running {
                child: None,
                matcher: Some(override_matcher.unwrap_or(GameMatcher::Folder {
                    install_dir: install_dir.into(),
                })),
                launcher: Some(Launcher::Epic),
            })
        }
        Source::Exe => {
            let target = PathBuf::from(
                game.launch_target
                    .clone()
                    .ok_or_else(|| anyhow!("no executable path"))?,
            );
            let args = match game.launch_args.as_deref() {
                Some(a) if !a.trim().is_empty() => split_args(a).context("launch arguments")?,
                _ => Vec::new(),
            };
            let cwd = game.working_dir.as_deref().map(Path::new);
            let child = TrackedChild::spawn(&target, &args, cwd)?;
            // Launcher-style stubs exit at once; watch their folder instead.
            let folder = game.track_by_folder.then(|| GameMatcher::Folder {
                install_dir: target.parent().unwrap_or(Path::new("/")).to_path_buf(),
            });
            Ok(Running {
                child: (!game.track_by_folder).then_some(child),
                matcher: override_matcher.or(folder),
                launcher: None,
            })
        }
        Source::Rom => {
            let dir_id = game
                .rom_directory_id
                .as_deref()
                .ok_or_else(|| anyhow!("ROM without directory"))?;
            let emu = db
                .emulator_for_rom_directory(dir_id)
                .await?
                .ok_or_else(|| anyhow!("ROM directory has no emulator"))?;
            let rom = PathBuf::from(
                game.launch_target
                    .clone()
                    .ok_or_else(|| anyhow!("ROM without path"))?,
            );
            let (exe, args) = rom_command(
                &emu.executable_path,
                &emu.launch_template,
                &rom,
                game.launch_args.as_deref(),
            )?;
            let child = TrackedChild::spawn(&exe, &args, None)?;
            Ok(Running {
                child: Some(child),
                matcher: override_matcher,
                launcher: None,
            })
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Debug-view driver: launch, poll once per second, report each
/// transition, then record the session. Returns the session length if the
/// game ran.
pub async fn run_to_completion(
    db: &Db,
    game: &Game,
    timing: Timing,
    epic_fallback: bool,
    dismiss_launcher: bool,
) -> anyhow::Result<Option<Duration>> {
    let origin = Instant::now();
    let mut lc = Lifecycle::new(timing);
    for e in lc.launch(origin.elapsed()) {
        report(game, e);
    }
    let mut running = match start(db, game, epic_fallback).await {
        Ok(r) => r,
        Err(e) => {
            lc.cancel();
            bail!("could not start {}: {e:#}", game.title);
        }
    };
    let mut watcher = ProcessWatcher::new(&[]);
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let child_alive = running.child.as_mut().is_some_and(TrackedChild::is_alive);
        let matched = match &running.matcher {
            Some(m) => {
                watcher.refresh();
                watcher.any_matching(m)
            }
            None => false,
        };
        for e in lc.observe(origin.elapsed(), child_alive || matched) {
            report(game, e);
            match e {
                Effect::LaunchAbandoned { .. } => return Ok(None),
                Effect::GameEnded { session } => {
                    if dismiss_launcher {
                        if let Some(l) = running.launcher {
                            moonglass_platform::window::dismiss_launcher_window(l);
                        }
                    }
                    db.record_session(&game.id, now_unix(), session.as_secs() as i64)
                        .await?;
                    return Ok(Some(session));
                }
                _ => {}
            }
        }
    }
}

fn report(game: &Game, e: Effect) {
    let msg = match e {
        Effect::ShowLaunching => format!("Launching {}…", game.title),
        Effect::ShowStillWaiting => "Still waiting… (Steam may be updating the game)".to_string(),
        Effect::EnterRunning => format!("{} is running", game.title),
        Effect::LaunchAbandoned { reason } => {
            format!("Gave up launching {} ({reason:?})", game.title)
        }
        Effect::GameEnded { session } => {
            format!("{} exited after {}s", game.title, session.as_secs())
        }
    };
    tracing::info!("{msg}");
    println!("{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_command_line() {
        let rom = Path::new("/roms/gc/Wind Waker.rvz");
        let (exe, args) = rom_command(
            "flatpak",
            r#"run org.DolphinEmu.dolphin-emu -b -e "{rom}""#,
            rom,
            Some("--batch 'x y'"),
        )
        .unwrap();
        assert_eq!(exe, PathBuf::from("flatpak"));
        assert_eq!(
            args,
            [
                "run",
                "org.DolphinEmu.dolphin-emu",
                "-b",
                "-e",
                "/roms/gc/Wind Waker.rvz",
                "--batch",
                "x y"
            ]
        );
    }
}
