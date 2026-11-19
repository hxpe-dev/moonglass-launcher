//! Emulator profiles and ROM folders in Settings (spec 2 "Emulated ROMs",
//! 5.5): presets, the per-emulator menu, and what each of its rows does.

use std::path::{Path, PathBuf};

use gpui::Context;
use moonglass_db::{Db, Emulator, RomFolder};

use super::browser::Purpose;
use super::menu::{Menu, MenuAction, MenuRow};
use super::osk::{Osk, TextTarget};
use super::{LauncherView, Modal};
use crate::now_unix;

/// A ready-made profile: only the program's location is asked for.
pub struct Preset {
    pub name: &'static str,
    /// Arguments only (spec 2, Launch Template Syntax).
    pub template: &'static str,
    pub extensions: &'static str,
    pub folder_marker: Option<&'static str>,
    /// Flathub IDs of the emulator: on Linux an installed one is used
    /// directly (`flatpak run <id>`), with no program to choose.
    pub flatpak_ids: &'static [&'static str],
}

pub const PRESETS: [Preset; 7] = [
    Preset {
        name: "RetroArch",
        // A core is chosen with `-L <core>`; without it RetroArch asks.
        template: r#""{rom}""#,
        extensions: ".zip,.7z,.nes,.sfc,.smc,.gb,.gbc,.gba,.md,.gen,.n64,.z64,.nds,.cue,.chd,.m3u",
        folder_marker: None,
        flatpak_ids: &["org.libretro.RetroArch"],
    },
    Preset {
        name: "Dolphin",
        template: r#"-b -e "{rom}""#,
        extensions: ".iso,.rvz,.gcm,.gcz,.ciso,.wbfs,.wad",
        folder_marker: None,
        flatpak_ids: &["org.DolphinEmu.dolphin-emu"],
    },
    Preset {
        name: "PCSX2",
        template: r#"-batch -fullscreen -- "{rom}""#,
        extensions: ".iso,.chd,.cso,.cue,.m3u",
        folder_marker: None,
        flatpak_ids: &["net.pcsx2.PCSX2"],
    },
    Preset {
        name: "RPCS3",
        template: r#"--no-gui "{rom}""#,
        extensions: "",
        folder_marker: Some("PS3_GAME/USRDIR/EBOOT.BIN"),
        flatpak_ids: &["net.rpcs3.RPCS3"],
    },
    Preset {
        name: "Ryujinx",
        template: r#"--fullscreen "{rom}""#,
        extensions: SWITCH_FILES,
        folder_marker: None,
        flatpak_ids: &["io.github.ryubing.Ryujinx", "org.ryujinx.Ryujinx"],
    },
    Preset {
        // yuzu and the emulators built from it take the same arguments.
        name: "Eden / Citron / yuzu",
        template: r#"-f -g "{rom}""#,
        extensions: SWITCH_FILES,
        folder_marker: None,
        flatpak_ids: &["org.yuzu_emu.yuzu"],
    },
    Preset {
        name: "Another emulator",
        template: r#""{rom}""#,
        extensions: "",
        folder_marker: None,
        flatpak_ids: &[],
    },
];

/// Switch games. Updates and add-on content, which are `.nsp` files too, are
/// left out by the scanner.
const SWITCH_FILES: &str = ".nsp,.xci,.nro";

/// The last preset: a profile the user fills in.
const CUSTOM: usize = PRESETS.len() - 1;

/// An emulator profile with its ROM folders, as Settings shows it.
#[derive(Debug, Clone)]
pub struct EmulatorInfo {
    pub emulator: Emulator,
    pub folders: Vec<RomFolder>,
}

impl EmulatorInfo {
    pub fn summary(&self) -> String {
        match self.folders.len() {
            0 => "No ROM folder yet".into(),
            1 => "1 ROM folder".into(),
            n => format!("{n} ROM folders"),
        }
    }
}

pub async fn load(db: &Db) -> anyhow::Result<Vec<EmulatorInfo>> {
    let mut folders = db.rom_folders().await?;
    Ok(db
        .emulators()
        .await?
        .into_iter()
        .map(|emulator| {
            let (mine, rest) = folders
                .drain(..)
                .partition(|f| f.emulator_id == emulator.id);
            folders = rest;
            EmulatorInfo {
                emulator,
                folders: mine,
            }
        })
        .collect())
}

/// A profile ID from a name: "PCSX2" gives `pcsx2`, then `pcsx2-2`.
fn new_id(name: &str, taken: &[EmulatorInfo]) -> String {
    let mut slug: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    if slug.is_empty() {
        slug = "emulator".into();
    }
    let exists = |id: &str| taken.iter().any(|e| e.emulator.id == id);
    if !exists(&slug) {
        return slug;
    }
    (2..)
        .map(|n| format!("{slug}-{n}"))
        .find(|id| !exists(id))
        .unwrap_or(slug)
}

fn preset_menu() -> Menu {
    Menu {
        title: "Add an emulator".into(),
        note: Some("Choose the emulator, then its program.".into()),
        rows: PRESETS
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let label = if i == CUSTOM {
                    format!("{}…", p.name)
                } else if moonglass_platform::installed_flatpak(p.flatpak_ids).is_some() {
                    // Linux: picked up as installed, no program to choose.
                    format!("{}  ·  Flatpak found", p.name)
                } else {
                    p.name.to_string()
                };
                MenuRow::new(label, MenuAction::AddEmulator(i))
            })
            .collect(),
        selected: 0,
        armed: false,
        emulator: None,
        steam_accounts: false,
    }
}

fn emulator_menu(info: &EmulatorInfo) -> Menu {
    let e = &info.emulator;
    let file_types = match (&e.folder_marker, e.file_extensions.trim()) {
        (Some(marker), "") => format!("Games are folders containing {marker}"),
        (_, "") => "File types: none yet".into(),
        (_, ext) => format!("File types: {ext}"),
    };
    let mut rows = vec![
        MenuRow::new(
            format!("Program: {}", e.executable_path),
            MenuAction::ChangeProgram,
        ),
        MenuRow::new(
            format!("Arguments: {}", e.launch_template),
            MenuAction::EditArguments,
        ),
        MenuRow::new(file_types, MenuAction::EditFileTypes),
        MenuRow::new("Add a ROM folder…", MenuAction::AddRomFolder),
    ];
    rows.extend(info.folders.iter().map(|f| {
        MenuRow::danger(
            format!("Remove folder: {} ({})", f.path, f.platform),
            MenuAction::RemoveRomFolder(f.id.clone()),
        )
    }));
    rows.push(MenuRow::danger(
        format!("Remove {}", e.name),
        MenuAction::RemoveEmulator,
    ));
    Menu {
        title: e.name.clone(),
        note: Some("{rom} in the arguments is replaced by the game's file.".into()),
        rows,
        selected: 0,
        armed: false,
        emulator: Some(e.id.clone()),
        steam_accounts: false,
    }
}

impl LauncherView {
    fn emulator(&self, id: &str) -> Option<&EmulatorInfo> {
        self.emulators.iter().find(|e| e.emulator.id == id)
    }

    pub(super) fn open_preset_menu(&mut self) {
        self.set_modal(Modal::Menu(preset_menu()));
    }

    pub(super) fn open_emulator_menu(&mut self, id: &str) {
        if let Some(info) = self.emulator(id) {
            let menu = emulator_menu(info);
            self.set_modal(Modal::Menu(menu));
        }
    }

    /// New profiles and folders arrived from the database: an open
    /// emulator menu shows them, keeping its place.
    pub(super) fn emulators_changed(&mut self, list: Vec<EmulatorInfo>) {
        self.emulators = list;
        if let super::Screen::Settings(st) = &mut self.screen {
            st.set_emulators(self.emulators.len());
        }
        // A profile that was just added or given a new program.
        if let Some(id) = self.pending_emulator_menu.clone() {
            if self.emulator(&id).is_some() && matches!(self.modal, Modal::None) {
                self.pending_emulator_menu = None;
                self.open_emulator_menu(&id);
                return;
            }
        }
        let Modal::Menu(m) = &self.modal else { return };
        let Some(id) = m.emulator.clone() else { return };
        let at = m.selected;
        match self.emulator(&id).map(emulator_menu) {
            Some(mut fresh) => {
                fresh.selected = at.min(fresh.rows.len() - 1);
                // Same kind of modal: replaced in place, no animation.
                self.modal = Modal::Menu(fresh);
            }
            None => self.set_modal(Modal::None),
        }
    }

    pub(super) fn menu_click(&mut self, i: usize, cx: &mut Context<Self>) {
        self.set_input(super::InputMode::Mouse, cx);
        if let Modal::Menu(m) = &mut self.modal {
            if let Some(action) = m.click(i) {
                self.run_menu_action(action, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn run_menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        let Modal::Menu(m) = &self.modal else { return };
        let emulator = m.emulator.as_deref().and_then(|id| self.emulator(id));
        match (action, emulator.cloned()) {
            (MenuAction::AddEmulator(preset), _) => {
                let flatpak = PRESETS
                    .get(preset)
                    .and_then(|p| moonglass_platform::installed_flatpak(p.flatpak_ids));
                match flatpak {
                    // Installed from Flathub: nothing to choose.
                    Some(app_id) => self.add_preset(preset, "flatpak".into(), Some(app_id), None),
                    None => self.open_browser(Purpose::EmulatorProgram {
                        preset: Some(preset),
                        emulator: None,
                    }),
                }
            }
            (MenuAction::ChangeProgram, Some(info)) => {
                self.open_browser(Purpose::EmulatorProgram {
                    preset: None,
                    emulator: Some(info.emulator.id),
                });
            }
            (MenuAction::EditArguments, Some(info)) => {
                let e = info.emulator;
                self.set_modal(Modal::Keyboard(Osk::new(
                    TextTarget::EmulatorArguments(e.id),
                    e.launch_template,
                )));
            }
            (MenuAction::EditFileTypes, Some(info)) => {
                let e = info.emulator;
                self.set_modal(Modal::Keyboard(Osk::new(
                    TextTarget::EmulatorFileTypes(e.id),
                    e.file_extensions,
                )));
            }
            (MenuAction::AddRomFolder, Some(info)) => {
                self.open_browser(Purpose::RomFolder(info.emulator.id));
            }
            (MenuAction::RemoveRomFolder(folder), Some(_)) => {
                self.db_task(move |db, _| async move {
                    db.remove_rom_directory(&folder).await?;
                    Ok(Some("ROM folder removed".to_string()))
                });
            }
            (MenuAction::SwitchSteamAccount(id), _) => self.switch_steam_account(&id, cx),
            (MenuAction::ChooseSteamFolder, _) => self.open_browser(Purpose::SteamFolder),
            (MenuAction::AutoSteamFolder, _) => self.set_steam_folder(None),
            (MenuAction::UnignoreSteamApp(app), _) => self.set_steam_ignored(app, false, cx),
            (MenuAction::ChooseMusicFolder, _) => self.open_browser(Purpose::MusicFolder),
            (MenuAction::NextTrack, _) => {
                self.set_modal(Modal::None);
                self.audio.next_track();
            }
            (MenuAction::MusicOff, _) => self.set_music_folder(None),
            (MenuAction::RemoveEmulator, Some(info)) => {
                let e = info.emulator;
                self.set_modal(Modal::None);
                self.db_task(move |db, _| async move {
                    db.remove_emulator(&e.id).await?;
                    Ok(Some(format!("Removed {}", e.name)))
                });
            }
            _ => {}
        }
        cx.notify();
    }

    /// The file browser returned an emulator's program.
    pub(super) fn picked_emulator_program(
        &mut self,
        preset: Option<usize>,
        emulator: Option<String>,
        path: PathBuf,
    ) {
        let program = path.to_string_lossy().into_owned();
        let profile = match (emulator.as_deref().and_then(|id| self.emulator(id)), preset) {
            // A new program for an existing profile.
            (Some(info), _) => Emulator {
                executable_path: program,
                ..info.emulator.clone()
            },
            (None, Some(i)) => {
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                return self.add_preset(i, program, None, stem);
            }
            (None, None) => return,
        };
        let id = profile.id.clone();
        self.db_task(move |db, _| async move {
            db.upsert_emulator(&profile).await?;
            Ok(Some(format!("Program changed for {}", profile.name)))
        });
        // Shown once the profile is back from the database.
        self.pending_emulator_menu = Some(id);
    }

    /// Adds a profile from preset `i`. `flatpak_app` runs it as that
    /// Flatpak (the program is then `flatpak`); `custom_name` names a
    /// profile made from "Another emulator".
    fn add_preset(
        &mut self,
        i: usize,
        program: String,
        flatpak_app: Option<String>,
        custom_name: Option<String>,
    ) {
        let p = &PRESETS[i.min(CUSTOM)];
        let name = match custom_name {
            Some(n) if i >= CUSTOM => n,
            _ => p.name.to_string(),
        };
        let launch_template = match &flatpak_app {
            Some(app) => format!("run {app} {}", p.template),
            None => p.template.to_string(),
        };
        let profile = Emulator {
            id: new_id(&name, &self.emulators),
            name,
            executable_path: program,
            launch_template,
            file_extensions: p.extensions.to_string(),
            folder_marker: p.folder_marker.map(str::to_string),
        };
        let id = profile.id.clone();
        let via = if flatpak_app.is_some() {
            " (its Flatpak)"
        } else {
            ""
        };
        self.db_task(move |db, _| async move {
            db.upsert_emulator(&profile).await?;
            Ok(Some(format!(
                "Added {}{via} · now add a ROM folder",
                profile.name
            )))
        });
        // Shown once the profile is back from the database.
        self.pending_emulator_menu = Some(id);
    }

    /// The file browser returned a ROM folder: ask what to call its
    /// platform, suggesting the folder's own name ("GameCube").
    pub(super) fn picked_rom_folder(&mut self, emulator: String, path: PathBuf) {
        let suggestion = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.set_modal(Modal::Keyboard(Osk::new(
            TextTarget::RomPlatform { emulator, path },
            suggestion,
        )));
    }

    pub(super) fn save_emulator_arguments(&mut self, id: &str, text: String) {
        let Some(info) = self.emulator(id) else {
            return;
        };
        if let Err(e) = moonglass_core::template::Template::parse(&text) {
            self.toast = Some((
                format!("Arguments not saved: {e}"),
                std::time::Instant::now(),
            ));
            self.open_emulator_menu(id);
            return;
        }
        let profile = Emulator {
            launch_template: text,
            ..info.emulator.clone()
        };
        self.save_profile(profile);
    }

    pub(super) fn save_emulator_file_types(&mut self, id: &str, text: String) {
        let Some(info) = self.emulator(id) else {
            return;
        };
        let profile = Emulator {
            file_extensions: text,
            ..info.emulator.clone()
        };
        self.save_profile(profile);
    }

    /// Saves a changed profile, rescans (file types decide what is listed),
    /// and returns to its menu.
    fn save_profile(&mut self, profile: Emulator) {
        let id = profile.id.clone();
        self.db_task(move |db, cache| async move {
            db.upsert_emulator(&profile).await?;
            crate::library::scan_all(&db, now_unix()).await?;
            crate::artwork::refresh(&db, &cache, false).await?;
            Ok(Some(format!("Saved for {}", profile.name)))
        });
        self.open_emulator_menu(&id);
    }

    pub(super) fn add_rom_folder(&mut self, emulator: &str, path: &Path, platform: String) {
        let Some(info) = self.emulator(emulator) else {
            return;
        };
        let platform = if platform.is_empty() {
            info.emulator.name.clone()
        } else {
            platform
        };
        let normalized = moonglass_platform::normalize_path(path);
        let already = self
            .emulators
            .iter()
            .flat_map(|e| &e.folders)
            .any(|f| moonglass_platform::normalize_path(Path::new(&f.path)) == normalized);
        if already {
            self.toast = Some((
                "That folder is already a ROM folder".into(),
                std::time::Instant::now(),
            ));
            self.open_emulator_menu(emulator);
            return;
        }
        let dir_id = format!("romdir:{}", moonglass_core::id::path_hash(&normalized));
        let emulator_id = info.emulator.id.clone();
        let path = path.to_string_lossy().into_owned();
        self.db_task(move |db, cache| async move {
            db.add_rom_directory(&dir_id, &path, &emulator_id, &platform, true)
                .await?;
            let before = db.games().await?.len();
            crate::library::scan_all(&db, now_unix()).await?;
            crate::artwork::refresh(&db, &cache, false).await?;
            let found = db.games().await?.len().saturating_sub(before);
            Ok(Some(format!(
                "{platform} folder added · {found} games found"
            )))
        });
        self.open_emulator_menu(emulator);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, folders: usize) -> EmulatorInfo {
        EmulatorInfo {
            emulator: Emulator {
                id: id.into(),
                name: "Dolphin".into(),
                executable_path: "C:/Emu/Dolphin.exe".into(),
                launch_template: PRESETS[1].template.into(),
                file_extensions: PRESETS[1].extensions.into(),
                folder_marker: None,
            },
            folders: (0..folders)
                .map(|i| RomFolder {
                    id: format!("romdir:{i}"),
                    path: format!("D:/ROMs/{i}"),
                    emulator_id: id.into(),
                    platform: "GameCube".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn preset_templates_are_valid() {
        for p in &PRESETS {
            moonglass_core::template::Template::parse(p.template)
                .unwrap_or_else(|e| panic!("{}: {e}", p.name));
        }
    }

    #[test]
    fn ids_do_not_collide() {
        let taken = [info("dolphin", 0), info("dolphin-2", 0)];
        assert_eq!(new_id("Dolphin", &taken), "dolphin-3");
        assert_eq!(new_id("PCSX2", &taken), "pcsx2");
        assert_eq!(new_id("…", &taken), "emulator");
    }

    #[test]
    fn menu_lists_folders_and_removal() {
        let m = emulator_menu(&info("dolphin", 2));
        let removals = m.rows.iter().filter(|r| r.danger).count();
        assert_eq!(removals, 3, "two folders and the emulator");
        assert_eq!(m.emulator.as_deref(), Some("dolphin"));
        assert_eq!(info("dolphin", 2).summary(), "2 ROM folders");
    }
}
