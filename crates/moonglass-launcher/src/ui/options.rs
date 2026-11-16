//! Per-game Options menu, `[Y]` (spec 5.5): Favorite, Hide, Change Artwork,
//! Launch Arguments, Working Directory, Process to Watch, Remove (for
//! standalone executables and ROM entries).

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};
use moonglass_core::{Game, Source};

use super::gamepad::NavAction;
use super::theme::{black, white, INK};
use super::LauncherView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    Play,
    Favorite,
    Hide,
    ChangeArt,
    ResetArt,
    LaunchArgs,
    WorkingDir,
    WatchProcess,
    TrackByFolder,
    Remove,
}

pub struct OptionsMenu {
    pub game: Game,
    pub items: Vec<Opt>,
    pub selected: usize,
    /// Remove asks for a second press.
    pub confirm_remove: bool,
}

impl OptionsMenu {
    pub fn new(game: Game) -> Self {
        let own = matches!(game.source, Source::Exe | Source::Rom);
        let mut items = vec![Opt::Play, Opt::Favorite, Opt::Hide, Opt::ChangeArt];
        if game.artwork_locked {
            items.push(Opt::ResetArt);
        }
        if own {
            items.push(Opt::LaunchArgs);
        }
        if game.source == Source::Exe {
            items.push(Opt::WorkingDir);
        }
        items.push(Opt::WatchProcess);
        if game.source == Source::Exe {
            items.push(Opt::TrackByFolder);
        }
        if own {
            items.push(Opt::Remove);
        }
        Self {
            game,
            items,
            selected: 0,
            confirm_remove: false,
        }
    }

    fn label(&self, o: Opt) -> String {
        let g = &self.game;
        match o {
            Opt::Play => "Play".into(),
            Opt::Favorite if g.is_favorite => "Remove from favorites".into(),
            Opt::Favorite => "Add to favorites".into(),
            Opt::Hide if g.is_hidden => "Show in library".into(),
            Opt::Hide => "Hide from library".into(),
            Opt::ChangeArt => "Change artwork…".into(),
            Opt::ResetArt => "Use automatic artwork".into(),
            Opt::LaunchArgs => match g.launch_args.as_deref().filter(|s| !s.is_empty()) {
                Some(a) => format!("Launch arguments: {a}"),
                None => "Launch arguments…".into(),
            },
            Opt::WorkingDir => match g.working_dir.as_deref() {
                Some(d) => format!("Working directory: {d}"),
                None => "Working directory: executable's folder".into(),
            },
            Opt::WatchProcess => match g.watch_process.as_deref().filter(|s| !s.is_empty()) {
                Some(p) => format!("Process to watch: {p}"),
                None => "Process to watch: automatic".into(),
            },
            Opt::TrackByFolder => format!(
                "Launcher-style executable: {}",
                if g.track_by_folder { "On" } else { "Off" }
            ),
            Opt::Remove if self.confirm_remove => "Press again to remove".into(),
            Opt::Remove => "Remove from Moonglass".into(),
        }
    }

    /// Returns the option to run, if any.
    pub fn nav(&mut self, a: NavAction) -> Option<Opt> {
        let n = self.items.len();
        match a {
            NavAction::Up => {
                self.selected = (self.selected + n - 1) % n;
                self.confirm_remove = false;
            }
            NavAction::Down => {
                self.selected = (self.selected + 1) % n;
                self.confirm_remove = false;
            }
            NavAction::Confirm => return self.items.get(self.selected).copied(),
            _ => {}
        }
        None
    }
}

impl LauncherView {
    pub(super) fn render_options(
        &self,
        m: &OptionsMenu,
        x: f32,
        y: f32,
        u: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_col().gap(px(4.0 * u));
        for (i, o) in m.items.iter().copied().enumerate() {
            let sel = i == m.selected;
            let danger = o == Opt::Remove;
            list = list.child(
                div()
                    .id(("opt", i))
                    .px(px(18.0 * u))
                    .py(px(10.0 * u))
                    .rounded(px(10.0 * u))
                    .cursor_pointer()
                    .when(sel, |d| d.bg(white(0.92)).text_color(rgb(INK)))
                    .when(!sel, |d| d.hover(|s| s.bg(white(0.08))))
                    .when(danger && !sel, |d| d.text_color(rgb(0xF08A88)))
                    .truncate()
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.options_click(i, window, cx)),
                    )
                    .child(SharedString::from(m.label(o))),
            );
        }
        div()
            .id("options-scrim")
            .occlude()
            .absolute()
            .size_full()
            .bg(black(0.35))
            .on_click(cx.listener(|this, _, _, cx| this.close_modal(cx)))
            .child(
                div()
                    .id("options")
                    .occlude()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(460.0 * u))
                    .p(px(16.0 * u))
                    .rounded(px(18.0 * u))
                    .bg(gpui::rgb(0x15171C))
                    .border_1()
                    .border_color(white(0.12))
                    .flex()
                    .flex_col()
                    .gap(px(10.0 * u))
                    // Clicks inside the menu must not close it.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(18.0 * u))
                            .text_size(px(15.0 * u))
                            .text_color(white(0.55))
                            .truncate()
                            .child(SharedString::from(m.game.title.clone())),
                    )
                    .child(list),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moonglass_core::GameId;

    fn game(source: Source) -> Game {
        Game {
            id: GameId::from_stored("x".into()),
            title: "X".into(),
            sort_title: "x".into(),
            search_title: "x".into(),
            source,
            source_id: None,
            launch_target: None,
            launch_args: None,
            working_dir: None,
            rom_directory_id: None,
            platform: None,
            install_dir: None,
            watch_process: None,
            track_by_folder: false,
            cover_path: None,
            hero_path: None,
            logo_path: None,
            dominant_color: None,
            accent_color: None,
            artwork_locked: false,
            date_added: 0,
            last_played: None,
            playtime_seconds: 0,
            is_favorite: false,
            is_hidden: false,
            is_installed: true,
        }
    }

    #[test]
    fn items_depend_on_source() {
        let steam = OptionsMenu::new(game(Source::Steam));
        assert!(!steam.items.contains(&Opt::Remove));
        assert!(!steam.items.contains(&Opt::LaunchArgs));
        let exe = OptionsMenu::new(game(Source::Exe));
        assert!(exe.items.contains(&Opt::Remove));
        assert!(exe.items.contains(&Opt::WorkingDir));
        assert!(exe.items.contains(&Opt::TrackByFolder));
        let rom = OptionsMenu::new(game(Source::Rom));
        assert!(rom.items.contains(&Opt::LaunchArgs));
        assert!(!rom.items.contains(&Opt::WorkingDir));
    }

    #[test]
    fn wraps_and_confirms() {
        let mut m = OptionsMenu::new(game(Source::Steam));
        m.nav(NavAction::Up);
        assert_eq!(m.selected, m.items.len() - 1);
        m.nav(NavAction::Down);
        assert_eq!(m.nav(NavAction::Confirm), Some(Opt::Play));
    }
}
