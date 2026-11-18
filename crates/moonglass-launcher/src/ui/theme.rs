//! Shared drawing helpers: colors, type, easing, key caps and buttons.
//!
//! The look follows the website: white on pure black, a light grotesque
//! for text, small uppercase mono labels, hairline rules and outline pills.

use std::sync::OnceLock;

use gpui::{div, prelude::*, px, rgb, Div, Hsla, Rgba, SharedString};

use super::icons::{icon, keycap_icons};

pub const OBSIDIAN: u32 = 0x000000;
pub const INK: u32 = 0x000000;

/// Text on black, in three strengths (the site's --text, --text-2, --text-3).
pub const TEXT: f32 = 0.95;
pub const TEXT_2: f32 = 0.66;
pub const TEXT_3: f32 = 0.42;
/// Hairline rules and outlines.
pub const LINE: f32 = 0.14;

static FONTS: OnceLock<(SharedString, SharedString)> = OnceLock::new();

/// The site's fonts, built into the binary (SIL Open Font License; the
/// licenses sit beside them in assets/fonts). Hanken Grotesk is one
/// variable file holding every weight.
pub fn bundled_fonts() -> Vec<std::borrow::Cow<'static, [u8]>> {
    vec![
        include_bytes!("../../assets/fonts/HankenGrotesk-Variable.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/IBMPlexMono-Medium.ttf")
            .as_slice()
            .into(),
    ]
}

/// Picks the families once, from what is available: the site's own fonts
/// (bundled), else the fallbacks it names itself.
pub fn pick_fonts(installed: &[String]) {
    let first = |want: &[&'static str]| -> SharedString {
        want.iter()
            .find(|w| installed.iter().any(|i| i.eq_ignore_ascii_case(w)))
            .copied()
            .unwrap_or(want[want.len() - 1])
            .into()
    };
    let _ = FONTS.set((
        first(&[
            "Hanken Grotesk",
            "Segoe UI",
            "Inter",
            "Cantarell",
            "DejaVu Sans",
        ]),
        first(&[
            "IBM Plex Mono",
            "Cascadia Mono",
            "JetBrains Mono",
            "DejaVu Sans Mono",
        ]),
    ));
}

/// The text family.
pub fn sans() -> SharedString {
    FONTS.get().map_or("Segoe UI".into(), |f| f.0.clone())
}

/// The family of labels, key caps and figures.
pub fn mono() -> SharedString {
    FONTS.get().map_or("Cascadia Mono".into(), |f| f.1.clone())
}

/// A small uppercase mono label, the site's `.label`.
pub fn label(text: impl AsRef<str>, size: f32, u: f32) -> Div {
    div()
        .font_family(mono())
        .text_size(px(size * u))
        .text_color(white(TEXT_3))
        .child(text.as_ref().to_uppercase())
}

/// Letters spread apart, as in the wordmark: "M O O N G L A S S".
pub fn spaced(text: &str) -> String {
    text.split(' ')
        .map(|w| {
            w.chars()
                .map(|c| c.to_uppercase().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("   ")
}

/// A hairline from `left` to `right` at `top`, with a dot at each end
/// (where a rule meets the frame on the site).
pub fn rule(top: f32, left: f32, right: f32, u: f32, alpha: f32) -> Div {
    let dot = |d: Div| {
        d.absolute()
            .top(px(-2.5 * u))
            .size(px(5.0 * u))
            .rounded_full()
            .bg(white(0.7 * alpha))
    };
    div()
        .absolute()
        .top(px(top))
        .left(px(left))
        .right(px(right))
        .h(px(1.0))
        .bg(white(LINE * alpha))
        .child(dot(div()).left(px(-2.5 * u)))
        .child(dot(div()).right(px(-2.5 * u)))
}

/// Thin crop marks at the four corners of the parent (which must be
/// positioned), as on a print proof and around the site's wordmark.
pub fn crop_marks(arm: f32, color: Hsla) -> [Div; 4] {
    crop_marks_out(arm, 0.0, color)
}

/// Crop marks standing `out` outside the parent's corners, like a camera's
/// focus brackets around what it frames.
pub fn crop_marks_out(arm: f32, out: f32, color: Hsla) -> [Div; 4] {
    let mark = || div().absolute().size(px(arm)).border_color(color);
    let o = px(-out);
    [
        mark().top(o).left(o).border_t_1().border_l_1(),
        mark().top(o).right(o).border_t_1().border_r_1(),
        mark().bottom(o).left(o).border_b_1().border_l_1(),
        mark().bottom(o).right(o).border_b_1().border_r_1(),
    ]
}

pub fn white(a: f32) -> Hsla {
    gpui::white().opacity(a)
}

pub fn black(a: f32) -> Hsla {
    gpui::black().opacity(a)
}

pub fn hex_color(s: Option<&str>, fallback: u32) -> Hsla {
    let v = s
        .and_then(|s| s.strip_prefix('#'))
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .unwrap_or(fallback);
    rgb(v).into()
}

/// 1 or 0, as an animation target.
pub fn on(b: bool) -> f32 {
    if b {
        1.0
    } else {
        0.0
    }
}

/// The color `t` of the way from `a` to `b`.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

/// The highlight of a selected row or key, `amount` of the way in: white
/// with dark text.
pub fn selected<E: Styled>(el: E, amount: f32) -> E {
    el.bg(white(amount))
        .text_color(mix(white(TEXT), rgb(INK).into(), amount))
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Progress of an animation segment that starts at `start` seconds and
/// lasts `dur` seconds, at time `t`.
pub fn segment(t: f32, start: f32, dur: f32) -> f32 {
    ((t - start) / dur).clamp(0.0, 1.0)
}

/// A rounded key cap ("A", "Enter", "LB RB"). Controller glyphs and arrow
/// pairs ("✕", "≡", "↑↓") are drawn as icons, centered in the cap.
pub fn keycap(label: &str, u: f32) -> Div {
    let cap = div()
        .h(px(30.0 * u))
        .min_w(px(30.0 * u))
        .rounded_full()
        .bg(white(0.92))
        .text_color(rgb(INK))
        .flex()
        .items_center()
        .justify_center();
    if let Some(icons) = keycap_icons(label) {
        return cap
            .when(icons.len() > 1, |d| d.px(px(8.0 * u)).gap(px(2.0 * u)))
            .children(icons.iter().map(|i| icon(*i, 15.0 * u, rgb(INK))));
    }
    cap.px(px(if label.chars().count() > 1 {
        10.0 * u
    } else {
        0.0
    }))
    .text_size(px(15.0 * u))
    .font_weight(gpui::FontWeight::BOLD)
    .child(label.to_string())
}

/// A pill button body; callers add the id, hover and click handler.
pub fn pill(label: impl Into<gpui::SharedString>, u: f32, emphasized: bool) -> Div {
    div()
        .px(px(22.0 * u))
        .py(px(10.0 * u))
        .rounded_full()
        .when(emphasized, |d| d.bg(white(0.92)).text_color(rgb(INK)))
        .when(!emphasized, |d| d.bg(white(0.12)))
        .child(label.into())
}

/// A small battery outline, `level` 0.0..=1.0.
pub fn battery(level: f32, u: f32) -> Div {
    let level = level.clamp(0.0, 1.0);
    let fill = if level > 0.4 {
        rgb(0x7BD88F)
    } else if level > 0.2 {
        rgb(0xF2C14E)
    } else {
        rgb(0xF0605D)
    };
    div()
        .flex()
        .items_center()
        .child(
            div()
                .w(px(30.0 * u))
                .h(px(15.0 * u))
                .rounded(px(4.0 * u))
                .border_1()
                .border_color(white(0.85))
                .p(px(2.0 * u))
                .child(
                    div()
                        .h_full()
                        .w(px((30.0 - 6.0) * u * level.max(0.08)))
                        .rounded(px(2.0 * u))
                        .bg(fill),
                ),
        )
        .child(
            div()
                .w(px(3.0 * u))
                .h(px(7.0 * u))
                .rounded(px(1.0 * u))
                .bg(white(0.85)),
        )
}
