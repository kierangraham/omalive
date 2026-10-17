//! The Camera page: before/after preview, a carousel of live looks, RGB
//! parade, white-balance pad, tone controls, and source and outputs.
use super::{
    Omalive,
    kit::{self, OnChange, Range, palette},
};
use gpui_kit::{
    AnyElement, Bounds, Context, CursorStyle, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    ObjectFit, PathBuilder, SharedString, StyledImage, Window, canvas, div, fill, img, point,
    prelude::*, px, relative, rems, size,
};
use gpui_omarchy::{IconName, icon};
use omalive_core::{
    Control,
    looks::{GRADE_PRESETS, grade_preset},
};
use omalive_media::CameraState;
use std::rc::Rc;

/// Looks the carousel shows at once.
const LOOKS_SHOWN: usize = 5;
pub(super) const PARADE: (usize, usize) = (28, 24);
/// Width of the side column, in rems.
const SIDE: f32 = 19.;
/// Gap between the columns, in rems.
const GAP: f32 = 1.75;

impl Omalive {
    /// Every look as (file, name): no LUT first, then the library.
    fn looks(&self) -> Vec<(String, String)> {
        std::iter::once((String::new(), "Original".to_string()))
            .chain(
                self.model
                    .luts
                    .iter()
                    .map(|lut| (lut.file.clone(), lut.name.clone())),
            )
            .collect()
    }

    /// Ask the camera for swatches of the looks in view, while the preview
    /// runs; the GPU renders them with each preview frame.
    pub(super) fn sync_swatches(&mut self) {
        // Follow the chosen look wherever it changed (a click, a key, the
        // bar, or loading settings), but let the chevrons browse away.
        if self.revealed.as_ref() != Some(&self.model.settings.camera.lut) {
            self.reveal_look();
            self.revealed = Some(self.model.settings.camera.lut.clone());
        }
        let files: Vec<String> = if self.preview_size.is_some() {
            let looks = self.looks();
            let from = self.looks_from.min(looks.len().saturating_sub(LOOKS_SHOWN));
            looks
                .into_iter()
                .skip(from)
                .take(LOOKS_SHOWN)
                .map(|(file, _)| file)
                .collect()
        } else {
            Vec::new()
        };
        if files == self.swatch_files {
            return;
        }
        let cubes = files
            .iter()
            .map(|file| {
                (!file.is_empty())
                    .then(|| self.model.cube_for(file))
                    .flatten()
            })
            .collect();
        self.engine.set_swatches(cubes);
        self.swatch_files = files;
    }

    /// Scroll the carousel so the chosen look is in view.
    fn reveal_look(&mut self) {
        let selected = &self.model.settings.camera.lut;
        let looks = self.looks();
        let index = looks.iter().position(|(f, _)| f == selected).unwrap_or(0);
        let from = self.looks_from.min(looks.len().saturating_sub(LOOKS_SHOWN));
        self.looks_from = if index < from {
            index
        } else if index >= from + LOOKS_SHOWN {
            index + 1 - LOOKS_SHOWN
        } else {
            from
        };
    }

    /// The chevrons: show the previous or next few looks.
    fn page_looks(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.looks().len().saturating_sub(LOOKS_SHOWN);
        let from = self.looks_from.min(last) as isize + by * LOOKS_SHOWN as isize;
        self.looks_from = from.clamp(0, last as isize) as usize;
        cx.notify();
    }

    /// `[` and `]` on this page: choose the previous or next look.
    pub(super) fn step_look(&mut self, by: isize, cx: &mut Context<Self>) {
        let looks = self.looks();
        let index = looks
            .iter()
            .position(|(f, _)| *f == self.model.settings.camera.lut)
            .unwrap_or(0) as isize;
        let next = (index + by).rem_euclid(looks.len() as isize) as usize;
        let effects = self.model.select_lut(&looks[next].0);
        self.run(effects, cx);
    }

    pub(super) fn camera_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wide = self.wide(window);
        let viewport = window.viewport_size().width / px(1.);
        let rem = 16. * self.zoom;
        let content = viewport - 2. * super::PAD_X * rem;
        let preview_width = if wide {
            content - (SIDE + GAP) * rem
        } else {
            content
        }
        .max(160.);
        let preview = self.preview_section(preview_width, window, cx);
        let looks = self.looks_section(window, cx);
        let parade = self.parade_section(window, cx);
        let balance = self.balance_section(window, cx);
        let tone = self.tone_section(window, cx);
        let source = self.source_section(wide, window, cx);
        let main = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(preview)
            .child(looks);
        let side = div()
            .flex()
            .flex_col()
            .child(parade)
            .child(kit::divider(cx))
            .child(balance)
            .child(kit::divider(cx))
            .child(tone);
        div()
            .id("camera-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(if wide {
                        div()
                            .flex()
                            .gap(rems(GAP))
                            .child(main.flex_1())
                            .child(side.w(rems(SIDE)).flex_shrink_0())
                    } else {
                        div()
                            .flex()
                            .flex_col()
                            .child(main)
                            .child(kit::divider(cx))
                            .child(side)
                    })
                    .child(kit::divider(cx))
                    .child(source),
            )
            .into_any_element()
    }

    fn preview_section(
        &mut self,
        width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let model = &self.model;
        let live = model.camera_running() && model.camera_active;
        let height = width * 9. / 16.;
        let split = self.split.value.clamp(0., 1.);
        let stats = match (&model.camera, model.stats) {
            (
                CameraState::Running {
                    width,
                    height,
                    grader,
                    ..
                },
                Some(stats),
            ) if live => {
                format!(
                    "{width}×{height} · {:.0} fps · {:.1} ms · {}",
                    stats.fps,
                    stats.process_ms,
                    grader.replace("Apple ", "")
                )
            }
            (CameraState::Running { .. }, _) if !model.camera_active => "Standby".into(),
            _ => "Off".into(),
        };
        let look = model
            .luts
            .iter()
            .find(|lut| lut.file == model.settings.camera.lut)
            .map(|lut| lut.name.clone());
        let graded_label = match (&look, model.settings.camera.bypass) {
            (_, true) => "Bypassed · b".to_string(),
            (Some(look), false) => look.clone(),
            (None, false) => "Graded".to_string(),
        };
        let bounds = self.preview_bounds.clone();
        let mut frame = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .bg(gpui_kit::black())
            .overflow_hidden()
            .debug_selector(|| "preview".into())
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            );
        if live
            && let (Some(original), Some(graded)) = (self.original.clone(), self.preview.clone())
        {
            frame = frame
                .cursor(CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.split_to(event.position.x, cx)
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.split_to(event.position.x, cx);
                    }
                }))
                .child(
                    img(original)
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(width))
                        .h(px(height))
                        .object_fit(ObjectFit::Fill),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(split))
                        .right_0()
                        .overflow_hidden()
                        .child(
                            img(graded)
                                .absolute()
                                .top_0()
                                .left(px(-split * width))
                                .w(px(width))
                                .h(px(height))
                                .object_fit(ObjectFit::Fill),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(split))
                        .ml(px(-1.))
                        .w(px(2.))
                        .bg(p.title),
                )
                .child(
                    div()
                        .absolute()
                        .top(relative(0.5))
                        .left(relative(split))
                        .ml(rems(-0.4))
                        .mt(rems(-1.))
                        .w(rems(0.8))
                        .h(rems(2.))
                        .bg(p.title),
                )
                .child(tag("Original", p.fg, false, &p))
                .child(tag(graded_label, p.title, true, &p));
        } else {
            let (title, detail) = match &model.camera {
                CameraState::Running { .. } => {
                    ("Standby", "Wakes when an app opens Omalive Camera")
                }
                CameraState::Starting => ("Starting…", "Opening the camera"),
                CameraState::Failed(_) => ("Camera failed", "See the status line"),
                CameraState::Off => ("Camera off", "Press s to start"),
            };
            frame = frame.bg(p.fill).child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(rems(0.4))
                    .child(div().text_color(p.title).text_size(rems(1.)).child(title))
                    .child(div().text_color(p.dim).child(detail)),
            );
        }
        div()
            .child(kit::section(
                "Preview",
                Some(div().child(stats).into_any_element()),
                window,
                cx,
            ))
            .child(frame)
            .into_any_element()
    }

    fn split_to(&mut self, x: gpui_kit::Pixels, cx: &mut Context<Self>) {
        let b = self.preview_bounds.get();
        if b.size.width > px(0.) {
            self.split_target = ((x - b.origin.x) / b.size.width).clamp(0.02, 0.98);
            cx.notify();
        }
    }

    fn looks_section(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let selected = self.model.settings.camera.lut.clone();
        let strength = self.model.settings.camera.lut_strength;
        let looks = self.looks();
        let from = self.looks_from.min(looks.len().saturating_sub(LOOKS_SHOWN));
        let mut tiles = div().flex_1().min_w_0().flex().gap(rems(0.4));
        for slot in 0..LOOKS_SHOWN {
            let Some((file, name)) = looks.get(from + slot).cloned() else {
                // Keep every tile the same width at the end of the list.
                tiles = tiles.child(div().flex_1().min_w_0());
                continue;
            };
            let is_selected = file == selected;
            // Swatches arrive with the next preview frame after a flip; never
            // show one under the wrong name.
            let swatch = (self.swatch_files.get(slot) == Some(&file))
                .then(|| self.swatches.get(slot).cloned())
                .flatten();
            let id = SharedString::from(format!(
                "look-{}",
                if file.is_empty() { "none" } else { &file }
            ));
            let selector = id.to_string();
            tiles =
                tiles.child(
                    div()
                        .id(id)
                        .debug_selector(move || selector.clone())
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(rems(0.3))
                        .cursor_pointer()
                        .child(
                            div()
                                .w_full()
                                .aspect_ratio(16. / 9.)
                                .border_2()
                                .border_color(if is_selected {
                                    p.hi
                                } else {
                                    gpui_kit::transparent_black()
                                })
                                .bg(p.fill)
                                .overflow_hidden()
                                .children(swatch.map(|image| {
                                    img(image).size_full().object_fit(ObjectFit::Cover)
                                })),
                        )
                        .child(
                            div()
                                .text_size(rems(0.7))
                                .line_height(rems(1.1))
                                .text_color(if is_selected { p.title } else { p.dim })
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(name),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let effects = this.model.select_lut(&file);
                            this.run(effects, cx);
                        })),
                );
        }
        let arrow = |id: &'static str, name: IconName, by: isize, enabled: bool| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .flex()
                .items_center()
                .justify_center()
                .w(rems(1.6))
                .h(rems(3.))
                .mb(rems(1.1))
                .text_color(if enabled { p.title } else { p.muted })
                .when(enabled, |a| {
                    a.cursor_pointer()
                        .hover(|a| a.bg(p.fill))
                        .on_click(cx.listener(move |this, _, _, cx| this.page_looks(by, cx)))
                })
                .child(icon(name).size(rems(1.1)))
        };
        let strip = div()
            .flex()
            .items_center()
            .gap(rems(0.4))
            .child(arrow("looks-back", IconName::ChevronLeft, -1, from > 0))
            .child(tiles)
            .child(arrow(
                "looks-forward",
                IconName::ChevronRight,
                1,
                from + LOOKS_SHOWN < looks.len(),
            ));
        let position = looks
            .iter()
            .position(|(file, _)| *file == selected)
            .map_or(0, |i| i + 1);
        let entity = cx.entity();
        let on_change: OnChange = Rc::new(move |value, _, cx| {
            entity.update(cx, |this, cx| {
                let effects = this.model.set_strength(value);
                this.run(effects, cx);
            })
        });
        let right = if selected.is_empty() {
            format!("{position} of {} · [ ] to flip", looks.len())
        } else {
            format!(
                "{position} of {} · strength {:.0}%",
                looks.len(),
                strength * 100.
            )
        };
        div()
            .mt(rems(1.2))
            .child(kit::section(
                "Looks",
                Some(div().child(right).into_any_element()),
                window,
                cx,
            ))
            .child(strip)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_end()
                    .gap(rems(1.5))
                    .mt(rems(0.9))
                    .child(div().flex_1().min_w(rems(12.)).child(kit::slider(
                        "strength",
                        "Strength",
                        strength,
                        format!("{:.0} %", strength * 100.),
                        Range {
                            min: 0.,
                            max: 1.,
                            step: 0.01,
                            neutral: 1.,
                            bipolar: false,
                        },
                        &self.focus_for("strength"),
                        selected.is_empty(),
                        on_change,
                        window,
                        cx,
                    )))
                    .child(
                        div()
                            .flex()
                            .gap(rems(0.6))
                            .mb(rems(0.75))
                            .when(self.model.starter_looks_missing(), |row| {
                                row.child(
                                    kit::button("starter-looks", "Add looks", "", cx).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.model.install_starter_looks();
                                            this.swatch_files.clear();
                                            cx.notify();
                                        }),
                                    ),
                                )
                            })
                            .child(
                                kit::button("import-lut", "Import", "i", cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.import_lut(cx))),
                            )
                            .child(
                                kit::button("open-luts", "Folder", "o", cx).on_click(
                                    cx.listener(|this, _, _, cx| this.open_lut_folder(cx)),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn parade_section(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let data = self.parade.clone();
        let live = self.model.camera_running() && self.model.camera_active;
        let colors = [p.red, p.grn, p.blu];
        let (bg, line) = (p.bg, p.line);
        let scope = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let (columns, rows) = PARADE;
                let third = b.size.width / 3.;
                for channel in 0..3 {
                    let x0 = b.origin.x + third * channel as f32;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x0, b.origin.y + b.size.height - px(1.)),
                            size(third - px(8.), px(1.)),
                        ),
                        line,
                    ));
                }
                if !live || data.is_empty() {
                    return;
                }
                let pitch_x = (third - px(8.)) / columns as f32;
                let pitch_y = b.size.height / rows as f32;
                for (channel, color) in colors.iter().enumerate() {
                    let x0 = b.origin.x + third * channel as f32;
                    for row in 0..rows {
                        for column in 0..columns {
                            let intensity = data[row * columns + column][channel];
                            if intensity < 0.04 {
                                continue;
                            }
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(
                                        x0 + pitch_x * column as f32,
                                        b.origin.y + b.size.height - pitch_y * (row + 1) as f32,
                                    ),
                                    size(px(2.), px(2.)),
                                ),
                                kit::mix(bg, *color, (0.35 + 0.75 * intensity).min(1.)),
                            ));
                        }
                    }
                }
            },
        )
        .size_full();
        div()
            .child(kit::section(
                "Parade",
                Some(
                    div()
                        .text_color(p.dim)
                        .child("R · G · B")
                        .into_any_element(),
                ),
                window,
                cx,
            ))
            .child(
                div()
                    .h(rems(6.5))
                    .debug_selector(|| "parade".into())
                    .child(scope),
            )
            .into_any_element()
    }

    fn balance_section(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let grade = self.model.settings.camera.grade;
        let (px_, py_) = (self.pad.0.value, self.pad.1.value);
        let colors = (p.bg, p.cyn, p.org, p.mag, p.grn, p.title, p.muted);
        let bounds = self.pad_bounds.clone();
        let images = self.images.clone();
        let pad = canvas(
            move |b, _, _| bounds.set(b),
            move |b, _, window, _| {
                let (bg, cool, warm, magenta, green, title, line) = colors;
                let (w, h) = (b.size.width / px(1.), b.size.height / px(1.));
                // Dot colour at (u, v): cool to warm across, magenta to green down.
                let tint = |u: f32, v: f32| {
                    let horizontal = kit::mix(
                        bg,
                        if u < 0.5 { cool } else { warm },
                        0.25 + (u - 0.5).abs() * 1.5,
                    );
                    kit::mix(
                        horizontal,
                        if v < 0.5 { magenta } else { green },
                        (v - 0.5).abs() * 0.7,
                    )
                };
                // The field is static: one cached image, redrawn only when
                // the size, scale or theme changes.
                let scale = window.scale_factor();
                let (dw, dh) = ((w * scale).round() as u32, (h * scale).round() as u32);
                let key = kit::hash((dw, dh, scale.to_bits(), format!("{colors:?}")));
                if dw > 0
                    && dh > 0
                    && let Some(image) = images.get("balance", key, (dw, dh), window, |pixels| {
                        let mut y = 1.;
                        while y < h {
                            let mut x = 1.;
                            while x < w {
                                let color = kit::bgra(tint(x / w, y / h));
                                let (x0, y0) =
                                    ((x * scale).round() as u32, (y * scale).round() as u32);
                                let (x1, y1) = (
                                    ((x + 2.) * scale).round() as u32,
                                    ((y + 2.) * scale).round() as u32,
                                );
                                for py in y0..y1.min(dh) {
                                    for px_ in x0..x1.min(dw) {
                                        let i = ((py * dw + px_) * 4) as usize;
                                        pixels[i..i + 4].copy_from_slice(&color);
                                    }
                                }
                                x += 6.;
                            }
                            y += 6.;
                        }
                    })
                {
                    let _ = window.paint_image(b, b, Default::default(), image, 0, false);
                }
                // Only the dots near the knob glow, so only they are painted live.
                let reach = h / 6.;
                let (kx, ky) = (w * px_, h * py_);
                let first = |centre: f32| (((centre - reach - 1.) / 6.).floor().max(0.)) * 6. + 1.;
                let mut y = first(ky);
                while y < h && y <= ky + reach {
                    let mut x = first(kx);
                    while x < w && x <= kx + reach {
                        let (u, v) = (x / w, y / h);
                        let glow = (1. - ((u - px_) * w / h).hypot(v - py_) * 6.).max(0.);
                        if glow > 0. {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(b.origin.x + px(x), b.origin.y + px(y)),
                                    size(px(2.), px(2.)),
                                ),
                                kit::mix(tint(u, v), title, glow),
                            ));
                        }
                        x += 6.;
                    }
                    y += 6.;
                }
                window.paint_quad(fill(
                    Bounds::new(
                        point(b.origin.x + b.size.width / 2., b.origin.y),
                        size(px(1.), b.size.height),
                    ),
                    line,
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        point(b.origin.x, b.origin.y + b.size.height / 2.),
                        size(b.size.width, px(1.)),
                    ),
                    line,
                ));
                let centre = point(
                    b.origin.x + b.size.width * px_,
                    b.origin.y + b.size.height * py_,
                );
                let mut outline = PathBuilder::stroke(px(2.));
                let r = px(7.);
                outline.move_to(point(centre.x - r, centre.y - r));
                outline.line_to(point(centre.x + r, centre.y - r));
                outline.line_to(point(centre.x + r, centre.y + r));
                outline.line_to(point(centre.x - r, centre.y + r));
                outline.close();
                if let Ok(path) = outline.build() {
                    window.paint_path(path, title);
                }
            },
        )
        .size_full();
        let signed = |v: f32| format!("{:+.0}", v * 100.).replace('-', "−");
        let right = format!(
            "Warm {} · Tint {}",
            signed(grade.warmth),
            signed(grade.tint)
        );
        div()
            .child(kit::section(
                "Balance",
                Some(div().child(right).into_any_element()),
                window,
                cx,
            ))
            .child(
                div()
                    .relative()
                    .h(rems(7.))
                    .debug_selector(|| "balance".into())
                    .cursor(CursorStyle::Crosshair)
                    .text_size(rems(0.58))
                    .text_color(p.fg)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if event.click_count >= 2 {
                                let mut effects = this.model.set_grade(Control::Warmth, 0.);
                                effects.extend(this.model.set_grade(Control::Tint, 0.));
                                this.run(effects, cx);
                            } else {
                                this.balance_to(event.position, cx);
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.balance_to(event.position, cx);
                        }
                    }))
                    .child(pad)
                    .child(
                        div()
                            .absolute()
                            .left(rems(0.2))
                            .top(relative(0.5))
                            .mt(rems(-1.))
                            .child("COOL"),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(rems(0.2))
                            .top(relative(0.5))
                            .mt(rems(-1.))
                            .child("WARM"),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(relative(0.5))
                            .ml(rems(0.35))
                            .child("MAGENTA"),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left(relative(0.5))
                            .ml(rems(0.35))
                            .child("GREEN"),
                    ),
            )
            .into_any_element()
    }

    fn balance_to(&mut self, position: gpui_kit::Point<gpui_kit::Pixels>, cx: &mut Context<Self>) {
        let b = self.pad_bounds.get();
        if b.size.width <= px(0.) {
            return;
        }
        let u = ((position.x - b.origin.x) / b.size.width).clamp(0., 1.);
        let v = ((position.y - b.origin.y) / b.size.height).clamp(0., 1.);
        let mut effects = self.model.set_grade(Control::Warmth, (u - 0.5) * 2.);
        effects.extend(self.model.set_grade(Control::Tint, (0.5 - v) * 2.));
        self.run(effects, cx);
    }

    fn tone_section(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let grade = self.model.settings.camera.grade;
        let mut section = div().child(kit::section(
            "Tone",
            Some(div().child("b compare · r reset").into_any_element()),
            window,
            cx,
        ));
        // Presets set every slider at once; editing one makes it Custom.
        let current = grade_preset(&grade).map(|preset| preset.name);
        let mut presets = div().flex().flex_wrap().gap(rems(0.45)).mb(rems(0.8));
        for preset in &GRADE_PRESETS {
            let id = format!("grade-{}", preset.name.to_lowercase().replace(' ', "-"));
            let selector = id.clone();
            presets = presets.child(
                kit::choice(
                    SharedString::from(id),
                    preset.name,
                    current == Some(preset.name),
                    cx,
                )
                .debug_selector(move || selector.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let effects = this.model.apply_grade_preset(preset);
                    this.run(effects, cx);
                })),
            );
        }
        if current.is_none() {
            presets = presets.child(kit::choice("grade-custom", "Custom", true, cx));
        }
        let p = palette(cx);
        let about = grade_preset(&grade)
            .map(|preset| preset.description)
            .unwrap_or("Your own settings");
        section = section
            .child(presets.mb(rems(0.3)))
            .child(div().mb(rems(0.8)).text_color(p.dim).child(about));
        for control in [
            Control::Exposure,
            Control::Contrast,
            Control::Highlights,
            Control::Shadows,
            Control::Vibrance,
            Control::Saturation,
        ] {
            let (min, max, step) = control.range();
            let entity = cx.entity();
            let on_change: OnChange = Rc::new(move |value, _, cx| {
                entity.update(cx, |this, cx| {
                    let effects = this.model.set_grade(control, value);
                    this.run(effects, cx);
                })
            });
            section = section.child(kit::slider(
                control.key(),
                control.label(),
                grade.get(control),
                control.format(grade.get(control)).replace('-', "−"),
                Range {
                    min,
                    max,
                    step,
                    neutral: 0.,
                    bipolar: true,
                },
                &self.focus_for(control.key()),
                false,
                on_change,
                window,
                cx,
            ));
        }
        section.into_any_element()
    }

    fn source_section(&self, wide: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let model = &self.model;
        let running = model.camera_running();
        let mut cameras = div().flex().flex_col();
        for camera in model.cameras.clone() {
            let id = camera.id.clone();
            let selected = model.selected_camera().is_some_and(|c| c.id == camera.id);
            cameras = cameras.child(kit::list_row(
                SharedString::from(format!("camera-{}", camera.id)),
                camera.name.clone(),
                camera.path.display().to_string(),
                selected,
                cx.listener(move |this, _, _, cx| {
                    let effects = this.model.select_camera(&id, this.demo);
                    this.run(effects, cx);
                }),
                cx,
            ));
        }
        let mut sizes = div().flex().flex_wrap().gap(rems(0.6)).mt(rems(0.8));
        let current = model.camera_size();
        for (w, h) in model
            .selected_camera()
            .map(|c| c.sizes.clone())
            .unwrap_or_default()
        {
            let id = format!("size-{w}x{h}");
            let selector = id.clone();
            sizes = sizes.child(
                kit::choice(
                    SharedString::from(id),
                    format!("{w}×{h}"),
                    current == Some((w, h)),
                    cx,
                )
                .debug_selector(move || selector.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let effects = this.model.select_size((w, h), this.demo);
                    this.run(effects, cx);
                })),
            );
        }
        let pipewire = match (running, model.viewers) {
            (false, _) => (p.dim, false, "Starts with the camera"),
            (true, true) => (p.grn, true, "Streaming to an app"),
            (true, false) => (p.grn, true, "Ready for browsers and OBS"),
        };
        let v4l2_title = model
            .loopback
            .as_ref()
            .map_or("V4L2 · v4l2loopback".to_string(), |path| {
                format!("V4L2 · {path}")
            });
        let v4l2 = match &model.loopback {
            Some(_) if running => (p.grn, true, "Hopp · Zoom · Discord"),
            _ if running => (p.yel, true, "Load v4l2loopback for native apps"),
            _ => (p.dim, false, "Used when the module is loaded"),
        };
        let output = |title: String, (color, on, detail): (Hsla, bool, &'static str)| {
            div()
                .flex()
                .items_center()
                .gap(rems(0.9))
                .py(rems(0.5))
                .px(rems(0.75))
                .when(on, |r| r.bg(p.fill))
                .child(kit::dot(color, on))
                .child(
                    div()
                        .flex_1()
                        .text_color(if on { p.title } else { p.fg })
                        .child(title),
                )
                .child(div().text_color(p.dim).whitespace_nowrap().child(detail))
        };
        let on_demand = model.settings.camera.on_demand;
        let outputs = div()
            .flex()
            .flex_col()
            .child(output("PipeWire · Omalive Camera".into(), pipewire))
            .child(output(v4l2_title, v4l2))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mt(rems(0.8))
                    .px(rems(0.75))
                    .child(kit::label("Only when in use", window, cx))
                    .child(
                        kit::switch("on-demand", on_demand, true, cx)
                            .debug_selector(|| "toggle-on-demand".into())
                            .track_focus(&self.focus_for("on-demand"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let effects = this
                                    .model
                                    .set_on_demand(!this.model.settings.camera.on_demand);
                                this.run(effects, cx);
                            })),
                    ),
            );
        let (status, tone) = model.camera_status();
        let source = div()
            .flex()
            .flex_col()
            .child(kit::section("Source", None, window, cx))
            .child(cameras)
            .child(sizes);
        let outputs = div()
            .flex()
            .flex_col()
            .child(kit::section(
                "Outputs",
                Some(
                    div()
                        .text_color(super::tone_color(tone, &p))
                        .child(status)
                        .into_any_element(),
                ),
                window,
                cx,
            ))
            .child(outputs);
        if wide {
            div()
                .flex()
                .gap(rems(GAP))
                .child(source.flex_1().min_w_0())
                .child(outputs.flex_1().min_w_0())
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(rems(1.2))
                .child(source)
                .child(outputs)
                .into_any_element()
        }
    }
}

fn tag(text: impl Into<SharedString>, color: Hsla, right: bool, p: &kit::Palette) -> gpui_kit::Div {
    div()
        .absolute()
        .top(rems(0.6))
        .when(right, |t| t.right(rems(0.6)))
        .when(!right, |t| t.left(rems(0.6)))
        .px(rems(0.45))
        .py(rems(0.1))
        .bg(p.bg.opacity(0.8))
        .text_size(rems(0.66))
        .text_color(color)
        .child(text.into().to_uppercase())
}
