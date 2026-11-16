//! Moonglass, Phase 1 binary: the core without UI.
//!
//! This is the "plain debug list view" from the roadmap: it scans the
//! library, lists it in carousel order, and launches games through the real
//! lifecycle so detection, grace periods and timeouts can be exercised
//! before the GPUI carousel exists.

mod artwork;
mod launch;
mod library;
mod ui;
mod watch;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use moonglass_core::library::{carousel, search, visible_filters, Filter, RecentWindow};
use moonglass_core::lifecycle::Timing;
use moonglass_core::{Game, GameId};
use moonglass_db::{Db, Emulator};
use moonglass_platform::{AppPaths, InstanceGuard, InstanceStatus};

const USAGE: &str = "\
Moonglass

USAGE:
    moonglass                            open the carousel (MOONGLASS_WINDOWED=1 for a window)
    moonglass list [--filter <pill>] [--search <text>]
    moonglass scan
    moonglass launch <id|number> [--epic-fallback] [--give-up <secs>]
    moonglass add-exe <path> [title]
    moonglass add-emulator <id> <name> <executable> <template> <extensions> [folder-marker]
    moonglass add-rom-dir <path> <emulator-id> <platform> [--flat]
    moonglass remove <id>                (standalone executables and ROM entries)
    moonglass favorite <id> on|off
    moonglass hide <id> on|off
    moonglass options <id> [--args <a>] [--cwd <dir>] [--watch <process>] [--track-folder]
    moonglass artwork [--force]
    moonglass set-art <id> <image-file>
    moonglass unlock-art <id>
    moonglass sgdb-key <key>|clear
    moonglass autostart on|off|status
    moonglass paths

Pills: all, recent, favorites, steam, epic, apps, emulators.
<number> is the position shown by `moonglass list`.";

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn init_logging(paths: &AppPaths) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::EnvFilter;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("moonglass")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&paths.log_dir)
        .context("creating log file")?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("MOONGLASS_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Ok(guard)
}

fn parse_filter(s: &str) -> anyhow::Result<Filter> {
    Filter::ORDER
        .into_iter()
        .find(|f| f.label().eq_ignore_ascii_case(s))
        .ok_or_else(|| anyhow!("unknown filter {s:?}"))
}

fn on_off(s: Option<&String>) -> anyhow::Result<bool> {
    match s.map(String::as_str) {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => bail!("expected on or off"),
    }
}

/// Value following `--flag` in `args`.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

async fn recent_window(db: &Db) -> anyhow::Result<RecentWindow> {
    Ok(db.setting("recent_window").await?.unwrap_or_default())
}

/// Resolves a game by ID or by its number in the default `list` order.
async fn resolve(db: &Db, key: &str) -> anyhow::Result<Game> {
    let games = db.games().await?;
    if let Ok(n) = key.parse::<usize>() {
        let order = carousel(&games, Filter::All, now_unix(), recent_window(db).await?);
        let i = order
            .get(
                n.checked_sub(1)
                    .ok_or_else(|| anyhow!("numbers start at 1"))?,
            )
            .ok_or_else(|| anyhow!("no game number {n}"))?;
        return Ok(games[*i].clone());
    }
    games
        .into_iter()
        .find(|g| g.id.as_str() == key)
        .ok_or_else(|| anyhow!("no game with id {key}"))
}

async fn print_list(db: &Db, filter: Filter, query: Option<&str>) -> anyhow::Result<()> {
    let games = db.games().await?;
    let now = now_unix();
    let recent = recent_window(db).await?;
    let pills: Vec<_> = visible_filters(&games, now, recent)
        .iter()
        .map(|f| f.label())
        .collect();
    println!("Pills: {}", pills.join("  "));

    let order = match query {
        Some(q) => search(&games, q),
        None => carousel(&games, filter, now, recent),
    };
    if games.iter().all(|g| !g.is_visible()) {
        println!(
            "\n[ Add your games ]  (moonglass add-exe / add-rom-dir, or install Steam/Epic games)"
        );
        return Ok(());
    }
    if order.is_empty() {
        match query {
            Some(q) => println!("\nNo games match \"{q}\""),
            None => println!("\nNothing under {}", filter.label()),
        }
        return Ok(());
    }
    println!();
    for (n, &i) in order.iter().enumerate() {
        let g = &games[i];
        let fav = if g.is_favorite { "★" } else { " " };
        let played = g.playtime_seconds;
        let badge = g.platform.as_deref().unwrap_or(g.source.as_str());
        println!(
            "{:>3}. {fav} {:<48} {:<12} {:>6}m  {}",
            n + 1,
            g.title,
            badge,
            played / 60,
            g.id
        );
    }
    Ok(())
}

async fn run(args: Vec<String>) -> anyhow::Result<()> {
    let paths = AppPaths::discover()?;
    paths.create_all()?;
    let _log = init_logging(&paths)?;
    tracing::info!(
        platform = moonglass_platform::PLATFORM_NAME,
        version = env!("CARGO_PKG_VERSION"),
        "starting"
    );

    let cmd = args.first().map(String::as_str).unwrap_or("list");
    if matches!(cmd, "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(());
    }
    if cmd == "paths" {
        println!(
            "database: {}\ncache:    {}\nlogs:     {}",
            paths.database().display(),
            paths.cache_dir.display(),
            paths.log_dir.display()
        );
        return Ok(());
    }

    // Single instance: a launch while a game is being tracked would race it.
    let _instance = match InstanceGuard::acquire()? {
        InstanceStatus::Primary(g) => g,
        InstanceStatus::AlreadyRunning => {
            println!("Moonglass is already running; asked it to come forward.");
            return Ok(());
        }
    };

    let db = Db::open(&paths.database())
        .await
        .context("opening database")?;
    let rest = &args[args.len().min(1)..];

    match cmd {
        "list" => {
            library::scan_all(&db, now_unix()).await?;
            let filter = flag_value(rest, "--filter")
                .map(parse_filter)
                .transpose()?
                .unwrap_or(Filter::All);
            print_list(&db, filter, flag_value(rest, "--search")).await?;
        }
        "scan" => {
            for line in library::scan_all(&db, now_unix()).await?.lines {
                println!("{line}");
            }
        }
        "launch" => {
            let key = rest
                .first()
                .ok_or_else(|| anyhow!("launch needs a game id or number"))?;
            let game = resolve(&db, key).await?;
            let mut timing = Timing::default();
            if let Some(s) = flag_value(rest, "--give-up") {
                timing.give_up_after = Duration::from_secs(s.parse().context("--give-up")?);
            }
            let dismiss = db
                .setting::<bool>("dismiss_launcher_window")
                .await?
                .unwrap_or(true);
            let fallback = rest.iter().any(|a| a == "--epic-fallback");
            launch::run_to_completion(&db, &game, timing, fallback, dismiss).await?;
            // Spec 5.1: rescan when Moonglass resumes after a game exits.
            library::scan_all(&db, now_unix()).await?;
        }
        "add-exe" => {
            let path = PathBuf::from(
                rest.first()
                    .ok_or_else(|| anyhow!("add-exe needs a path"))?,
            );
            if !path.is_file() {
                bail!("{} is not a file", path.display());
            }
            let title = rest.get(1).cloned().unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
            let normalized = moonglass_platform::normalize_path(&path);
            let id = db
                .add_executable(&normalized, &path.to_string_lossy(), &title, now_unix())
                .await?;
            println!("Added {title} as {id}");
        }
        "add-emulator" => {
            let [id, name, exe, template, exts] =
                rest.get(..5).ok_or_else(|| anyhow!("{USAGE}"))?
            else {
                unreachable!()
            };
            moonglass_core::template::Template::parse(template)?;
            db.upsert_emulator(&Emulator {
                id: id.clone(),
                name: name.clone(),
                executable_path: exe.clone(),
                launch_template: template.clone(),
                file_extensions: exts.clone(),
                folder_marker: rest.get(5).cloned(),
            })
            .await?;
            println!("Saved emulator {name}");
        }
        "add-rom-dir" => {
            let [path, emu, platform] = rest.get(..3).ok_or_else(|| anyhow!("{USAGE}"))? else {
                unreachable!()
            };
            if db.emulator(emu).await?.is_none() {
                bail!("no emulator with id {emu}");
            }
            let path = PathBuf::from(path);
            let normalized = moonglass_platform::normalize_path(&path);
            let id = format!("romdir:{}", moonglass_core::id::path_hash(&normalized));
            let recursive = !rest.iter().any(|a| a == "--flat");
            db.add_rom_directory(&id, &path.to_string_lossy(), emu, platform, recursive)
                .await?;
            for line in library::scan_all(&db, now_unix()).await?.lines {
                println!("{line}");
            }
        }
        "remove" => {
            let game = resolve(&db, rest.first().ok_or_else(|| anyhow!("needs a game"))?).await?;
            if game.source.is_launcher() {
                bail!(
                    "{} comes from {}; hide it instead",
                    game.title,
                    game.source.as_str()
                );
            }
            db.remove_game(&game.id).await?;
            println!("Removed {}", game.title);
        }
        "favorite" | "hide" => {
            let game = resolve(&db, rest.first().ok_or_else(|| anyhow!("needs a game"))?).await?;
            let on = on_off(rest.get(1))?;
            if cmd == "favorite" {
                db.set_favorite(&game.id, on).await?;
            } else {
                db.set_hidden(&game.id, on).await?;
            }
            println!("{}: {cmd} {}", game.title, if on { "on" } else { "off" });
        }
        "options" => {
            let game = resolve(&db, rest.first().ok_or_else(|| anyhow!("needs a game"))?).await?;
            let keep = |flag: &str, current: &Option<String>| {
                flag_value(rest, flag)
                    .map(str::to_string)
                    .or_else(|| current.clone())
                    .filter(|s| !s.is_empty())
            };
            let launch_args = keep("--args", &game.launch_args);
            if let Some(a) = &launch_args {
                moonglass_core::template::split_args(a)?;
            }
            let cwd = keep("--cwd", &game.working_dir);
            let watch = keep("--watch", &game.watch_process);
            let track = rest.iter().any(|a| a == "--track-folder") || game.track_by_folder;
            db.set_launch_options(
                &GameId::from_stored(game.id.to_string()),
                launch_args.as_deref(),
                cwd.as_deref(),
                watch.as_deref(),
                track,
            )
            .await?;
            println!("Saved options for {}", game.title);
        }
        "artwork" => {
            let s = artwork::refresh(&db, &paths.cache_dir, rest.iter().any(|a| a == "--force"))
                .await?;
            println!(
                "Artwork: {} from Steam, {} from SteamGridDB, {} placeholders, {} failed",
                s.from_steam, s.from_sgdb, s.placeholders, s.failed
            );
        }
        "set-art" => {
            let game = resolve(&db, rest.first().ok_or_else(|| anyhow!("needs a game"))?).await?;
            let file = PathBuf::from(rest.get(1).ok_or_else(|| anyhow!("needs an image file"))?);
            artwork::set_override(&db, &paths.cache_dir, &game, &file).await?;
            println!("Artwork for {} set and locked", game.title);
        }
        "unlock-art" => {
            let game = resolve(&db, rest.first().ok_or_else(|| anyhow!("needs a game"))?).await?;
            db.unlock_artwork(&game.id).await?;
            artwork::refresh(&db, &paths.cache_dir, false).await?;
            println!("Artwork for {} is automatic again", game.title);
        }
        "sgdb-key" => match rest.first().map(String::as_str) {
            Some("clear") => {
                db.set_setting(artwork::SETTING_SGDB_KEY, &"").await?;
                println!("SteamGridDB key cleared");
            }
            Some(k) => {
                db.set_setting(artwork::SETTING_SGDB_KEY, &k).await?;
                println!("SteamGridDB key saved");
            }
            None => bail!("sgdb-key needs a key or 'clear'"),
        },
        "autostart" => match rest.first().map(String::as_str) {
            Some("status") | None => println!(
                "Start at login: {}",
                if moonglass_platform::start_at_login()? {
                    "on"
                } else {
                    "off"
                }
            ),
            Some(_) => {
                let on = on_off(rest.first())?;
                moonglass_platform::set_start_at_login(on)?;
                println!("Start at login: {}", if on { "on" } else { "off" });
            }
        },
        other => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("moonglass-io")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start the I/O runtime: {e}");
            std::process::exit(1);
        }
    };
    let result = if args.is_empty() {
        run_ui(&rt)
    } else {
        rt.block_on(run(args))
    };
    if let Err(e) = result {
        tracing::error!("{e:#}");
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// The carousel. GPUI owns the main thread; I/O runs on `rt`.
fn run_ui(rt: &tokio::runtime::Runtime) -> anyhow::Result<()> {
    let paths = AppPaths::discover()?;
    paths.create_all()?;
    let _log = init_logging(&paths)?;
    tracing::info!(
        platform = moonglass_platform::PLATFORM_NAME,
        version = env!("CARGO_PKG_VERSION"),
        "starting UI"
    );
    let instance = match InstanceGuard::acquire()? {
        InstanceStatus::Primary(g) => g,
        InstanceStatus::AlreadyRunning => {
            tracing::info!("already running; asked the other instance to come forward");
            return Ok(());
        }
    };
    let db = rt
        .block_on(Db::open(&paths.database()))
        .context("opening database")?;
    ui::run(ui::Backend {
        db,
        rt: rt.handle().clone(),
        paths,
        instance,
    });
    Ok(())
}
