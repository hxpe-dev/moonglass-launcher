//! Shared drawing helpers: colors, easing, key caps and buttons.

use gpui::{div, prelude::*, px, rgb, Div, Hsla, Rgba};

use super::icons::{icon, keycap_icons};

pub const OBSIDIAN: u32 = 0x07080B;
pub const INK: u32 = 0x101114;

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
    el.bg(white(0.92 * amount))
        .text_color(mix(white(0.95), rgb(INK).into(), amount))
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
        .bg(white(0.9))
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
