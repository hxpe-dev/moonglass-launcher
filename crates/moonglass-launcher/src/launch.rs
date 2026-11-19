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

/// The game's own arguments (Options → Launch arguments), split like a
/// shell would. `what` names them in the error.
fn game_args(game: &Game, what: &'static str) -> anyhow::Result<Vec<String>> {
    match game.launch_args.as_deref() {
        Some(a) if !a.trim().is_empty() => split_args(a).context(what),
        _ => Ok(Vec::new()),
    }
}

/// A friend's Steam session to join instead of starting the game alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamJoin {
    pub lobby: String,
    pub friend: String,
}

/// Starts `game` (or joins `join`, for a Steam game) and says how to watch
/// it.
pub async fn start(
    db: &Db,
    game: &Game,
    epic_fallback: bool,
    join: Option<&SteamJoin>,
) -> anyhow::Result<Running> {
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
            let args = game_args(game, "launch options")?;
            match join {
                Some(j) => moonglass_platform::join_steam_lobby(app_id, &j.lobby, &j.friend)?,
                None => moonglass_platform::launch_steam(app_id, &args)?,
            }
            Ok(Running {
                child: None,
                matcher: Some(override_matcher.unwrap_or_else(|| GameMatcher::Steam {
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
                matcher: Some(override_matcher.unwrap_or_else(|| GameMatcher::Folder {
                    install_dir: install_dir.into(),
                })),
                launcher: Some(Launcher::Epic),
            })
        }
        Source::Gog => {
            let product_id = game
                .source_id
                .as_deref()
                .ok_or_else(|| anyhow!("GOG game without product ID"))?;
            let install_dir = PathBuf::from(
                game.install_dir
                    .clone()
                    .ok_or_else(|| anyhow!("GOG game without install folder"))?,
            );
            let source =
                moonglass_platform::gog_source().ok_or_else(|| anyhow!("GOG is not available"))?;
            let matcher = override_matcher.unwrap_or_else(|| GameMatcher::Folder {
                install_dir: install_dir.clone(),
            });
            let has_client = match &source {
                moonglass_platform::GogSource::Registry { galaxy, .. } => galaxy.is_some(),
                moonglass_platform::GogSource::Heroic { .. } => true,
            };
            if has_client {
                moonglass_platform::launch_gog(&source, product_id, &install_dir)?;
                return Ok(Running {
                    child: None,
                    matcher: Some(matcher),
                    launcher: Some(Launcher::Gog),
                });
            }
            // An offline-installer game without Galaxy: its own program,
            // with the per-game arguments, watched by its folder (GOG's
            // programs are often small launchers that start the game).
            let target = PathBuf::from(
                game.launch_target
                    .clone()
                    .ok_or_else(|| anyhow!("GOG game without a program to start"))?,
            );
            let args = game_args(game, "launch arguments")?;
            let child = TrackedChild::spawn(&target, &args, Some(&install_dir))?;
            Ok(Running {
                child: Some(child),
                matcher: Some(matcher),
                launcher: None,
            })
        }
        Source::BattleNet => {
            let uid = game
                .source_id
                .as_deref()
                .ok_or_else(|| anyhow!("Battle.net game without product UID"))?;
            let install_dir = PathBuf::from(
                game.install_dir
                    .clone()
                    .ok_or_else(|| anyhow!("Battle.net game without install folder"))?,
            );
            let matcher = override_matcher.unwrap_or_else(|| GameMatcher::Folder {
                install_dir: install_dir.clone(),
            });
            let client = moonglass_platform::battlenet_source().and_then(|s| s.client);
            if let (Some(client), Some(code)) =
                (&client, moonglass_scan::battlenet::launch_code(uid))
            {
                moonglass_platform::launch_battlenet(client, code)?;
                return Ok(Running {
                    child: None,
                    matcher: Some(matcher),
                    launcher: Some(Launcher::BattleNet),
                });
            }
            // A game Moonglass has no product code for: its own
            // `<Game> Launcher.exe`, which hands the start to Battle.net.
            let target = game.launch_target.clone().ok_or_else(|| {
                anyhow!(
                    "{} cannot be started from Moonglass yet; start it once from Battle.net",
                    game.title
                )
            })?;
            let child = TrackedChild::spawn(Path::new(&target), &[], Some(&install_dir))?;
            Ok(Running {
                child: Some(child),
                matcher: Some(matcher),
                launcher: client.map(|_| Launcher::BattleNet),
            })
        }
        Source::MsStore => {
            let aumid = game
                .source_id
                .as_deref()
                .ok_or_else(|| anyhow!("Store game without application ID"))?;
            let install_dir = game
                .install_dir
                .clone()
                .ok_or_else(|| anyhow!("Store game without install folder"))?;
            moonglass_platform::launch_msstore(aumid)?;
            Ok(Running {
                child: None,
                matcher: Some(override_matcher.unwrap_or_else(|| GameMatcher::Folder {
                    install_dir: install_dir.into(),
                })),
                launcher: None,
            })
        }
        Source::Exe => {
            let target = PathBuf::from(
                game.launch_target
                    .clone()
                    .ok_or_else(|| anyhow!("no executable path"))?,
            );
            let args = game_args(game, "launch arguments")?;
            let cwd = game.working_dir.as_deref().map(Path::new);
            let child = TrackedChild::spawn(&target, &args, cwd)?;
            // Launcher-style stubs exit at once; watch their folder instead,
            // unless it is too broad to stand for one game (a drive root,
            // Downloads, the system folder): then the child is tracked.
            let folder = game
                .track_by_folder
                .then(|| target.parent().map(Path::to_path_buf))
                .flatten()
                .filter(|dir| {
                    let ok = moonglass_platform::is_trackable_folder(dir);
                    if !ok {
                        tracing::warn!(
                            folder = %dir.display(),
                            "folder holds more than this game; tracking the started process instead"
                        );
                    }
                    ok
                });
            Ok(Running {
                child: folder.is_none().then_some(child),
                matcher: override_matcher
                    .or_else(|| folder.map(|install_dir| GameMatcher::Folder { install_dir })),
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
    let mut running = match start(db, game, epic_fallback, None).await {
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
                    db.record_session(&game.id, crate::now_unix(), session.as_secs() as i64)
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
