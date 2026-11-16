//! Built-in file browser (spec 5.5): a controller-navigable list of folders,
//! used wherever a path is needed. Starts from the drive list (Windows) or
//! home plus mounted volumes (Linux), and filters files by what is being
//! picked.

use std::path::{Path, PathBuf};

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};
use moonglass_core::GameId;

use super::gamepad::NavAction;
use super::theme::{black, keycap, on, selected, white};
use super::LauncherView;

/// Rows shown at once; the list scrolls around the selection.
const VISIBLE_ROWS: usize = 11;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    AddExecutable,
    Artwork(GameId),
    WorkingDir(GameId),
    /// The program of a new emulator (from a preset) or of an existing one.
    EmulatorProgram {
        preset: Option<usize>,
        emulator: Option<String>,
    },
    /// A ROM folder for this emulator profile.
    RomFolder(String),
}

impl Purpose {
    pub fn title(&self) -> &'static str {
        match self {
            Purpose::AddExecutable => "Add a game: choose its executable",
            Purpose::Artwork(_) => "Change artwork: choose an image",
            Purpose::WorkingDir(_) => "Working directory: choose a folder",
            Purpose::EmulatorProgram { .. } => "Emulator: choose its program",
            Purpose::RomFolder(_) => "ROM folder: choose the folder with the games",
        }
    }

    /// Picks a folder rather than a file.
    fn picks_folder(&self) -> bool {
        matches!(self, Purpose::WorkingDir(_) | Purpose::RomFolder(_))
    }

    fn wants_file(&self, path: &Path) -> bool {
        match self {
            Purpose::AddExecutable | Purpose::EmulatorProgram { .. } => {
                moonglass_platform::is_launchable(path)
            }
            Purpose::Artwork(_) => path.extension().is_some_and(|e| {
                ["png", "jpg", "jpeg", "webp"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            }),
            Purpose::WorkingDir(_) | Purpose::RomFolder(_) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// Go to the parent folder (or the list of drives).
    Up,
    Root,
    Dir,
    File,
    /// Folder pickers: choose the folder being shown.
    UseThisFolder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub path: PathBuf,
    pub kind: EntryKind,
}

pub struct Browser {
    pub purpose: Purpose,
    /// `None` shows the starting points.
    pub dir: Option<PathBuf>,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub loading: bool,
    pub error: Option<String>,
}

/// What the view should do after a browser action.
pub enum BrowserResult {
    Nothing,
    Open(Option<PathBuf>),
    Picked(PathBuf),
    Close,
}

/// Reads a folder for `purpose`; runs off the UI thread.
pub fn read_entries(purpose: &Purpose, dir: Option<&Path>) -> Result<Vec<Entry>, String> {
    let Some(dir) = dir else {
        return Ok(moonglass_platform::browse_roots()
            .into_iter()
            .map(|(label, path)| Entry {
                label,
                path,
                kind: EntryKind::Root,
            })
            .collect());
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with('$') {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            dirs.push(Entry {
                label: name,
                path,
                kind: EntryKind::Dir,
            });
        } else if purpose.wants_file(&path) {
            files.push(Entry {
                label: name,
                path,
                kind: EntryKind::File,
            });
        }
    }
    let by_name = |a: &Entry, b: &Entry| a.label.to_lowercase().cmp(&b.label.to_lowercase());
    dirs.sort_by(by_name);
    files.sort_by(by_name);

    let mut out = vec![Entry {
        label: "..".into(),
        path: dir.parent().map(Path::to_path_buf).unwrap_or_default(),
        kind: EntryKind::Up,
    }];
    if purpose.picks_folder() {
        out.push(Entry {
            label: "Use this folder".into(),
            path: dir.to_path_buf(),
            kind: EntryKind::UseThisFolder,
        });
    }
    out.extend(dirs);
    out.extend(files);
    Ok(out)
}

impl Browser {
    pub fn new(purpose: Purpose) -> Self {
        Self {
            purpose,
            dir: None,
            entries: Vec::new(),
            selected: 0,
            loading: true,
            error: None,
        }
    }

    fn activate(&self, i: usize) -> BrowserResult {
        let Some(e) = self.entries.get(i) else {
            return BrowserResult::Nothing;
        };
        match e.kind {
            EntryKind::Up => BrowserResult::Open(self.parent()),
            EntryKind::Root | EntryKind::Dir => BrowserResult::Open(Some(e.path.clone())),
            EntryKind::File | EntryKind::UseThisFolder => BrowserResult::Picked(e.path.clone()),
        }
    }

    /// The parent folder, or the starting points from a drive root.
    fn parent(&self) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let parent = dir.parent()?;
        if parent.as_os_str().is_empty() {
            None
        } else {
            Some(parent.to_path_buf())
        }
    }

    pub fn nav(&mut self, a: NavAction) -> BrowserResult {
        let n = self.entries.len();
        match a {
            NavAction::Up if n > 0 => self.selected = (self.selected + n - 1) % n,
            NavAction::Down if n > 0 => self.selected = (self.selected + 1) % n,
            NavAction::FilterPrev | NavAction::Left => {
                self.selected = self.selected.saturating_sub(VISIBLE_ROWS)
            }
            NavAction::FilterNext | NavAction::Right => {
                self.selected = (self.selected + VISIBLE_ROWS).min(n.saturating_sub(1))
            }
            NavAction::Confirm => return self.activate(self.selected),
            NavAction::Back => {
                return if self.dir.is_some() {
                    BrowserResult::Open(self.parent())
                } else {
                    BrowserResult::Close
                }
            }
            _ => {}
        }
        BrowserResult::Nothing
    }

    /// Mouse: the first click selects, a click on the selected row opens it.
    pub fn click(&mut self, i: usize) -> BrowserResult {
        if self.selected == i {
            self.activate(i)
        } else {
            self.selected = i;
            BrowserResult::Nothing
        }
    }

    /// Typing jumps to the next entry starting with that letter.
    pub fn jump_to(&mut self, ch: char) {
        let ch = ch.to_ascii_lowercase();
        let n = self.entries.len();
        for step in 1..=n {
            let i = (self.selected + step) % n;
            if self.entries[i].label.to_lowercase().starts_with(ch) {
                self.selected = i;
                return;
            }
        }
    }

    pub fn wheel(&mut self, steps: isize) {
        let n = self.entries.len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + steps).clamp(0, n - 1) as usize;
        }
    }
}

impl LauncherView {
    pub(super) fn render_browser(
        &self,
        b: &Browser,
        w: f32,
        h: f32,
        u: f32,
        shown: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let start = b
            .selected
            .saturating_sub(VISIBLE_ROWS / 2)
            .min(b.entries.len().saturating_sub(VISIBLE_ROWS));
        let end = (start + VISIBLE_ROWS).min(b.entries.len());

        let mut list = div().flex().flex_col().gap(px(4.0 * u));
        for i in start..end {
            let e = &b.entries[i];
            let sel = i == b.selected;
            let icon = match e.kind {
                EntryKind::Up => "↑",
                EntryKind::Root => "◆",
                EntryKind::Dir => "▸",
                EntryKind::File => "•",
                EntryKind::UseThisFolder => "✓",
            };
            let lit = self.glide(("browse-row", i), on(sel));
            list = list.child(
                selected(div().id(("browse-row", i)), lit)
                    .flex()
                    .items_center()
                    .gap(px(14.0 * u))
                    .px(px(18.0 * u))
                    .py(px(9.0 * u))
                    .rounded(px(10.0 * u))
                    .cursor_pointer()
                    .when(!sel, |d| d.hover(|s| s.bg(white(0.08))))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.browser_click(i, window, cx)),
                    )
                    .child(div().w(px(18.0 * u)).child(icon))
                    .child(div().truncate().child(SharedString::from(e.label.clone()))),
            );
        }
        let body: AnyElement = if b.loading {
            div()
                .text_color(white(0.6))
                .child("Loading…")
                .into_any_element()
        } else if let Some(err) = &b.error {
            div()
                .text_color(white(0.7))
                .child(format!("Can't open this folder: {err}"))
                .into_any_element()
        } else if b.entries.is_empty() {
            div()
                .text_color(white(0.6))
                .child("Empty")
                .into_any_element()
        } else {
            list.into_any_element()
        };

        let path = b
            .dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Choose a location".into());

        let hint = |k: &str, l: &str| {
            div()
                .flex_none()
                .mr(px(26.0 * u))
                .flex()
                .items_center()
                .gap(px(8.0 * u))
                .child(keycap(k, u))
                .child(div().flex_none().child(l.to_string()))
        };
        let keyboard = self.input != super::InputMode::Gamepad;
        let (k_open, k_back, k_page) = if keyboard {
            ("Enter", "Esc", "PgUp PgDn")
        } else {
            ("A", "B", "LB RB")
        };

        div()
            .id("browser")
            .occlude()
            .absolute()
            .size_full()
            .bg(black(0.72))
            .opacity(shown)
            .flex()
            .items_center()
            .justify_center()
            .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _, cx| {
                let dy = match ev.delta {
                    gpui::ScrollDelta::Lines(l) => -l.y,
                    gpui::ScrollDelta::Pixels(p) => -f32::from(p.y) / 40.0,
                };
                if let Some(b) = this.browser_mut() {
                    b.wheel(dy.round() as isize);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .relative()
                    .top(px((1.0 - shown) * 36.0 * u))
                    .w(px((w * 0.62).min(1100.0 * u)))
                    .h(px(h * 0.80))
                    .p(px(30.0 * u))
                    .rounded(px(22.0 * u))
                    .bg(rgb(0x14161B))
                    .border_1()
                    .border_color(white(0.1))
                    .flex()
                    .flex_col()
                    .gap(px(14.0 * u))
                    .child(div().text_size(px(30.0 * u)).child(b.purpose.title()))
                    .child(
                        div()
                            .text_size(px(16.0 * u))
                            .text_color(white(0.6))
                            .truncate()
                            .child(path),
                    )
                    .child(div().flex_1().overflow_hidden().child(body))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(30.0 * u))
                            .text_size(px(16.0 * u))
                            .text_color(white(0.75))
                            .child(hint(k_open, "Open / choose"))
                            .child(hint(k_back, "Up / close"))
                            .child(hint(k_page, "Page"))
                            .child(
                                div()
                                    .id("browser-close")
                                    .ml_auto()
                                    .cursor_pointer()
                                    .px(px(16.0 * u))
                                    .py(px(6.0 * u))
                                    .rounded_full()
                                    .bg(white(0.12))
                                    .hover(|s| s.bg(white(0.24)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_modal(cx);
                                    }))
                                    .child("Cancel"),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_folders_then_matching_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("Zeta")).unwrap();
        std::fs::create_dir(tmp.path().join("alpha")).unwrap();
        std::fs::create_dir(tmp.path().join(".hidden")).unwrap();
        std::fs::write(tmp.path().join("cover.PNG"), b"x").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), b"x").unwrap();
        let p = Purpose::Artwork(GameId::steam("1"));
        let e = read_entries(&p, Some(tmp.path())).unwrap();
        let labels: Vec<_> = e.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["..", "alpha", "Zeta", "cover.PNG"]);

        let w = Purpose::WorkingDir(GameId::steam("1"));
        let e = read_entries(&w, Some(tmp.path())).unwrap();
        assert_eq!(e[1].kind, EntryKind::UseThisFolder);
        assert!(e.iter().all(|e| e.kind != EntryKind::File));
    }

    #[test]
    fn navigation_and_jump() {
        let mut b = Browser::new(Purpose::AddExecutable);
        b.dir = Some(PathBuf::from("/x/y"));
        b.entries = ["..", "Apps", "Games", "Gog"]
            .iter()
            .map(|l| Entry {
                label: l.to_string(),
                path: PathBuf::from(l),
                kind: if *l == ".." {
                    EntryKind::Up
                } else {
                    EntryKind::Dir
                },
            })
            .collect();
        b.jump_to('g');
        assert_eq!(b.selected, 2);
        b.jump_to('g');
        assert_eq!(b.selected, 3);
        b.nav(NavAction::Down);
        assert_eq!(b.selected, 0);
        assert!(
            matches!(b.nav(NavAction::Confirm), BrowserResult::Open(Some(p)) if p == Path::new("/x"))
        );
        assert!(matches!(b.click(1), BrowserResult::Nothing));
        assert!(matches!(b.click(1), BrowserResult::Open(Some(_))));
    }
}
