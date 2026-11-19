//! User preferences shown in Settings (spec 3, Customization; 5.5). Stored
//! as one JSON value in the `settings` table and applied immediately.

use moonglass_core::library::RecentWindow;
use moonglass_core::Source;
use moonglass_platform::CaptureTool;
use serde::{Deserialize, Serialize};

pub const PREFS_KEY: &str = "ui_prefs";

/// Bumped when a default changes in a way saved preferences must follow
/// ([`Prefs::upgraded`]).
///
/// 1. The return-to-Moonglass buttons became both sticks.
const PREFS_VERSION: u32 = 1;

/// A setting with a fixed list of values, cycled with left/right or a click.
pub trait Choice: Sized + Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    fn label(self) -> &'static str;

    fn step(self, delta: isize) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap_or(0) as isize;
        let n = Self::ALL.len() as isize;
        Self::ALL[(i + delta).rem_euclid(n) as usize]
    }
}

macro_rules! choice {
    ($name:ident { $($variant:ident => $label:expr),+ $(,)? } default $default:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[allow(clippy::enum_variant_names)]
        pub enum $name {
            $($variant,)+
        }
        impl $name {
            pub const DEFAULT: Self = Self::$default;
        }
        impl Choice for $name {
            const ALL: &'static [Self] = &[$(Self::$variant),+];
            fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}

choice!(CardShape {
    Poster => "Posters (2:3)",
    Banner => "Banners (16:9)",
    Square => "Squares (1:1)",
} default Poster);

// How a game's art sits on its card when their shapes differ (a square
// Switch icon on a poster card, say).
choice!(CardFit {
    Fit => "Fit whole art",
    Fill => "Zoom to fill",
    Original => "Original shape",
} default Fit);

choice!(FocusSize {
    Normal => "Normal",
    Large => "Large",
    ExtraLarge => "Extra large",
} default Large);

choice!(Spacing {
    Compact => "Compact",
    Normal => "Normal",
    Wide => "Wide",
} default Normal);

choice!(CardDetails {
    ArtOnly => "Art only",
    ArtTitle => "Art + title",
    ArtPlaytime => "Art + playtime & source",
} default ArtOnly);

choice!(Ambient {
    Bloom => "Color bloom",
    Fanart => "Full-screen art",
    Motion => "Full-screen art + motion blur",
    Oled => "Pure black (OLED)",
} default Bloom);

choice!(Blur {
    Light => "Light",
    Medium => "Medium",
    Strong => "Strong",
} default Medium);

choice!(Glow {
    Off => "Off",
    Subtle => "Subtle",
    Normal => "Normal",
    Bright => "Bright",
} default Normal);

choice!(Clock {
    H24 => "24-hour",
    H12 => "12-hour",
} default H24);

// Both sticks by default: Windows can treat View + Menu as the Xbox
// button ("Use View + Menu as Guide button in apps" in the Game Bar
// settings), and holding that opens Task View over the game.
choice!(RestoreButtons {
    ViewMenu => "View + Menu",
    Sticks => "Both sticks pressed in",
} default Sticks);

choice!(RestoreHold {
    S1 => "1 second",
    S2 => "2 seconds",
    S3 => "3 seconds",
} default S2);

/// What the Quick Menu asks to save clips and screenshots (spec 5.8). The
/// choices offered depend on the platform ([`CaptureTool::available`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capture {
    Off,
    GameBar,
    Nvidia,
    Amd,
    GpuScreenRecorder,
}

impl Capture {
    fn from_tool(t: Option<CaptureTool>) -> Self {
        match t {
            None => Capture::Off,
            Some(CaptureTool::GameBar) => Capture::GameBar,
            Some(CaptureTool::Nvidia) => Capture::Nvidia,
            Some(CaptureTool::Amd) => Capture::Amd,
            Some(CaptureTool::GpuScreenRecorder) => Capture::GpuScreenRecorder,
        }
    }

    /// The tool, if it exists on this platform.
    pub fn tool(self) -> Option<CaptureTool> {
        let t = match self {
            Capture::Off => return None,
            Capture::GameBar => CaptureTool::GameBar,
            Capture::Nvidia => CaptureTool::Nvidia,
            Capture::Amd => CaptureTool::Amd,
            Capture::GpuScreenRecorder => CaptureTool::GpuScreenRecorder,
        };
        CaptureTool::available().contains(&t).then_some(t)
    }

    pub fn label(self) -> &'static str {
        self.tool().map_or("Off", CaptureTool::name)
    }

    /// Off, then this platform's tools.
    fn options() -> Vec<Capture> {
        std::iter::once(Capture::Off)
            .chain(
                CaptureTool::available()
                    .iter()
                    .map(|t| Self::from_tool(Some(*t))),
            )
            .collect()
    }

    pub fn step(self, delta: isize) -> Self {
        let all = Self::options();
        let i = all.iter().position(|c| *c == self).unwrap_or(0) as isize;
        all[(i + delta).rem_euclid(all.len() as isize) as usize]
    }
}

impl Default for Capture {
    fn default() -> Self {
        Self::from_tool(CaptureTool::default_here())
    }
}

choice!(LaunchTimeout {
    M1 => "1 minute",
    M3 => "3 minutes",
    M5 => "5 minutes",
    M10 => "10 minutes",
} default M3);

choice!(RecentDays {
    D7 => "7 days",
    D30 => "30 days",
    D90 => "90 days",
} default D30);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub card_shape: CardShape,
    pub card_fit: CardFit,
    pub focus_size: FocusSize,
    pub spacing: Spacing,
    pub card_details: CardDetails,
    pub ambient: Ambient,
    pub blur: Blur,
    pub glow: Glow,
    pub startup_animation: bool,
    pub reduced_motion: bool,
    pub clock: Clock,
    pub recent: RecentDays,
    pub show_hidden: bool,
    /// Hide Steam games the signed-in Steam account cannot play.
    pub steam_playable_only: bool,
    pub dismiss_launcher_window: bool,
    pub restore_buttons: RestoreButtons,
    pub restore_hold: RestoreHold,
    pub launch_timeout: LaunchTimeout,
    /// What saves clips and screenshots from the Quick Menu.
    pub capture: Capture,
    /// Stores whose games are shown. A store turned off is still scanned,
    /// so turning it back on is instant and keeps its games' data.
    pub steam_enabled: bool,
    pub epic_enabled: bool,
    pub gog_enabled: bool,
    pub battlenet_enabled: bool,
    pub msstore_enabled: bool,
    /// [`PREFS_VERSION`] when saved; 0 for preferences saved before it.
    #[serde(default)]
    pub version: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            card_shape: CardShape::DEFAULT,
            card_fit: CardFit::DEFAULT,
            focus_size: FocusSize::DEFAULT,
            spacing: Spacing::DEFAULT,
            card_details: CardDetails::DEFAULT,
            ambient: Ambient::DEFAULT,
            blur: Blur::DEFAULT,
            glow: Glow::DEFAULT,
            startup_animation: true,
            reduced_motion: false,
            clock: Clock::DEFAULT,
            recent: RecentDays::DEFAULT,
            show_hidden: false,
            steam_playable_only: true,
            dismiss_launcher_window: true,
            restore_buttons: RestoreButtons::DEFAULT,
            restore_hold: RestoreHold::DEFAULT,
            launch_timeout: LaunchTimeout::DEFAULT,
            capture: Capture::default(),
            steam_enabled: true,
            epic_enabled: true,
            gog_enabled: true,
            battlenet_enabled: true,
            msstore_enabled: true,
            version: PREFS_VERSION,
        }
    }
}

/// The stores that can be turned off, in display order. Standalone
/// executables and emulated games are the player's own additions and are
/// always shown.
pub const STORES: [Source; 5] = [
    Source::Steam,
    Source::Epic,
    Source::Gog,
    Source::BattleNet,
    Source::MsStore,
];

/// Whether `s` can exist on this platform (Battle.net and the Microsoft
/// Store are Windows only). Settings lists only these.
pub fn store_possible(s: Source) -> bool {
    !matches!(s, Source::BattleNet | Source::MsStore) || moonglass_platform::WINDOWS_STORES
}

pub fn store_name(s: Source) -> &'static str {
    match s {
        Source::Steam => "Steam",
        Source::Epic => "Epic",
        Source::Gog => "GOG",
        Source::BattleNet => "Battle.net",
        Source::MsStore => "Microsoft Store",
        Source::Exe => "Apps",
        Source::Rom => "Emulators",
    }
}

impl Prefs {
    /// Brings preferences saved by an older Moonglass up to date. Every
    /// setting is saved whenever any one changes, so a stored value may be
    /// an old default nobody chose.
    pub fn upgraded(mut self) -> Self {
        if self.version < 1 && self.restore_buttons == RestoreButtons::ViewMenu {
            self.restore_buttons = RestoreButtons::Sticks;
        }
        self.version = PREFS_VERSION;
        self
    }

    /// Whether games from `source` are shown.
    pub fn store_enabled(&self, source: Source) -> bool {
        match source {
            Source::Steam => self.steam_enabled,
            Source::Epic => self.epic_enabled,
            Source::Gog => self.gog_enabled,
            Source::BattleNet => self.battlenet_enabled,
            Source::MsStore => self.msstore_enabled,
            Source::Exe | Source::Rom => true,
        }
    }

    pub fn set_store_enabled(&mut self, source: Source, on: bool) {
        match source {
            Source::Steam => self.steam_enabled = on,
            Source::Epic => self.epic_enabled = on,
            Source::Gog => self.gog_enabled = on,
            Source::BattleNet => self.battlenet_enabled = on,
            Source::MsStore => self.msstore_enabled = on,
            Source::Exe | Source::Rom => {}
        }
    }

    /// Card width / height.
    pub fn card_aspect(&self) -> f32 {
        match self.card_shape {
            CardShape::Poster => 2.0 / 3.0,
            CardShape::Banner => 16.0 / 9.0,
            CardShape::Square => 1.0,
        }
    }

    /// Focused card height as a share of the screen height, before the
    /// width limit for wide shapes.
    pub fn focus_height(&self) -> f32 {
        match self.focus_size {
            FocusSize::Normal => 0.44,
            FocusSize::Large => 0.50,
            FocusSize::ExtraLarge => 0.56,
        }
    }

    pub fn gap_factor(&self) -> f32 {
        match self.spacing {
            Spacing::Compact => 0.06,
            Spacing::Normal => 0.12,
            Spacing::Wide => 0.22,
        }
    }

    pub fn blur_level(&self) -> usize {
        match self.blur {
            Blur::Light => 0,
            Blur::Medium => 1,
            Blur::Strong => 2,
        }
    }

    pub fn glow_brightness(&self) -> f32 {
        match self.glow {
            Glow::Off => 0.0,
            Glow::Subtle => 0.55,
            Glow::Normal => 1.0,
            Glow::Bright => 1.45,
        }
    }

    pub fn recent_window(&self) -> RecentWindow {
        match self.recent {
            RecentDays::D7 => RecentWindow::Days7,
            RecentDays::D30 => RecentWindow::Days30,
            RecentDays::D90 => RecentWindow::Days90,
        }
    }

    pub fn restore_combo(&self) -> moonglass_platform::RestoreCombo {
        match self.restore_buttons {
            RestoreButtons::ViewMenu => moonglass_platform::RestoreCombo::ViewMenu,
            RestoreButtons::Sticks => moonglass_platform::RestoreCombo::StickClicks,
        }
    }

    pub fn restore_hold_time(&self) -> std::time::Duration {
        std::time::Duration::from_secs(match self.restore_hold {
            RestoreHold::S1 => 1,
            RestoreHold::S2 => 2,
            RestoreHold::S3 => 3,
        })
    }

    pub fn give_up_after(&self) -> std::time::Duration {
        std::time::Duration::from_secs(
            60 * match self.launch_timeout {
                LaunchTimeout::M1 => 1,
                LaunchTimeout::M3 => 3,
                LaunchTimeout::M5 => 5,
                LaunchTimeout::M10 => 10,
            },
        )
    }

    pub fn animate(&self) -> bool {
        !self.reduced_motion
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_wraps() {
        assert_eq!(Glow::Bright.step(1), Glow::Off);
        assert_eq!(Glow::Off.step(-1), Glow::Bright);
        assert_eq!(CardShape::Poster.step(1), CardShape::Banner);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let p: Prefs = serde_json::from_str(r#"{"glow":"Bright"}"#).unwrap();
        assert_eq!(p.glow, Glow::Bright);
        assert_eq!(p.focus_size, FocusSize::Large);
        assert!(p.startup_animation);
        assert!(p.dismiss_launcher_window);
        // Saved before stores could be turned off: all of them are on.
        assert!(STORES.iter().all(|s| p.store_enabled(*s)));
    }

    #[test]
    fn old_view_menu_default_becomes_sticks() {
        let old: Prefs = serde_json::from_str(r#"{"restore_buttons":"ViewMenu"}"#).unwrap();
        assert_eq!(old.version, 0);
        let p = old.upgraded();
        assert_eq!(p.restore_buttons, RestoreButtons::Sticks);
        assert_eq!(p.version, PREFS_VERSION);

        // Chosen after the change: kept.
        let chosen: Prefs =
            serde_json::from_str(r#"{"restore_buttons":"ViewMenu","version":1}"#).unwrap();
        assert_eq!(chosen.upgraded().restore_buttons, RestoreButtons::ViewMenu);
        assert_eq!(Prefs::default().version, PREFS_VERSION);
    }

    #[test]
    fn capture_cycles_through_this_platforms_tools() {
        let mut seen = vec![Capture::Off];
        let mut c = Capture::Off.step(1);
        while c != Capture::Off {
            assert!(c.tool().is_some(), "{c:?} offered but unavailable");
            seen.push(c);
            c = c.step(1);
        }
        assert_eq!(seen.len(), CaptureTool::available().len() + 1);
        assert_eq!(Capture::Off.step(-1), *seen.last().unwrap());
    }

    #[test]
    fn stores_turn_off_one_at_a_time() {
        let mut p = Prefs::default();
        p.set_store_enabled(Source::Epic, false);
        assert!(p.store_enabled(Source::Steam));
        assert!(!p.store_enabled(Source::Epic));
        // The player's own additions cannot be turned off.
        p.set_store_enabled(Source::Exe, false);
        assert!(p.store_enabled(Source::Exe));
    }
}
