//! Drawing the home screen (carousel), the startup animation, the blurred
//! backdrop, and the launch / Now Playing overlays.

use std::path::{Path, PathBuf};

use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, relative, rgb, AnyElement,
    BoxShadow, Context, Div, MouseButton, ObjectFit, SharedString, Stateful,
};
use moonglass_art::ingest::{blurred_hero_path, card_fill_image_path, card_image_path, CardCrop};
use moonglass_core::{Game, Source};

use super::icons::{icon, Icon};
use super::prefs::{Ambient as AmbientMode, CardDetails, CardFit, CardShape, Clock};
use super::theme::{
    battery, black, ease_out_cubic, hex_color, keycap, mix, on, pill, segment, white, INK, OBSIDIAN,
};
use super::{Hint, InputMode, LauncherView, Overlay, OverlayButton, StopState};
use crate::friends::playing_line;

/// Friends in a game, and a borrowed copy that is taken.
const ONLINE: u32 = 0x7BD88F;
const IN_USE: u32 = 0xF2C14E;

/// Unfocused cards relative to the focused one.
const INACTIVE_SCALE: f32 = 0.68;
/// How much unfocused cards are darkened (0 = not at all).
const INACTIVE_DIM: f32 = 0.38;
/// Cards decoded and kept on each side of the focused one (spec 5.2).
const TEXTURE_NEIGHBORS: usize = 6;

pub(super) struct Layout {
    pub focus_w: f32,
    pub focus_h: f32,
    pub base_w: f32,
    pub gap: f32,
    pub left: f32,
    pub center_y: f32,
    pub focus_top: f32,
}

fn source_label(g: &Game) -> String {
    match g.source {
        Source::Steam => "STEAM".into(),
        Source::Epic => "EPIC".into(),
        Source::Exe => "APP".into(),
        Source::Rom => g
            .platform
            .clone()
            .unwrap_or_else(|| "EMULATOR".into())
            .to_uppercase(),
    }
}

fn is_placeholder_cover(g: &Game) -> bool {
    g.cover_path
        .as_deref()
        .is_none_or(|p| moonglass_art::ingest::is_placeholder_cover(Path::new(p)))
}

fn playtime(g: &Game) -> Option<String> {
    (g.playtime_seconds >= 60).then(|| {
        let m = g.playtime_seconds / 60;
        if m >= 60 {
            format!("{}h {:02}m", m / 60, m % 60)
        } else {
            format!("{m} min")
        }
    })
}

impl LauncherView {
    pub(super) fn layout(&self, w: f32, h: f32, u: f32) -> Layout {
        let aspect = self.prefs.card_aspect();
        let mut focus_h = h * self.prefs.focus_height();
        let mut focus_w = focus_h * aspect;
        // Wide shapes are limited by the screen width instead.
        let max_w = w * 0.46;
        if focus_w > max_w {
            focus_w = max_w;
            focus_h = focus_w / aspect;
        }
        let base_w = focus_w * INACTIVE_SCALE;
        let center_y = h * 0.585;
        Layout {
            focus_w,
            focus_h,
            base_w,
            gap: base_w * self.prefs.gap_factor(),
            left: 64.0 * u,
            center_y,
            focus_top: center_y - focus_h / 2.0,
        }
    }

    /// Where the Options menu opens: right of the focused card.
    pub(super) fn options_anchor(&self, w: f32, h: f32, u: f32) -> (f32, f32) {
        let l = self.layout(w, h, u);
        let x = (l.left + l.focus_w + 24.0 * u).min(w - 480.0 * u);
        (x, l.focus_top)
    }

    /// The image for a card, per Settings → Card art, and with "Original
    /// shape" the image's own width / height, which the card takes.
    /// Artwork made before the shaped images existed falls back to the
    /// uncropped cover.
    fn image_for_card(&mut self, g: &Game) -> Option<(PathBuf, Option<f32>)> {
        let cover = PathBuf::from(g.cover_path.as_deref()?);
        let crop = match self.prefs.card_shape {
            CardShape::Poster => CardCrop::Poster,
            CardShape::Square => CardCrop::Square,
            CardShape::Banner => CardCrop::Banner,
        };
        let card = card_image_path(&cover, crop);
        let fallback = if card.is_file() { card } else { cover.clone() };
        Some(match self.prefs.card_fit {
            CardFit::Fit => (fallback, None),
            CardFit::Fill => {
                let fill = card_fill_image_path(&cover, crop);
                (if fill.is_file() { fill } else { fallback }, None)
            }
            CardFit::Original => {
                // Banners show the hero art, the other shapes the cover.
                let source = match crop {
                    CardCrop::Banner => g
                        .hero_path
                        .as_deref()
                        .map(PathBuf::from)
                        .filter(|p| p.is_file())
                        .unwrap_or(cover),
                    _ => cover,
                };
                let size = *self
                    .logo_sizes
                    .entry(source.to_string_lossy().into_owned())
                    .or_insert_with(|| moonglass_art::dimensions(&source));
                match size {
                    Some((w, h)) if w > 0 && h > 0 => (source, Some(w as f32 / h as f32)),
                    _ => (fallback, None),
                }
            }
        })
    }

    pub(super) fn render_home(
        &mut self,
        mut root: Stateful<Div>,
        w: f32,
        h: f32,
        u: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let intro = self.intro_t();
        // Intro timeline (seconds): wordmark 0–0.9, background 0.35–0.95,
        // cards from 0.6 (staggered), chrome 0.8–1.25.
        let chrome_alpha = intro.map_or(1.0, |t| ease_out_cubic(segment(t, 0.8, 0.45)));

        root = self.render_background(root, false);

        let l = self.layout(w, h, u);
        let pulse = self.pulse();

        if !self.loaded {
            // Nothing yet; the first frame stays plain.
        } else if self.library_empty() {
            root = root.child(
                div()
                    .id("add-your-games")
                    .absolute()
                    .cursor_pointer()
                    .hover(|s| s.bg(white(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_input(InputMode::Mouse, cx);
                        this.open_setup();
                        cx.notify();
                    }))
                    .left(px(l.left))
                    .top(px(l.focus_top))
                    .w(px(l.focus_w))
                    .h(px(l.focus_h))
                    .rounded(px(18.0 * u))
                    .border_2()
                    .border_color(white(0.5))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(30.0 * u))
                    .child("Add your games"),
            );
        } else if self.order.is_empty() {
            let msg = match &self.query {
                Some(q) if !q.is_empty() => format!("No games match \"{q}\""),
                _ => "Nothing here".into(),
            };
            root = root.child(
                div()
                    .absolute()
                    .left(px(l.left))
                    .top(px(l.center_y))
                    .text_size(px(32.0 * u))
                    .text_color(white(0.7))
                    .child(msg),
            );
        } else {
            root = self.render_cards(root, &l, pulse, intro, w, u, cx);
            root = self.render_hero_zone(root, &l, w, h, u, chrome_alpha);
        }

        root = root.child(self.render_top_bar(w, u, chrome_alpha, cx));
        if matches!(self.screen, super::Screen::Home) {
            root = root.child(self.render_action_bar(u, chrome_alpha, cx));
        }

        // Startup: the wordmark over black, then the screen fades in.
        if let Some(t) = intro {
            let cover = 1.0 - ease_out_cubic(segment(t, 0.35, 0.6));
            if cover > 0.001 {
                root = root.child(div().absolute().size_full().bg(black(cover)));
            }
            let mark = ease_out_cubic(segment(t, 0.0, 0.3)) * (1.0 - segment(t, 0.55, 0.35));
            if mark > 0.001 {
                root = root.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(mark)
                        .child(
                            div()
                                .text_size(px((56.0 + 8.0 * segment(t, 0.0, 0.9)) * u))
                                .font_weight(gpui::FontWeight::LIGHT)
                                .text_color(white(0.95))
                                .child("M O O N G L A S S"),
                        ),
                );
            }
        }
        root
    }

    /// What Settings and the launch / Now Playing screens sit on: only the
    /// focused game's most blurred artwork and its color, with no cards, so
    /// the whole screen is one even surface.
    pub(super) fn render_backdrop(&self, layer: Stateful<Div>) -> Stateful<Div> {
        self.render_background(layer, true)
    }

    fn render_background(&self, mut root: Stateful<Div>, backdrop: bool) -> Stateful<Div> {
        let mode = if backdrop {
            AmbientMode::Bloom
        } else {
            self.prefs.ambient
        };
        if mode == AmbientMode::Oled && !backdrop {
            return root;
        }
        if mode == AmbientMode::Motion {
            return self.render_motion_background(root);
        }
        for (amb, opacity) in [
            (self.ambient_from.clone(), 1.0 - self.crossfade),
            (self.ambient_to.clone(), self.crossfade),
        ] {
            let Some(amb) = amb else { continue };
            if opacity <= 0.001 {
                continue;
            }
            let image = match mode {
                AmbientMode::Fanart => amb.fanart.clone(),
                // The backdrop: always the strongest blur.
                _ if backdrop => amb.fanart.as_ref().map(|h| blurred_hero_path(h, 2)),
                _ => amb.background.clone(),
            };
            if let Some(bg) = image.filter(|p| p.is_file()) {
                root = root.child(
                    img(bg)
                        .absolute()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(
                            opacity
                                * if mode == AmbientMode::Fanart {
                                    0.9
                                } else {
                                    0.85
                                },
                        ),
                );
            }
            let (top, bottom): (gpui::Hsla, gpui::Hsla) = match mode {
                AmbientMode::Fanart => (black(0.15), rgb(OBSIDIAN).into()),
                _ => (amb.dominant.opacity(0.35), rgb(OBSIDIAN).into()),
            };
            let stop = if mode == AmbientMode::Fanart {
                0.98
            } else {
                0.92
            };
            root = root.child(
                div()
                    .absolute()
                    .size_full()
                    .opacity(opacity)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(top, 0.),
                        linear_color_stop(bottom, stop),
                    )),
            );
        }
        root
    }

    /// Full-screen art with motion blur. GPUI has no blur filter, so each
    /// game has a sideways-smeared copy of its art made at ingestion, laid
    /// over the sharp art. At rest a part of it shows, as streaks; when the
    /// focus moves, the new art slides in from the direction of travel,
    /// fully smeared, and sharpens as it settles. Fast scrolling keeps it
    /// smeared.
    fn render_motion_background(&self, mut root: Stateful<Div>) -> Stateful<Div> {
        /// Share of the smeared copy shown at rest.
        const REST: f32 = 0.55;
        /// How far the art slides, as a share of the screen width.
        const SLIDE: f32 = 0.06;
        // The art is wider than the screen, so sliding never shows an edge.
        const OVERSCAN: f32 = SLIDE + 0.01;
        // Cards per second at which the blur is at its strongest.
        let speed = (self.spring.velocity.abs() / 6.0).min(1.0);
        let dir = self.motion_dir;
        let settle = ease_out_cubic(self.crossfade);
        for (amb, opacity, offset) in [
            // Leaving: slides out the way the focus moved.
            (
                self.ambient_from.clone(),
                1.0 - self.crossfade,
                -dir * SLIDE * settle,
            ),
            // Arriving: slides in from the other side.
            (
                self.ambient_to.clone(),
                self.crossfade,
                dir * SLIDE * (1.0 - settle),
            ),
        ] {
            let Some(amb) = amb else { continue };
            if opacity <= 0.001 {
                continue;
            }
            let smear = (REST + (1.0 - REST) * speed.max(1.0 - settle)).min(1.0);
            let layer = |path: PathBuf| {
                img(path)
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(-OVERSCAN + offset))
                    .w(relative(1.0 + 2.0 * OVERSCAN))
                    .object_fit(ObjectFit::Cover)
            };
            if let Some(art) = amb.fanart.clone().filter(|p| p.is_file()) {
                root = root.child(layer(art).opacity(opacity * 0.9));
            }
            if let Some(motion) = amb.motion.clone().filter(|p| p.is_file()) {
                root = root.child(layer(motion).opacity(opacity * 0.9 * smear));
            }
            root = root.child(
                div()
                    .absolute()
                    .size_full()
                    .opacity(opacity)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(black(0.2), 0.),
                        linear_color_stop(rgb(OBSIDIAN), 0.98),
                    )),
            );
        }
        root
    }

    #[allow(clippy::too_many_arguments)]
    fn render_cards(
        &mut self,
        mut root: Stateful<Div>,
        l: &Layout,
        pulse: f32,
        intro: Option<f32>,
        w: f32,
        u: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let aspect = self.prefs.card_aspect();
        let details = self.prefs.card_details;
        let glow = self.prefs.glow_brightness();
        let pos = self.spring.value;
        let lo = self.selected.saturating_sub(TEXTURE_NEIGHBORS);
        let hi = (self.selected + TEXTURE_NEIGHBORS + 1).min(self.order.len());
        for slot in lo..hi {
            let g = self.shown[self.order[slot]].clone();
            // A card glides to its new place when the list changes (filter,
            // search) and fades in when it is new to the row.
            let id = g.id.as_str();
            let place = self.glide(("card-place", id), slot as f32);
            let entered = self.appear(("card", id));
            let d = place - pos;
            // 1 when focused, 0 one card away or further.
            let c = (1.0 - d.abs()).max(0.0);
            let cw = l.base_w + (l.focus_w - l.base_w) * c;
            let ch = cw / aspect;
            // Cards right of the focus are pushed by its extra width.
            let x = l.left + d * (l.base_w + l.gap) + d.clamp(0.0, 1.0) * (l.focus_w - l.base_w);
            if x > w || x + cw < 0.0 {
                continue;
            }
            // "Original shape": the card takes the art's own shape, as large
            // as fits in its slot, centered in it; the row keeps its rhythm.
            let art = self.image_for_card(&g);
            let (x, cw, ch, slot_h) = match art.as_ref().and_then(|(_, a)| *a) {
                Some(a) if a > aspect => (x, cw, cw / a, ch),
                Some(a) => {
                    let narrow = ch * a;
                    (x + (cw - narrow) / 2.0, narrow, ch, ch)
                }
                None => (x, cw, ch, ch),
            };
            // Startup: cards rise in one after another.
            let (rise, appear) = match intro {
                Some(t) => {
                    let k = slot.saturating_sub(self.selected) as f32;
                    let e = ease_out_cubic(segment(t, 0.6 + 0.07 * k, 0.55));
                    ((1.0 - e) * 80.0 * u, e)
                }
                None => (0.0, 1.0),
            };
            let rise = rise + (1.0 - entered) * 36.0 * u;
            let appear = appear * entered;
            let y = l.center_y - ch / 2.0 + rise;
            // Details sit under the slot, at the same height for every card.
            let label_top = l.center_y + slot_h / 2.0 + rise + 12.0 * u;
            // The border and glow follow the focus continuously: `c` is 1 on
            // the focused card and falls to 0 one card away.
            let focus = c;
            let accent = hex_color(g.accent_color.as_deref(), 0x5A78C8);
            let dominant = hex_color(g.dominant_color.as_deref(), 0x2A3550);
            let radius_f = 14.0 * u * (cw / l.focus_w).max(0.7);
            let radius = px(radius_f);
            // The focused card's 2 px border sits inside it, so its content
            // uses the border's inner radius to stay concentric.
            let content_radius = px(if focus > 0.01 {
                (radius_f - 2.0).max(0.0)
            } else {
                radius_f
            });

            let mut card = div()
                .id(("card", slot))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(cw))
                .h(px(ch))
                .rounded(radius)
                .opacity(appear)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.click_card(slot, window, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| this.right_click_card(slot, cx)),
                );
            if focus > 0.01 {
                card = card.border_2().border_color(white(0.92 * focus));
                let glow = glow * focus;
                if glow > 0.0 {
                    // Glow: stacked box shadows in the accent and dominant
                    // colors; the pulse animates their opacity and spread.
                    let fw = l.focus_w.min(l.focus_h);
                    card = card.shadow(vec![
                        BoxShadow {
                            color: accent.opacity((0.45 + 0.25 * pulse) * glow),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(fw * 0.22),
                            spread_radius: px(fw * (0.02 + 0.03 * pulse)),
                        },
                        BoxShadow {
                            color: dominant.opacity((0.35 + 0.2 * pulse) * glow),
                            offset: point(px(0.), px(fw * 0.05)),
                            blur_radius: px(fw * 0.55),
                            spread_radius: px(fw * (0.06 + 0.06 * pulse)),
                        },
                        BoxShadow {
                            color: accent.opacity(0.18 * glow),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(fw * 0.9),
                            spread_radius: px(fw * 0.1),
                        },
                    ]);
                }
            } else {
                card = card.hover(|s| s.border_1().border_color(white(0.45)));
            }

            let mut inner = div()
                .size_full()
                .rounded(content_radius)
                .overflow_hidden()
                .relative();
            if let Some((path, _)) = art {
                // The image is rounded itself (clipping by the parent is
                // rectangular) and already has the card's shape, so its
                // corners are the card's corners.
                inner = inner.child(
                    img(path)
                        .absolute()
                        .size_full()
                        .rounded(content_radius)
                        .object_fit(ObjectFit::Cover),
                );
            }
            if is_placeholder_cover(&g) && self.prefs.card_shape != CardShape::Banner {
                inner = inner.child(
                    div()
                        .absolute()
                        .size_full()
                        .p(px(18.0 * u))
                        .flex()
                        .items_end()
                        .text_size(px(26.0 * u * (cw / l.focus_w)))
                        .child(SharedString::from(g.title.clone())),
                );
            }
            // Friends playing this game, and a borrowed copy in use.
            let friends = self.friends.playing(&g).len();
            let in_use = self.friends.in_use_by(&g).is_some();
            if friends > 0 || in_use {
                let tag = |text: String, color: u32| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0 * u))
                        .px(px(8.0 * u))
                        .py(px(2.0 * u))
                        .rounded(px(6.0 * u))
                        .bg(black(0.7))
                        .text_color(rgb(color))
                        .text_size(px(13.0 * u))
                        .child(text)
                };
                let mut tags = div()
                    .absolute()
                    .top(px(8.0 * u))
                    .left(px(8.0 * u))
                    .flex()
                    .gap(px(6.0 * u));
                if friends > 0 {
                    // An online dot drawn as a circle, not a text bullet.
                    let dot = div().size(px(7.0 * u)).rounded_full().bg(rgb(ONLINE));
                    tags = tags.child(tag(friends.to_string(), ONLINE).child(dot));
                }
                if in_use {
                    tags = tags.child(tag("In use".into(), IN_USE));
                }
                inner = inner.child(tags);
            }
            // Corner badges: favorite, hidden.
            if g.is_favorite || self.hidden_ids.contains(&g.id) {
                let mut badges = div()
                    .absolute()
                    .top(px(8.0 * u))
                    .right(px(8.0 * u))
                    .flex()
                    .gap(px(6.0 * u));
                if self.hidden_ids.contains(&g.id) {
                    badges = badges.child(
                        div()
                            .px(px(8.0 * u))
                            .py(px(2.0 * u))
                            .rounded(px(6.0 * u))
                            .bg(black(0.7))
                            .text_size(px(13.0 * u))
                            .child("Hidden"),
                    );
                }
                if g.is_favorite {
                    badges = badges.child(
                        div()
                            .size(px(24.0 * u))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0 * u))
                            .bg(black(0.7))
                            .child(icon(Icon::Star, 15.0 * u, rgb(0xF5C451))),
                    );
                }
                inner = inner.child(badges);
            }
            let dim = INACTIVE_DIM * (1.0 - c)
                + if self.hidden_ids.contains(&g.id) {
                    0.25
                } else {
                    0.0
                };
            if dim > 0.001 {
                // Dim with a black layer rather than transparency, so the
                // background never shows through the card.
                inner = inner.child(
                    div()
                        .absolute()
                        .size_full()
                        .rounded(content_radius)
                        .bg(black(dim)),
                );
            }
            root = root.child(card.child(inner));

            // Card details under the card.
            if details != CardDetails::ArtOnly {
                let mut label = div()
                    .absolute()
                    .left(px(x))
                    .top(px(label_top))
                    .w(px(cw))
                    .opacity(appear * (0.65 + 0.35 * c))
                    .flex()
                    .flex_col()
                    .gap(px(4.0 * u))
                    .text_size(px((15.0 + 3.0 * c) * u))
                    .child(div().truncate().child(SharedString::from(g.title.clone())));
                if details == CardDetails::ArtPlaytime {
                    let mut meta = source_label(&g);
                    if let Some(p) = playtime(&g) {
                        meta = format!("{meta} · {p}");
                    }
                    label = label.child(
                        div()
                            .text_size(px(13.0 * u))
                            .text_color(white(0.6))
                            .truncate()
                            .child(meta),
                    );
                }
                root = root.child(label);
            }
        }
        root
    }

    fn render_hero_zone(
        &mut self,
        root: Stateful<Div>,
        l: &Layout,
        w: f32,
        h: f32,
        u: f32,
        alpha: f32,
    ) -> Stateful<Div> {
        let logo = self
            .current()
            .and_then(|g| g.logo_path.clone())
            .and_then(|p| {
                let size = *self
                    .logo_sizes
                    .entry(p.clone())
                    .or_insert_with(|| moonglass_art::dimensions(Path::new(&p)));
                size.map(|s| (p, s))
            });
        let Some(g) = self.current().cloned() else {
            return root;
        };
        // The title slides in when the focus moves to another game.
        let shown = self.appear(("hero", g.id.as_str()));
        let zone_top = h * 0.16;
        let mut hero = div()
            .absolute()
            .left(px(l.left - (1.0 - shown) * 28.0 * u))
            .top(px(zone_top))
            .h(px((l.focus_top - zone_top - 16.0 * u).max(40.0 * u)))
            .opacity(alpha * shown)
            .flex()
            .flex_col()
            .items_start()
            .justify_end()
            .gap(px(10.0 * u));
        hero = match logo {
            Some((path, (lw, lh))) => {
                // Fit inside 34% width x 11% height, keep aspect.
                let (max_w, max_h) = (w * 0.34, h * 0.11);
                let k = (max_w / lw.max(1) as f32).min(max_h / lh.max(1) as f32);
                hero.child(
                    img(PathBuf::from(path))
                        .w(px(lw as f32 * k))
                        .h(px(lh as f32 * k)),
                )
            }
            None => hero.child(
                div()
                    .text_size(px(50.0 * u))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(SharedString::from(g.title.clone())),
            ),
        };
        let mut meta = div().flex().gap(px(14.0 * u)).items_center().child(
            div()
                .px(px(10.0 * u))
                .py(px(3.0 * u))
                .rounded(px(6.0 * u))
                .bg(white(0.16))
                .text_size(px(16.0 * u))
                .child(source_label(&g)),
        );
        if let Some(p) = playtime(&g) {
            meta = meta.child(
                div()
                    .text_size(px(18.0 * u))
                    .text_color(white(0.7))
                    .child(format!("{p} in Moonglass")),
            );
        }
        let note = |text: String, color: u32| {
            div()
                .text_size(px(18.0 * u))
                .text_color(rgb(color))
                .child(text)
        };
        if let Some(line) = playing_line(self.friends.playing(&g)) {
            meta = meta.child(note(line, ONLINE));
        }
        if let Some(owner) = self.friends.in_use_by(&g) {
            meta = meta.child(note(format!("In use by {}", owner.name), IN_USE));
        }
        root.child(hero.child(meta))
    }

    fn render_top_bar(&self, w: f32, u: f32, alpha: f32, cx: &mut Context<Self>) -> Div {
        // Filter pills (clickable).
        let mut pills = div().flex().gap(px(6.0 * u));
        for (i, f) in self.filters.iter().copied().enumerate() {
            let active = f == self.filter && self.query.is_none();
            let a = self.glide(("pill", f.label()), on(active));
            pills = pills.child(
                div()
                    .id(("pill", i))
                    .px(px(18.0 * u))
                    .py(px(6.0 * u))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(white(0.92 * a))
                    .text_color(mix(white(0.62), rgb(INK).into(), a))
                    .when(!active, |d| {
                        d.hover(|s| s.bg(white(0.12)).text_color(white(0.95)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.click_filter(f, cx)))
                    .child(f.label()),
            );
        }

        // Search box, top right.
        // Focused while typing, on a physical keyboard or the on-screen one.
        let on_screen_keyboard = matches!(
            &self.modal,
            super::Modal::Keyboard(k) if k.target == super::TextTarget::Search
        );
        let focused = self.search_focused && (self.is_home_idle() || on_screen_keyboard);
        let query = self.query.clone().unwrap_or_default();
        let caret = focused && self.caret_on();
        // The box widens and lights up as it takes focus.
        let lit = self.glide("search-focus", on(focused));
        let search = div()
            .id("search-box")
            .w(px(
                (w * 0.22).clamp(260.0 * u, 420.0 * u) * (1.0 + 0.3 * lit)
            ))
            .h(px(42.0 * u))
            .px(px(16.0 * u))
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(10.0 * u))
            .cursor_text()
            .bg(white(0.09 + 0.09 * lit))
            .border_1()
            .border_color(white(0.65 * lit))
            .when(!focused, |d| d.hover(|s| s.bg(white(0.14))))
            .on_click(cx.listener(|this, _, _, cx| {
                this.set_input(InputMode::Mouse, cx);
                this.click_search_box(cx);
            }))
            .child(icon(Icon::Search, 18.0 * u, white(0.6)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .text_size(px(18.0 * u))
                    .child(
                        div()
                            .truncate()
                            .when(query.is_empty(), |d| d.text_color(white(0.5)))
                            .child(SharedString::from(if query.is_empty() && !focused {
                                "Search".to_string()
                            } else {
                                query.clone()
                            })),
                    )
                    // Blinking caret while the field has focus.
                    .child(
                        div()
                            .ml(px(1.0 * u))
                            .w(px(2.0 * u))
                            .h(px(22.0 * u))
                            .when(caret, |d| d.bg(white(0.95))),
                    ),
            )
            .when(self.query.is_some(), |d| {
                d.child(
                    div()
                        .id("search-clear")
                        .size(px(28.0 * u))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .opacity(0.7)
                        .hover(|s| s.bg(white(0.15)).opacity(1.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.clear_search();
                            cx.notify();
                        }))
                        .child(icon(Icon::Close, 14.0 * u, white(1.0))),
                )
            })
            .child(
                div()
                    .id("search-osk")
                    .w(px(34.0 * u))
                    .h(px(28.0 * u))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0 * u))
                    .cursor_pointer()
                    .opacity(0.6)
                    .hover(|s| s.bg(white(0.15)).opacity(1.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        if this.is_home_idle() {
                            this.open_search_keyboard();
                            cx.notify();
                        }
                    }))
                    .child(icon(Icon::Keyboard, 22.0 * u, white(1.0))),
            );

        // Controller status: battery where the OS reports it.
        let status: Div = {
            let pad = &self.controller;
            let connected = pad.connected || self.xinput_power.is_some();
            let mut s = div()
                .flex()
                .items_center()
                .gap(px(10.0 * u))
                .text_size(px(18.0 * u));
            if !connected {
                s = s.text_color(white(0.5)).child("No controller");
            } else {
                s = s.text_color(white(0.85)).child("Controller");
                match (pad.battery, self.xinput_power) {
                    (Some(p), _) => {
                        s = s.child(battery(p as f32 / 100.0, u));
                        if pad.charging {
                            s = s.child(
                                div()
                                    .text_size(px(14.0 * u))
                                    .text_color(white(0.7))
                                    .child("charging"),
                            );
                        }
                    }
                    (None, Some(moonglass_platform::PadPower::Level(l))) => {
                        s = s.child(battery(l as f32 / 3.0, u));
                    }
                    (None, Some(moonglass_platform::PadPower::Wired)) => {
                        s = s.child(
                            div()
                                .text_size(px(14.0 * u))
                                .text_color(white(0.7))
                                .child("wired"),
                        );
                    }
                    (None, None) => {}
                }
            }
            s
        };
        let clock = match self.prefs.clock {
            Clock::H24 => chrono::Local::now().format("%H:%M").to_string(),
            Clock::H12 => chrono::Local::now().format("%-I:%M %p").to_string(),
        };
        let gear = div()
            .id("gear")
            .size(px(40.0 * u))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(white(0.14)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.set_input(InputMode::Mouse, cx);
                this.input_action(super::gamepad::NavAction::OpenSettings, window, cx);
            }))
            .child(icon(Icon::Gear, 24.0 * u, white(0.9)));

        div()
            .absolute()
            .top(px(30.0 * u))
            .left(px(64.0 * u))
            .right(px(64.0 * u))
            .opacity(alpha)
            .flex()
            .items_center()
            .justify_between()
            .child(pills)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(24.0 * u))
                    .child(search)
                    .child(status)
                    .child(div().text_color(white(0.85)).child(clock))
                    .child(gear),
            )
    }

    fn render_action_bar(&self, u: f32, alpha: f32, cx: &mut Context<Self>) -> Div {
        use super::gamepad::Family;
        let f = self.controller.family;
        let [a, b, x, y] = match f {
            Family::Xbox => ["A", "B", "X", "Y"],
            Family::PlayStation => ["✕", "○", "□", "△"],
            Family::Nintendo => ["A", "B", "Y", "X"],
        };
        let searching = self.query.is_some();
        let mut hints: Vec<(&str, &str, Hint)> = Vec::new();
        if self.input == InputMode::Gamepad {
            hints.push((a, "Play", Hint::Play));
            if searching {
                hints.push((b, "Clear", Hint::Back));
            }
            hints.extend([
                (x, "Search", Hint::Search),
                (y, "Options", Hint::Options),
                ("LB RB", "Filter", Hint::Filter),
                ("≡", "Settings", Hint::Settings),
            ]);
            if !searching {
                hints.push((b, "Hold to exit", Hint::Exit));
            }
        } else {
            hints.push(("Enter", "Play", Hint::Play));
            if searching {
                hints.push(("Esc", "Clear", Hint::Back));
            }
            hints.extend([
                ("Ctrl+O", "Options", Hint::Options),
                ("Tab", "Filter", Hint::Filter),
                ("Ctrl+,", "Settings", Hint::Settings),
            ]);
            if !searching {
                hints.push(("Esc", "Hold to exit", Hint::Exit));
            }
        }
        let mut bar = div()
            .absolute()
            .bottom(px(36.0 * u))
            .left(px(64.0 * u))
            .right(px(64.0 * u))
            .opacity(alpha)
            .flex()
            .justify_end()
            .gap(px(12.0 * u));
        for (i, (glyph, label, action)) in hints.into_iter().enumerate() {
            bar =
                bar.child(
                    div()
                        .id(("hint", i))
                        .flex()
                        .items_center()
                        .gap(px(10.0 * u))
                        .px(px(10.0 * u))
                        .py(px(6.0 * u))
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|s| s.bg(white(0.12)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.hint_action(action, window, cx)
                        }))
                        .child(keycap(glyph, u))
                        .child(div().text_color(white(0.85)).child(label.to_string())),
                );
        }
        bar
    }

    /// Launching, Now Playing, and messages.
    pub(super) fn render_overlay(
        &self,
        w: f32,
        h: f32,
        u: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let gamepad = self.input == InputMode::Gamepad;
        let (k_ok, k_back, k_stop) = if gamepad {
            ("A", "B", "X")
        } else {
            ("Enter", "Esc", "Del")
        };
        let title = self.session_title();
        let button = |id: &'static str,
                      key: &str,
                      label: String,
                      b: OverlayButton,
                      emph: bool,
                      enabled: bool,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .when(enabled, |d| {
                    d.cursor_pointer().hover(|s| s.opacity(0.85)).on_click(
                        cx.listener(move |this, _, window, cx| this.overlay_click(b, window, cx)),
                    )
                })
                .when(!enabled, |d| d.opacity(0.5))
                .child(
                    pill("", u, emph)
                        .flex()
                        .items_center()
                        .gap(px(12.0 * u))
                        .child(keycap(key, u))
                        .child(label),
                )
        };

        let content: Div = match &self.overlay {
            Overlay::None => return None,
            Overlay::Launching { still_waiting } => {
                let mut c = div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(24.0 * u))
                    .child(
                        div()
                            .text_size(px(44.0 * u))
                            .child(format!("Launching {title}…")),
                    );
                if *still_waiting {
                    c = c
                        .child(
                            div()
                                .text_color(white(0.7))
                                .child("Still waiting… Steam may be updating the game."),
                        )
                        .child(button(
                            "ov-cancel",
                            k_back,
                            "Cancel".into(),
                            OverlayButton::Cancel,
                            false,
                            true,
                            cx,
                        ));
                }
                c
            }
            Overlay::Playing { stop } => {
                let since = self
                    .session
                    .as_ref()
                    .and_then(|s| s.running_since)
                    .map(|t| t.elapsed().as_secs() / 60)
                    .unwrap_or(0);
                let playing_for = if since == 0 {
                    "Started just now".to_string()
                } else {
                    format!("Playing for {since} min")
                };
                let logo = self.session.as_ref().and_then(|s| {
                    let p = s.game.logo_path.clone()?;
                    let (lw, lh) = moonglass_art::dimensions(Path::new(&p))?;
                    Some((p, lw, lh))
                });
                let headline: AnyElement = match logo {
                    Some((p, lw, lh)) => {
                        let k = ((w * 0.45) / lw.max(1) as f32).min((h * 0.18) / lh.max(1) as f32);
                        img(PathBuf::from(p))
                            .w(px(lw as f32 * k))
                            .h(px(lh as f32 * k))
                            .into_any_element()
                    }
                    None => div()
                        .text_size(px(64.0 * u))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(title.clone())
                        .into_any_element(),
                };
                let (stop_label, stop_enabled, stop_note) = match stop {
                    StopState::Idle => ("Stop game".to_string(), true, None),
                    StopState::Requested(_) => (
                        "Stopping…".to_string(),
                        false,
                        Some(format!(
                            "Asked {title} to close. Unsaved progress may be lost."
                        )),
                    ),
                    StopState::CanForce => (
                        "Force stop".to_string(),
                        true,
                        Some(format!(
                            "{title} is still running. Force stop ends it immediately."
                        )),
                    ),
                    StopState::Forcing => ("Stopping…".to_string(), false, None),
                };
                let mut c = div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(22.0 * u))
                    .child(
                        div()
                            .text_size(px(16.0 * u))
                            .text_color(white(0.7))
                            .child("N O W   P L A Y I N G"),
                    )
                    .child(headline)
                    .child(div().text_color(white(0.75)).child(playing_for))
                    .child(
                        div()
                            .mt(px(14.0 * u))
                            .flex()
                            .gap(px(18.0 * u))
                            .child(button(
                                "ov-return",
                                k_ok,
                                "Return to game".into(),
                                OverlayButton::ReturnToGame,
                                true,
                                true,
                                cx,
                            ))
                            .child(button(
                                "ov-stop",
                                k_stop,
                                stop_label,
                                OverlayButton::Stop,
                                false,
                                stop_enabled,
                                cx,
                            )),
                    );
                if let Some(n) = stop_note {
                    c = c.child(
                        div()
                            .text_size(px(17.0 * u))
                            .text_color(white(0.65))
                            .child(n),
                    );
                }
                c
            }
            Overlay::Message(m) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(24.0 * u))
                .child(div().text_size(px(40.0 * u)).child(m.clone()))
                .child(button(
                    "ov-ok",
                    k_ok,
                    "OK".into(),
                    OverlayButton::Dismiss,
                    true,
                    true,
                    cx,
                )),
        };

        let backdrop = matches!(
            self.overlay,
            Overlay::Launching { .. } | Overlay::Playing { .. }
        );
        // Each kind of overlay fades in and settles upward.
        let shown = self.appear(("overlay", std::mem::discriminant(&self.overlay)));
        Some(
            div()
                .id("overlay")
                .occlude()
                .absolute()
                .size_full()
                .opacity(shown)
                // Over the blurred backdrop only a light scrim is needed;
                // the content sits on its own soft panel to stay readable.
                .bg(black(if backdrop { 0.45 } else { 0.82 }))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    content
                        .relative()
                        .top(px((1.0 - shown) * 30.0 * u))
                        .px(px(64.0 * u))
                        .py(px(44.0 * u))
                        .rounded(px(32.0 * u))
                        .when(backdrop, |d| {
                            d.bg(black(0.42)).border_1().border_color(white(0.08))
                        }),
                )
                .into_any_element(),
        )
    }
}
