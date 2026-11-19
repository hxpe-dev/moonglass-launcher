//! The interface's icons: SVGs drawn on a 24-unit grid, centered in their
//! box, and embedded in the binary. Text glyphs (⚙, ⌨, ★) sit on the font's
//! baseline and differ between fonts, so they never center; an SVG fills
//! exactly the box it is given.
//!
//! GPUI draws an SVG as a mask: the shape's own colors are ignored and the
//! element's text color fills it, so [`icon`] always takes a color.

use std::borrow::Cow;

use gpui::{prelude::*, px, svg, AssetSource, Hsla, SharedString, Svg};

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon {
            $($variant,)+
        }

        impl Icon {
            const ALL: &'static [Icon] = &[$(Icon::$variant),+];

            fn path(self) -> &'static str {
                match self {
                    $(Icon::$variant => concat!("icons/", $file),)+
                }
            }

            fn source(self) -> &'static str {
                match self {
                    $(Icon::$variant => include_str!(concat!("../../assets/icons/", $file)),)+
                }
            }
        }
    };
}

icons! {
    Gear => "gear.svg",
    Keyboard => "keyboard.svg",
    Search => "search.svg",
    Close => "close.svg",
    Star => "star.svg",
    Menu => "menu.svg",
    Circle => "circle.svg",
    Square => "square.svg",
    Triangle => "triangle.svg",
    ArrowUp => "arrow-up.svg",
    ArrowDown => "arrow-down.svg",
    ArrowLeft => "arrow-left.svg",
    ArrowRight => "arrow-right.svg",
    ChevronLeft => "chevron-left.svg",
    ChevronRight => "chevron-right.svg",
    ChevronUp => "chevron-up.svg",
    ChevronDown => "chevron-down.svg",
    Drive => "drive.svg",
    Folder => "folder.svg",
    File => "file.svg",
    Check => "check.svg",
    Previous => "previous.svg",
    Next => "next.svg",
    Play => "play.svg",
    Pause => "pause.svg",
}

/// An icon `size` pixels square, filled with `color`.
pub fn icon(which: Icon, size: f32, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(which.path())
        .size(px(size))
        .flex_none()
        .text_color(color.into())
}

/// The icons a key cap label stands for: controller glyphs ("✕", "≡") and
/// arrow pairs ("↑↓"). `None` for labels drawn as text ("A", "Enter").
pub fn keycap_icons(label: &str) -> Option<&'static [Icon]> {
    Some(match label {
        "✕" => &[Icon::Close],
        "○" => &[Icon::Circle],
        "□" => &[Icon::Square],
        "△" => &[Icon::Triangle],
        "≡" => &[Icon::Menu],
        "↑↓" => &[Icon::ArrowUp, Icon::ArrowDown],
        "←→" => &[Icon::ArrowLeft, Icon::ArrowRight],
        _ => return None,
    })
}

/// Serves the embedded icons to GPUI's SVG renderer.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL
            .iter()
            .find(|i| i.path() == path)
            .map(|i| Cow::Borrowed(i.source().as_bytes())))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .iter()
            .map(|i| i.path())
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads() {
        for i in Icon::ALL {
            let bytes = Assets.load(i.path()).unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg") && text.contains("viewBox=\"0 0 24 24\""));
            // The SVG parser rejects a file whose root repeats an attribute.
            let root = text.split('>').next().unwrap();
            let mut names: Vec<&str> = root
                .split_whitespace()
                .filter_map(|a| a.split_once('=').map(|(n, _)| n))
                .collect();
            let n = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), n, "{i:?} repeats an attribute");
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}
