//! omalive's visual kit: Omarchy Quattro panel language (ruled sections,
//! letter-spaced uppercase headings, square switches, thin round sliders,
//! bordered choices) with btop-style dot graphs. Every colour comes from the
//! live Omarchy theme, including the palette slots gpui-omarchy does not
//! carry (blue, magenta, cyan, orange), which [`ThemeSlots`] reads from the
//! theme's `colors.toml`.
use gpui_kit::{
    AnyElement, App, BoxShadow, Div, ElementId, FocusHandle, FontWeight, Global, Hsla,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Rgba, ScrollWheelEvent,
    SharedString, Stateful, TextAlign, TextRun, Window, canvas, div, fill, font, linear_color_stop,
    linear_gradient, point, prelude::*, px, relative, rems, size,
};
use gpui_kit::{Bounds, ClickEvent, Corners, RenderImage};
use gpui_omarchy::{ActiveTheme, Theme};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

/// Dot pitch and dot size of the graphs, in logical pixels.
pub const PITCH: f32 = 3.;
const DOT: f32 = 2.;
/// Letter spacing of uppercase labels, as a fraction of the font size.
const TRACKING: f32 = 0.16;

/// Palette slots beyond gpui-omarchy's semantic theme, read from the active
/// theme's `colors.toml`. Missing slots fall back to colours derived from
/// the theme, so ANSI-only and future themes still work.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeSlots {
    pub blue: Option<Hsla>,
    pub magenta: Option<Hsla>,
    pub cyan: Option<Hsla>,
    pub orange: Option<Hsla>,
    pub muted: Option<Hsla>,
    pub dim: Option<Hsla>,
}

impl Global for ThemeSlots {}

impl ThemeSlots {
    /// Parse the extra slots from a `colors.toml`; unknown or malformed
    /// values are ignored.
    pub fn parse(contents: &str) -> Self {
        let table: toml::Table = toml::from_str(contents).unwrap_or_default();
        let color = |keys: &[&str]| {
            keys.iter().find_map(|key| {
                let hex = table.get(*key)?.as_str()?.strip_prefix('#')?;
                (hex.len() == 6)
                    .then(|| u32::from_str_radix(hex, 16).ok())
                    .flatten()
                    .map(|v| Hsla::from(gpui_kit::rgb(v)))
            })
        };
        Self {
            blue: color(&["blue", "color4"]),
            magenta: color(&["magenta", "color5"]),
            cyan: color(&["cyan", "color6"]),
            orange: color(&["orange"]),
            muted: color(&["muted", "color8"]),
            dim: color(&["dark_foreground"]),
        }
    }

    /// The active theme's slots: `~/.local/state/omarchy/current`, or the
    /// older `~/.config/omarchy/current`.
    pub fn load() -> Self {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return Self::default();
        };
        [".local/state/omarchy/current", ".config/omarchy/current"]
            .iter()
            .find_map(|dir| std::fs::read_to_string(home.join(dir).join("theme/colors.toml")).ok())
            .map(|contents| Self::parse(&contents))
            .unwrap_or_default()
    }
}

/// Read the theme's extra slots now and whenever gpui-omarchy reloads the theme.
pub fn init(cx: &mut App) {
    cx.set_global(ThemeSlots::load());
    cx.observe_global::<Theme>(|cx| cx.set_global(ThemeSlots::load()))
        .detach();
}

#[derive(Clone)]
pub struct Palette {
    pub bg: Hsla,
    pub fg: Hsla,
    /// Brightest text: titles and values.
    pub title: Hsla,
    /// Labels and secondary text.
    pub dim: Hsla,
    /// Section dividers and grid lines.
    pub line: Hsla,
    /// Control borders.
    pub muted: Hsla,
    pub hi: Hsla,
    pub blu: Hsla,
    pub mag: Hsla,
    pub red: Hsla,
    pub grn: Hsla,
    pub yel: Hsla,
    pub org: Hsla,
    pub cyn: Hsla,
    /// Hover fill.
    pub fill: Hsla,
    /// Selected rows and slider tracks.
    pub fill2: Hsla,
    /// Selected controls.
    pub accfill: Hsla,
    pub mono: SharedString,
}

impl Palette {
    pub fn of(t: &Theme, slots: &ThemeSlots) -> Self {
        let muted = slots.muted.unwrap_or(t.border);
        Self {
            bg: t.background,
            fg: t.foreground,
            title: t.bright,
            dim: slots
                .dim
                .unwrap_or_else(|| mix(t.foreground, t.background, 0.45)),
            line: mix(t.background, muted, 0.5),
            muted,
            hi: t.accent,
            blu: slots.blue.unwrap_or(t.accent),
            mag: slots
                .magenta
                .unwrap_or_else(|| mix(t.accent, t.danger, 0.45)),
            red: t.danger,
            grn: t.success,
            yel: t.warning,
            org: slots
                .orange
                .unwrap_or_else(|| mix(t.warning, t.danger, 0.5)),
            cyn: slots.cyan.unwrap_or_else(|| mix(t.accent, t.success, 0.5)),
            fill: t.foreground.opacity(0.08),
            fill2: t.foreground.opacity(0.16),
            accfill: t.accent.opacity(0.12),
            mono: t.mono_font.clone(),
        }
    }

    /// Waveform gradient by loudness: cyan → blue → magenta → red.
    pub fn signal(&self) -> [Hsla; 4] {
        [self.cyn, self.blu, self.mag, self.red]
    }

    /// The theme's hues in spectral order, laid across frequency so every
    /// band of a voice shows its own colour rather than only its loud peaks.
    pub fn spectrum(&self) -> [Hsla; 6] {
        [self.cyn, self.blu, self.mag, self.red, self.org, self.yel]
    }

    /// One colour per voice-chain stage, input to output.
    pub fn stages(&self) -> [Hsla; 6] {
        [self.cyn, self.blu, self.grn, self.yel, self.org, self.mag]
    }

    /// Level gradient for meters: green → yellow → red.
    pub fn level(&self) -> [Hsla; 3] {
        [self.grn, self.yel, self.red]
    }

    /// Slider fill: accent → magenta.
    pub fn control(&self) -> [Hsla; 2] {
        [self.hi, self.mag]
    }

    /// EQ band markers, low to high.
    pub fn bands(&self) -> [Hsla; 5] {
        [self.cyn, self.blu, self.grn, self.yel, self.mag]
    }
}

pub fn palette(cx: &App) -> Palette {
    match cx.try_global::<ThemeSlots>() {
        Some(slots) => Palette::of(cx.omarchy(), slots),
        None => Palette::of(cx.omarchy(), &ThemeSlots::default()),
    }
}

/// Linear mix in RGB space.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

/// Colour at `t` (0…1) along evenly spaced stops.
pub fn gradient(stops: &[Hsla], t: f32) -> Hsla {
    let t = t.clamp(0., 1.);
    if stops.len() < 2 {
        return stops.first().copied().unwrap_or_default();
    }
    let position = t * (stops.len() - 1) as f32;
    let i = (position.floor() as usize).min(stops.len() - 2);
    mix(stops[i], stops[i + 1], position - i as f32)
}

// ---- text ---------------------------------------------------------------

/// Uppercase, letter-spaced text in coloured runs, as Quattro's headings.
/// GPUI has no letter spacing, so the line is shaped with every glyph forced
/// to the mono advance plus tracking: one element, one shaped line.
pub fn tracked(runs: Vec<(String, Hsla)>, size_rems: f32, window: &Window, cx: &App) -> AnyElement {
    let p = palette(cx);
    let font_size = window.rem_size() * size_rems;
    let face = font(p.mono.clone());
    let advance = window
        .text_system()
        .em_advance(window.text_system().resolve_font(&face), font_size)
        .unwrap_or(font_size * 0.6);
    let cell = advance + font_size * TRACKING;
    let mut text = String::new();
    let mut styles = Vec::new();
    for (part, color) in runs {
        let part = part.to_uppercase();
        styles.push(TextRun {
            len: part.len(),
            font: face.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        text.push_str(&part);
    }
    let count = text.chars().count() as f32;
    let line_height = font_size * 1.4;
    let text = SharedString::from(text);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let line =
                window
                    .text_system()
                    .shape_line(text.clone(), font_size, &styles, Some(cell));
            let _ = line.paint(
                bounds.origin,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        },
    )
    .flex_shrink_0()
    .w(cell * count - font_size * TRACKING)
    .h(line_height)
    .into_any_element()
}

/// A dim letter-spaced label.
pub fn label(text: impl Into<String>, window: &Window, cx: &App) -> AnyElement {
    tracked(vec![(text.into(), palette(cx).dim)], 0.68, window, cx)
}

/// A section heading: letter-spaced label left, value right.
pub fn section(
    title: impl Into<String>,
    right: Option<AnyElement>,
    window: &Window,
    cx: &App,
) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(rems(1.))
        .mb(rems(0.7))
        .child(tracked(vec![(title.into(), p.dim)], 0.7, window, cx))
        .children(right.map(|right| {
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(p.fg)
                .child(right)
        }))
}

/// A thin rule between sections.
pub fn divider(cx: &App) -> Div {
    div()
        .h(px(1.))
        .flex_shrink_0()
        .my(rems(1.05))
        .bg(palette(cx).line)
}

/// A small square status mark: filled when on, hollow when off.
pub fn dot(color: Hsla, filled: bool) -> Div {
    div()
        .size(rems(0.45))
        .flex_shrink_0()
        .when(filled, |d| d.bg(color))
        .when(!filled, |d| d.border_1().border_color(color))
}

// ---- graphs -------------------------------------------------------------

/// Images drawn once and reused across frames, such as a graph's dot mask.
/// A replaced image is released from the atlas.
#[derive(Clone, Default)]
pub struct Images(Rc<RefCell<HashMap<&'static str, Cached>>>);

/// An image and the key it was drawn for.
type Cached = (u64, Arc<RenderImage>);

impl Images {
    /// The image cached under `name` for `key`, drawn by `draw` into BGRA
    /// pixels of `width`×`height` when missing or stale.
    pub fn get(
        &self,
        name: &'static str,
        key: u64,
        (width, height): (u32, u32),
        window: &mut Window,
        draw: impl FnOnce(&mut [u8]),
    ) -> Option<Arc<RenderImage>> {
        let mut images = self.0.borrow_mut();
        if let Some((cached, image)) = images.get(name)
            && *cached == key
        {
            return Some(image.clone());
        }
        let mut pixels = vec![0; width as usize * height as usize * 4];
        draw(&mut pixels);
        let buffer =
            image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(width, height, pixels)?;
        let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buffer
        )]));
        if let Some((_, old)) = images.insert(name, (key, image.clone())) {
            let _ = window.drop_image(old);
        }
        Some(image)
    }
}

pub fn bgra(color: Hsla) -> [u8; 4] {
    let c = Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255. + 0.5) as u8;
    [byte(c.b), byte(c.g), byte(c.r), byte(c.a)]
}

pub fn hash(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Where a graph's dot rows start: at its bottom edge (bars) or its top
/// edge (waveforms, which grow from a centre line on the top grid).
#[derive(Clone, Copy, Hash)]
pub enum Anchor {
    Top,
    Bottom,
}

/// Lay a background-coloured grid with transparent dot holes over
/// `bounds`, splitting everything painted beneath into dots. One cached
/// image replaces a mask quad per row and lets a bar span many columns.
pub fn paint_dot_mask(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    background: Hsla,
    anchor: Anchor,
    images: &Images,
    name: &'static str,
) {
    let scale = window.scale_factor();
    let (width, height) = (bounds.size.width / px(1.), bounds.size.height / px(1.));
    let (w, h) = (
        (width * scale).round() as u32,
        (height * scale).round() as u32,
    );
    if w == 0 || h == 0 {
        return;
    }
    let color = bgra(background);
    let key = hash((w, h, color, scale.to_bits(), anchor));
    let image = images.get(name, key, (w, h), window, |pixels| {
        draw_dot_mask(pixels, w, scale, height, anchor, color)
    });
    if let Some(image) = image {
        let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
    }
}

/// Fill BGRA `pixels` (`width` device pixels per row) with the mask for a
/// graph `height` logical pixels tall: transparent dot holes, `color` grid.
fn draw_dot_mask(
    pixels: &mut [u8],
    width: u32,
    scale: f32,
    height: f32,
    anchor: Anchor,
    color: [u8; 4],
) {
    // Pixel centres in logical units decide hole or grid.
    let hole = |position: f32| position.rem_euclid(PITCH) < DOT;
    let columns: Vec<bool> = (0..width).map(|x| hole((x as f32 + 0.5) / scale)).collect();
    for (y, line) in pixels.chunks_exact_mut(width as usize * 4).enumerate() {
        let from_top = (y as f32 + 0.5) / scale;
        let row = match anchor {
            Anchor::Top => hole(from_top),
            Anchor::Bottom => hole(height - from_top - (PITCH - DOT)),
        };
        for (pixel, open) in line.chunks_exact_mut(4).zip(&columns) {
            if !(row && *open) {
                pixel.copy_from_slice(&color);
            }
        }
    }
}

/// Bars rising from the bottom of `bounds`, one per value (0…1), each a
/// single gradient quad spanning as many dot columns as its share of the
/// width; [`paint_dot_mask`] splits them into dots. Each column takes its
/// hue from its place along `hues` (left to right) and brightens towards
/// `highlight` as it rises.
pub fn paint_hued_bars(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    values: &[f32],
    hues: &[Hsla],
    highlight: Hsla,
    alpha: f32,
) {
    let count = column_count(bounds);
    let mut pooled = Vec::new();
    let values = if values.len() > count {
        resample(values, count, &mut pooled);
        &pooled[..]
    } else {
        values
    };
    let height = bounds.size.height / px(1.);
    let rows = (height / PITCH).floor();
    for (i, value) in values.iter().enumerate() {
        let lit = (value.clamp(0., 1.) * rows).round();
        if lit < 1. {
            continue;
        }
        let (from, to) = span(i, values.len(), count);
        let hue = gradient(hues, (from + to) as f32 / 2. / count as f32);
        let top = mix(hue, highlight, 0.45 * lit / rows);
        let h = lit * PITCH;
        window.paint_quad(fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(from as f32 * PITCH),
                    bounds.origin.y + px(height - h),
                ),
                size(px((to - from) as f32 * PITCH), px(h)),
            ),
            linear_gradient(
                0.,
                linear_color_stop(top.opacity(alpha), 1.),
                linear_color_stop(hue.opacity(alpha * 0.7), 0.),
            ),
        ));
    }
}

/// One-pixel marks at each value's height, spanning the same columns as
/// [`paint_hued_bars`].
pub fn paint_marks(window: &mut Window, bounds: Bounds<Pixels>, values: &[f32], color: Hsla) {
    let count = column_count(bounds);
    let mut pooled = Vec::new();
    let values = if values.len() > count {
        resample(values, count, &mut pooled);
        &pooled[..]
    } else {
        values
    };
    let height = bounds.size.height / px(1.);
    let rows = (height / PITCH).floor();
    for (i, value) in values.iter().enumerate() {
        let lit = (value.clamp(0., 1.) * rows).round();
        if lit < 1. {
            continue;
        }
        let (from, to) = span(i, values.len(), count);
        window.paint_quad(fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(from as f32 * PITCH),
                    bounds.origin.y + px(height - lit * PITCH),
                ),
                size(px((to - from) as f32 * PITCH), px(1.)),
            ),
            color,
        ));
    }
}

/// The dot columns `[from, to)` that value `i` of `n` covers across `count`.
fn span(i: usize, n: usize, count: usize) -> (usize, usize) {
    let from = i * count / n;
    (from, ((i + 1) * count / n).max(from + 1))
}

/// A waveform: one quad per column mirrored around the centre line,
/// coloured by its amplitude along `stops`; `reach` stretches the gradient
/// so ordinary speech, not only peaks, reaches the warm end. Pair with an
/// [`Anchor::Top`] mask.
pub fn paint_wave(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    values: &[f32],
    stops: &[Hsla],
    alpha: f32,
    reach: f32,
) {
    let half = (bounds.size.height / px(1.) / 2. / PITCH).floor();
    let centre = bounds.origin.y + px(half * PITCH);
    for (c, value) in values.iter().enumerate() {
        let lit = (value.clamp(0., 1.) * half)
            .round()
            .max(if *value > 0.002 { 1. } else { 0. });
        if lit < 1. {
            continue;
        }
        let h = px(lit * PITCH);
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(c as f32 * PITCH), centre - h),
                size(px(DOT), h * 2.),
            ),
            gradient(stops, lit / half * reach).opacity(alpha),
        ));
    }
}

/// How many graph columns fit in `bounds`.
pub fn column_count(bounds: Bounds<Pixels>) -> usize {
    ((bounds.size.width / px(1.)) / PITCH).floor().max(1.) as usize
}

/// Resample `values` to `count` columns (nearest, with max-pooling when
/// shrinking so narrow peaks survive).
pub fn resample(values: &[f32], count: usize, out: &mut Vec<f32>) {
    out.clear();
    if values.is_empty() {
        out.resize(count, 0.);
        return;
    }
    for c in 0..count {
        let a = c * values.len() / count;
        let b = ((c + 1) * values.len() / count)
            .max(a + 1)
            .min(values.len());
        out.push(values[a..b].iter().copied().fold(f32::MIN, f32::max));
    }
}

/// A flat level bar: green to 55 %, through yellow to red at full scale,
/// lit up to `level` (0…1), with a peak-hold tick at `hold`.
pub fn level_bar(level: f32, hold: Option<f32>, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let [grn, yel, red] = p.level();
    let (track, tick) = (p.fill2, p.title);
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let bar = Bounds::new(
                point(b.origin.x, b.origin.y + px(3.)),
                size(b.size.width, b.size.height - px(6.)),
            );
            window.paint_quad(fill(bar, track));
            let level = level.clamp(0., 1.);
            let segments = [
                (0., 0.55, grn, grn),
                (0.55, 0.8, grn, yel),
                (0.8, 1., yel, red),
            ];
            for (from, to, a, z) in segments {
                if level <= from {
                    break;
                }
                let end = level.min(to);
                let at_end = mix(a, z, (end - from) / (to - from));
                window.paint_quad(fill(
                    Bounds::new(
                        point(bar.origin.x + bar.size.width * from, bar.origin.y),
                        size(bar.size.width * (end - from), bar.size.height),
                    ),
                    linear_gradient(90., linear_color_stop(a, 0.), linear_color_stop(at_end, 1.)),
                ));
            }
            if let Some(hold) = hold.filter(|h| *h > 0.005) {
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            b.origin.x + b.size.width * hold.min(1.) - px(1.),
                            b.origin.y,
                        ),
                        size(px(2.), b.size.height),
                    ),
                    tick,
                ));
            }
        },
    )
    .w_full()
    .h(rems(0.95))
}

// ---- controls -----------------------------------------------------------

/// A square switch, as Quattro's panel toggles.
pub fn switch(id: impl Into<ElementId>, on: bool, small: bool, cx: &App) -> Stateful<Div> {
    let p = palette(cx);
    let (w, h, knob, inset) = if small {
        (2.6, 1.4, 0.95, 0.225)
    } else {
        (3.6, 1.9, 1.35, 0.275)
    };
    div()
        .id(id)
        .relative()
        .flex_shrink_0()
        .w(rems(w))
        .h(rems(h))
        .bg(p.fill2)
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .top(rems(inset))
                .when(on, |k| k.right(rems(inset)).bg(p.hi))
                .when(!on, |k| k.left(rems(inset)).bg(p.dim))
                .size(rems(knob)),
        )
}

/// One choice in a row of bordered options; the selected one is filled.
pub fn choice(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let p = palette(cx);
    div()
        .id(id)
        .flex_1()
        .min_w(rems(4.))
        .flex()
        .justify_center()
        .py(rems(0.4))
        .px(rems(0.6))
        .border_1()
        .whitespace_nowrap()
        .cursor_pointer()
        .when(selected, |c| {
            c.bg(p.accfill).border_color(p.hi).text_color(p.title)
        })
        .when(!selected, |c| {
            c.border_color(p.muted)
                .text_color(p.fg)
                .hover(|c| c.bg(p.fill))
        })
        .child(text.into())
}

/// A bordered action button: `label · key`.
pub fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    key: &'static str,
    cx: &App,
) -> Stateful<Div> {
    let p = palette(cx);
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .flex()
        .gap(rems(0.5))
        .py(rems(0.35))
        .px(rems(0.75))
        .border_1()
        .border_color(p.muted)
        .text_color(p.title)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|b| b.bg(p.fill).border_color(p.hi))
        .child(label.into())
        .child(div().text_color(p.dim).child(key))
}

/// A list row: neutral fill when selected, as Quattro's lists.
pub fn list_row(
    id: SharedString,
    text: String,
    detail: String,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .flex()
        .items_center()
        .gap(rems(0.9))
        .py(rems(0.5))
        .px(rems(0.75))
        .cursor_pointer()
        .when(selected, |r| r.bg(p.fill2).text_color(p.title))
        .when(!selected, |r| r.text_color(p.fg).hover(|r| r.bg(p.fill)))
        .child(dot(if selected { p.hi } else { p.muted }, selected))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text),
        )
        .child(div().text_color(p.dim).whitespace_nowrap().child(detail))
        .on_click(on_click)
        .into_any_element()
}

/// The value range and behaviour of an adjustable control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    /// Neutral value for ± controls (the fill starts here) and double-click reset.
    pub neutral: f32,
    pub bipolar: bool,
}

impl Range {
    pub fn fraction(&self, value: f32) -> f32 {
        ((value - self.min) / (self.max - self.min)).clamp(0., 1.)
    }

    pub fn value_at(&self, fraction: f32) -> f32 {
        let raw = self.min + fraction.clamp(0., 1.) * (self.max - self.min);
        (self.min + ((raw - self.min) / self.step).round() * self.step).clamp(self.min, self.max)
    }
}

pub type OnChange = Rc<dyn Fn(f32, &mut Window, &mut App)>;

/// A Quattro slider: a letter-spaced label and value over a thin rounded
/// track with a round knob. Drag or click to set, scroll or arrow keys (h/l)
/// to step, shift for ×10, Home/End for the ends, double-click to reset.
#[allow(clippy::too_many_arguments)]
pub fn slider(
    id: &'static str,
    title: impl Into<String>,
    value: f32,
    text: impl Into<SharedString>,
    range: Range,
    focus: &FocusHandle,
    disabled: bool,
    on_change: OnChange,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    let focused = focus.is_focused(window);
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
    let from = if range.bipolar {
        range.fraction(range.neutral)
    } else {
        0.
    };
    let to = range.fraction(value);
    let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
    let stops = p.control();
    let track_bounds = bounds.clone();
    let track = div()
        .relative()
        .h(rems(1.25))
        .mx(rems(0.4))
        .child(
            canvas(move |b, _, _| track_bounds.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(rems(0.5))
                .h(rems(0.25))
                .rounded(rems(0.125))
                .bg(p.fill2),
        )
        .when(range.bipolar, |t| {
            t.child(
                div()
                    .absolute()
                    .left(relative(from))
                    .top(rems(0.25))
                    .w(px(1.))
                    .h(rems(0.75))
                    .bg(p.dim),
            )
        })
        .when(hi > lo, |t| {
            t.child(
                div()
                    .absolute()
                    .left(relative(lo))
                    .w(relative(hi - lo))
                    .top(rems(0.5))
                    .h(rems(0.25))
                    .rounded(rems(0.125))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(gradient(&stops, lo), 0.),
                        linear_color_stop(gradient(&stops, hi.max(0.35)), 1.),
                    )),
            )
        })
        .child(
            div()
                .absolute()
                .left(relative(to))
                .ml(rems(-0.5))
                .top(rems(0.125))
                .size(rems(1.))
                .rounded_full()
                .bg(p.title)
                .border_2()
                .border_color(if focused { p.hi } else { p.bg })
                .shadow(vec![BoxShadow {
                    color: p.mag.opacity(0.45),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(10.),
                    spread_radius: px(0.),
                    inset: false,
                }]),
        );
    let set = {
        let on_change = on_change.clone();
        let bounds = bounds.clone();
        move |x: Pixels, window: &mut Window, cx: &mut App| {
            let b = bounds.get();
            if b.size.width > px(0.) {
                on_change(range.value_at((x - b.origin.x) / b.size.width), window, cx);
            }
        }
    };
    let step_by = {
        let on_change = on_change.clone();
        move |steps: f32, window: &mut Window, cx: &mut App| {
            on_change(
                (value + steps * range.step).clamp(range.min, range.max),
                window,
                cx,
            )
        }
    };
    let (down, drag, wheel, keys, reset) = (set.clone(), set, step_by.clone(), step_by, on_change);
    let focus_on_press = focus.clone();
    div()
        .id(ElementId::from(id))
        .debug_selector(move || format!("control-{id}"))
        .flex()
        .flex_col()
        .gap(rems(0.35))
        .mb(rems(0.75))
        .when(disabled, |row| row.opacity(0.4))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(rems(0.75))
                .child(tracked(
                    vec![(title.into(), if focused { p.title } else { p.dim })],
                    0.66,
                    window,
                    cx,
                ))
                .child(
                    div()
                        .text_color(if focused { p.title } else { p.fg })
                        .whitespace_nowrap()
                        .child(text.into()),
                ),
        )
        .child(track)
        .when(!disabled, |row| {
            row.track_focus(focus)
                .cursor_ew_resize()
                .on_mouse_down(
                    MouseButton::Left,
                    move |event: &MouseDownEvent, window, cx| {
                        focus_on_press.focus(window, cx);
                        if event.click_count >= 2 {
                            reset(range.neutral, window, cx);
                        } else {
                            down(event.position.x, window, cx);
                        }
                        cx.stop_propagation();
                    },
                )
                .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                    if event.pressed_button == Some(MouseButton::Left) {
                        drag(event.position.x, window, cx);
                    }
                })
                .on_scroll_wheel(move |event: &ScrollWheelEvent, window, cx| {
                    let delta = event.delta.pixel_delta(px(16.)).y / px(1.);
                    if delta.abs() > 0.5 {
                        wheel(if delta > 0. { 1. } else { -1. }, window, cx);
                        cx.stop_propagation();
                    }
                })
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    let fast = if event.keystroke.modifiers.shift {
                        10.
                    } else {
                        1.
                    };
                    let steps = match event.keystroke.key.as_str() {
                        "left" | "h" => -fast,
                        "right" | "l" => fast,
                        "home" => -1e6,
                        "end" => 1e6,
                        _ => return,
                    };
                    keys(steps, window, cx);
                    cx.stop_propagation();
                })
        })
        .into_any_element()
}

/// Lay controls out in two columns when there is room.
pub fn columns(children: Vec<AnyElement>, wide: bool) -> Div {
    if wide {
        let mut row = div().flex().flex_wrap().gap_x(rems(2.));
        for child in children {
            row = row.child(
                div()
                    .flex_1()
                    .min_w(rems(14.))
                    .max_w(relative(0.5))
                    .child(child),
            );
        }
        row
    } else {
        div().flex().flex_col().children(children)
    }
}

/// A title in Quattro's header weight.
pub fn heading(text: impl Into<SharedString>, cx: &App) -> Div {
    let p = palette(cx);
    div()
        .text_color(p.title)
        .text_size(rems(1.3))
        .font_weight(FontWeight::BOLD)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::rgb;

    #[test]
    fn gradient_hits_its_stops() {
        let (a, b, c): (Hsla, Hsla, Hsla) = (
            rgb(0xff0000).into(),
            rgb(0x00ff00).into(),
            rgb(0x0000ff).into(),
        );
        let at = |t| Rgba::from(gradient(&[a, b, c], t));
        assert!((at(0.).r - 1.).abs() < 1e-3);
        assert!((at(0.5).g - 1.).abs() < 1e-3);
        assert!((at(1.).b - 1.).abs() < 1e-3);
        assert!((at(0.25).r - 0.5).abs() < 1e-2 && (at(0.25).g - 0.5).abs() < 1e-2);
    }

    #[test]
    fn ranges_snap_and_resample_keeps_peaks() {
        let range = Range {
            min: -12.,
            max: 12.,
            step: 0.5,
            neutral: 0.,
            bipolar: true,
        };
        assert_eq!(range.value_at(0.5), 0.);
        assert_eq!(range.value_at(0.76), 6.);
        assert_eq!(range.fraction(12.), 1.);
        let mut out = Vec::new();
        resample(&[0., 0., 9., 0., 0., 0.], 3, &mut out);
        assert_eq!(out, [0., 9., 0.]);
        resample(&[1., 2.], 4, &mut out);
        assert_eq!(out, [1., 1., 2., 2.]);
    }

    #[test]
    fn dot_masks_open_whole_dots_on_the_anchor() {
        // 6×6 logical at 2×: two dots each way, holes 4 device px, gaps 2.
        let open = |anchor| {
            let mut pixels = vec![0; 12 * 12 * 4];
            draw_dot_mask(&mut pixels, 12, 2., 6., anchor, [9, 9, 9, 255]);
            pixels.chunks(4).map(|p| p[3] == 0).collect::<Vec<_>>()
        };
        let row = |mask: &[bool], y: usize| {
            mask[y * 12..(y + 1) * 12]
                .iter()
                .map(|o| if *o { 'o' } else { '#' })
                .collect::<String>()
        };
        let top = open(Anchor::Top);
        assert_eq!(row(&top, 0), "oooo##oooo##");
        assert_eq!(row(&top, 4), "############");
        // Bottom-anchored: a 1 px gap (2 device rows) at the bottom edge, then
        // a whole dot, so bars rising from the bottom end on whole dots.
        let bottom = open(Anchor::Bottom);
        for y in [11, 10, 5, 4] {
            assert_eq!(row(&bottom, y), "############", "row {y}");
        }
        for y in [9, 6, 3, 0] {
            assert_eq!(row(&bottom, y), "oooo##oooo##", "row {y}");
        }
    }

    #[test]
    fn bars_share_columns_evenly() {
        assert_eq!(span(0, 3, 9), (0, 3));
        assert_eq!(span(2, 3, 9), (6, 9));
        assert_eq!(span(1, 4, 6), (1, 3));
        // More values than columns still give each a column.
        assert_eq!(span(5, 10, 4), (2, 3));
    }

    #[test]
    fn theme_slots_read_semantic_then_ansi_keys() {
        let semantic = ThemeSlots::parse(
            "accent = \"#7aa2f7\"\nblue = \"#7aa2f7\"\nmagenta = \"#ad8ee6\"\ncyan = \"#449dab\"\norange = \"#eb927b\"\nmuted = \"#414868\"\n",
        );
        assert_eq!(semantic.magenta, Some(rgb(0xad8ee6).into()));
        assert_eq!(semantic.orange, Some(rgb(0xeb927b).into()));
        let ansi =
            ThemeSlots::parse("color4 = \"#205ea6\"\ncolor5 = \"#ce5d97\"\ncolor8 = \"bad\"\n");
        assert_eq!(ansi.blue, Some(rgb(0x205ea6).into()));
        assert_eq!(ansi.magenta, Some(rgb(0xce5d97).into()));
        assert_eq!(ansi.muted, None);
        assert_eq!(ThemeSlots::parse("not toml ["), ThemeSlots::default());
    }

    #[test]
    fn missing_slots_derive_from_the_theme() {
        let theme = Theme::tokyo_night();
        let p = Palette::of(&theme, &ThemeSlots::default());
        assert_eq!(p.blu, theme.accent);
        assert_ne!(p.mag, theme.accent);
        let slots = ThemeSlots {
            magenta: Some(rgb(0x123456).into()),
            ..Default::default()
        };
        assert_eq!(Palette::of(&theme, &slots).mag, Hsla::from(rgb(0x123456)));
    }
}
