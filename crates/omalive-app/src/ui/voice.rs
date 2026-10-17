//! The Voice page: spectrum with an editable EQ, waveform and output
//! meters, the processing chain and the selected module's controls.
use super::{
    Omalive,
    kit::{self, OnChange, Range, palette},
};
use crate::{model::Tone, visuals::FLOOR_DB};
use gpui_kit::{
    AnyElement, Bounds, Context, CursorStyle, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    PathBuilder, Pixels, Point, SharedString, Window, canvas, div, fill, linear_color_stop,
    linear_gradient, point, prelude::*, px, relative, rems, size,
};
use omalive_core::{
    Preset,
    settings::voice_range,
    voice::{BANDS, eq_response},
};
use omalive_media::VoiceState;
use std::rc::Rc;

/// The chain's modules, in signal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
    Input,
    LowCut,
    Denoise,
    Eq,
    Compressor,
    Output,
}

impl Module {
    pub const ALL: [Module; 6] = [
        Module::Input,
        Module::LowCut,
        Module::Denoise,
        Module::Eq,
        Module::Compressor,
        Module::Output,
    ];
    fn name(self) -> &'static str {
        match self {
            Module::Input => "Input",
            Module::LowCut => "Low cut",
            Module::Denoise => "Denoise",
            Module::Eq => "EQ",
            Module::Compressor => "Compress",
            Module::Output => "Output",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Module::Input => "module-input",
            Module::LowCut => "module-lowcut",
            Module::Denoise => "module-denoise",
            Module::Eq => "module-eq",
            Module::Compressor => "module-comp",
            Module::Output => "module-output",
        }
    }
}

const LOW: f32 = 20.;
const HIGH: f32 = 20_000.;
/// The EQ curve's ± range on the spectrum.
const EQ_RANGE: f32 = 15.;
/// EQ nodes: settings key, frequency (for gains) and label: the low cut,
/// then the bands of `voice::BANDS`.
const NODES: [(&str, f32, &str); 5] = [
    ("highpass", 0., "Low cut"),
    (BANDS[0].key, BANDS[0].frequency, BANDS[0].name),
    (BANDS[1].key, BANDS[1].frequency, BANDS[1].name),
    (BANDS[2].key, BANDS[2].frequency, BANDS[2].name),
    (BANDS[3].key, BANDS[3].frequency, BANDS[3].name),
];

fn x_of(frequency: f32) -> f32 {
    (frequency / LOW).ln() / (HIGH / LOW).ln()
}

fn eq_y(db: f32) -> f32 {
    0.5 - db.clamp(-EQ_RANGE, EQ_RANGE) / (2. * EQ_RANGE)
}

pub(super) fn format(key: &str, value: f32) -> String {
    let value = if value.abs() < 0.05 { 0. } else { value };
    match key {
        "highpass" => format!("{value:.0} Hz"),
        "low" | "mud" | "mid" | "high" | "output" => format!("{value:+.1} dB"),
        "vad" => format!("{value:.0} %"),
        "threshold" | "makeup" => format!("{value:.1} dB"),
        "ratio" => format!("{value:.1} : 1"),
        "attack" => format!("{value:.1} ms"),
        "release" => format!("{value:.0} ms"),
        _ => format!("{value:.1}"),
    }
}

/// dB with a typographic minus, as the meters read.
fn db(value: f32) -> String {
    format!("{value:.1} dB").replace('-', "−")
}

impl Omalive {
    pub(super) fn voice_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let wide = self.wide(window);
        let spectrum = self.spectrum_section(window, cx);
        let signal = self.signal_section(window, cx);
        let output = self.output_section(window, cx);
        let chain = self.chain_section(wide, window, cx);
        let module = self.module_section(wide, window, cx);
        div()
            .id("voice-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .child(spectrum)
                    .child(kit::divider(cx))
                    .child(if wide {
                        div()
                            .flex()
                            .gap(rems(1.75))
                            .child(div().flex_1().min_w_0().child(signal))
                            .child(div().w(relative(0.4)).flex_shrink_0().child(output))
                    } else {
                        div()
                            .flex()
                            .flex_col()
                            .gap(rems(1.2))
                            .child(signal)
                            .child(output)
                    })
                    .child(kit::divider(cx))
                    .child(chain)
                    .child(kit::divider(cx))
                    .child(module),
            )
            .into_any_element()
    }

    fn spectrum_section(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let running = self.model.voice_running();
        let visuals = &self.visuals;
        let input: Vec<f32> = visuals
            .input
            .iter()
            .map(|db| (db - FLOOR_DB) / -FLOOR_DB)
            .collect();
        let output: Vec<f32> = visuals
            .output
            .iter()
            .map(|s| (s.value - FLOOR_DB) / -FLOOR_DB)
            .collect();
        let peaks: Vec<f32> = visuals
            .peaks
            .iter()
            .map(|db| (db - FLOOR_DB) / -FLOOR_DB)
            .collect();
        let voice = self.model.settings.voice.clone();
        let curve: Vec<(f32, f32)> = (0..=240)
            .map(|i| {
                let f = LOW * (HIGH / LOW).powf(i as f32 / 240.);
                (i as f32 / 240., eq_y(eq_response(&voice, f)))
            })
            .collect();
        let nodes: Vec<(f32, f32)> = NODES
            .iter()
            .map(|(key, frequency, _)| {
                if *key == "highpass" {
                    (
                        x_of(voice.highpass),
                        eq_y(eq_response(&voice, voice.highpass)),
                    )
                } else {
                    (x_of(*frequency), eq_y(voice.get(key)))
                }
            })
            .collect();
        let dragging = self.drag_node;
        let bounds = self.spectrum_bounds.clone();
        let painted_nodes = nodes.clone();
        let bands = p.bands();
        let colors = (p.spectrum(), p.muted, p.hi, p.bg, p.title);
        let images = self.images.clone();
        let graph = canvas(
            move |b, _, _| bounds.set(b),
            move |b, _, window, _| {
                let (hues, muted, accent, bg, title) = colors;
                let line = kit::mix(bg, muted, 0.5);
                if running {
                    // The raw microphone is a faint ghost in the same hues,
                    // so what the chain removed reads at a glance.
                    kit::paint_hued_bars(window, b, &input, &hues, bg, 0.3);
                    kit::paint_hued_bars(window, b, &output, &hues, title, 1.);
                    kit::paint_dot_mask(window, b, bg, kit::Anchor::Bottom, &images, "spectrum");
                    kit::paint_marks(window, b, &peaks, title.opacity(0.8));
                }
                // The grid goes over the mask so its lines stay continuous.
                for frequency in [50., 100., 200., 500., 1000., 2000., 5000., 10000.] {
                    let x = b.origin.x + b.size.width * x_of(frequency);
                    window.paint_quad(fill(
                        Bounds::new(point(x, b.origin.y), size(px(1.), b.size.height)),
                        line,
                    ));
                }
                for gain in [-12., 0., 12.] {
                    let y = b.origin.y + b.size.height * eq_y(gain);
                    window.paint_quad(fill(
                        Bounds::new(point(b.origin.x, y), size(b.size.width, px(1.))),
                        if gain == 0. { muted } else { line },
                    ));
                }
                let at = |x: f32, y: f32| {
                    point(
                        b.origin.x + b.size.width * x,
                        b.origin.y + b.size.height * y.clamp(0., 1.),
                    )
                };
                let mut path = PathBuilder::stroke(px(1.5));
                for (i, (x, y)) in curve.iter().enumerate() {
                    if i == 0 {
                        path.move_to(at(*x, *y));
                    } else {
                        path.line_to(at(*x, *y));
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, accent);
                }
                for (i, (x, y)) in painted_nodes.iter().enumerate() {
                    let centre = at(*x, *y);
                    let r = px(if dragging == Some(i) { 6. } else { 5. });
                    let color = bands[i];
                    let square =
                        Bounds::new(point(centre.x - r, centre.y - r), size(r * 2., r * 2.));
                    window.paint_quad(fill(square, color));
                    if dragging != Some(i) {
                        let inner = Bounds::new(
                            point(square.origin.x + px(1.5), square.origin.y + px(1.5)),
                            size(r * 2. - px(3.), r * 2. - px(3.)),
                        );
                        window.paint_quad(fill(inner, bg));
                    }
                }
            },
        )
        .size_full();
        let node_labels =
            NODES
                .iter()
                .zip(&nodes)
                .enumerate()
                .map(|(i, ((key, _, label), (x, y)))| {
                    let value = if *key == "highpass" {
                        format!("{label} {:.0}", voice.highpass)
                    } else {
                        format!("{label} {:+.1}", voice.get(key))
                    };
                    div()
                        .absolute()
                        .left(relative(x.min(0.86)))
                        .top(relative(y.clamp(0.06, 1.)))
                        .mt(rems(-1.2))
                        .ml(rems(0.6))
                        .text_size(rems(0.62))
                        .text_color(bands[i])
                        .whitespace_nowrap()
                        .child(value.to_uppercase())
                });
        let freq_labels = [
            (50., "50"),
            (100., "100"),
            (200., "200"),
            (500., "500"),
            (1000., "1k"),
            (2000., "2k"),
            (5000., "5k"),
            (10000., "10k"),
        ]
        .map(|(f, label)| {
            div()
                .absolute()
                .bottom(rems(-1.2))
                .left(relative(x_of(f)))
                .ml(rems(-0.4))
                .child(label)
        });
        let lufs = match self.visuals.readout.lufs.filter(|_| running) {
            Some(l) => div()
                .text_color(p.grn)
                .child(format!("{l:.0} LUFS").replace('-', "−")),
            None => div().text_color(p.dim).child("– LUFS"),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .child(kit::section(
                "Spectrum",
                Some(lufs.into_any_element()),
                window,
                cx,
            ))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(rems(12.))
                    .ml(rems(1.9))
                    .mb(rems(1.3))
                    .text_size(rems(0.62))
                    .text_color(p.dim)
                    .debug_selector(|| "spectrum".into())
                    .cursor(if self.drag_node.is_some() {
                        CursorStyle::ClosedHand
                    } else {
                        CursorStyle::OpenHand
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.spectrum_press(event.position, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.spectrum_drag(event.position, cx);
                        } else if this.drag_node.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.drag_node = None;
                            cx.notify();
                        }),
                    )
                    .child(graph)
                    .children(node_labels)
                    .children(freq_labels)
                    .children([(12., "+12"), (0., "0"), (-12., "−12")].map(|(db, label)| {
                        div()
                            .absolute()
                            .left(rems(-1.9))
                            .top(relative(eq_y(db)))
                            .mt(rems(-0.45))
                            .child(label)
                    }))
                    .when(!running, |graph| {
                        graph.child(
                            div()
                                .absolute()
                                .top(relative(0.66))
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .text_size(rems(0.8))
                                .text_color(p.dim)
                                .child("Start voice to see your signal · s"),
                        )
                    }),
            )
            .into_any_element()
    }

    fn spectrum_press(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let b = self.spectrum_bounds.get();
        let voice = &self.model.settings.voice;
        let nearest = NODES
            .iter()
            .enumerate()
            .map(|(i, (key, frequency, _))| {
                let (x, y) = if *key == "highpass" {
                    (
                        x_of(voice.highpass),
                        eq_y(eq_response(voice, voice.highpass)),
                    )
                } else {
                    (x_of(*frequency), eq_y(voice.get(key)))
                };
                let node = point(
                    b.origin.x + b.size.width * x,
                    b.origin.y + b.size.height * y,
                );
                let d = ((node.x - position.x) / px(1.)).hypot((node.y - position.y) / px(1.));
                (i, d)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, d)) = nearest
            && d < 16.
        {
            self.drag_node = Some(i);
            self.module = if i == 0 { Module::LowCut } else { Module::Eq };
            cx.notify();
        }
    }

    fn spectrum_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(i) = self.drag_node else {
            return;
        };
        let b = self.spectrum_bounds.get();
        if b.size.width <= px(0.) {
            return;
        }
        let (key, _, _) = NODES[i];
        let effects = if key == "highpass" {
            let x = ((position.x - b.origin.x) / b.size.width).clamp(0., 1.);
            self.model.set_voice(key, LOW * (HIGH / LOW).powf(x))
        } else {
            let y = ((position.y - b.origin.y) / b.size.height).clamp(0., 1.);
            self.model.set_voice(key, (0.5 - y) * 2. * EQ_RANGE)
        };
        self.run(effects, cx);
    }

    fn signal_section(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let input: Vec<f32> = self.visuals.wave_input.iter().map(|v| v.sqrt()).collect();
        let output: Vec<f32> = self.visuals.wave_output.iter().map(|v| v.sqrt()).collect();
        let (signal, ghost, bg) = (p.signal(), p.muted, p.bg);
        let images = self.images.clone();
        let running = self.model.voice_running();
        let graph = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                if !running {
                    return;
                }
                let count = kit::column_count(b);
                let tail = |values: &[f32]| values[values.len().saturating_sub(count)..].to_vec();
                let (input, output) = (tail(&input), tail(&output));
                let offset = count.saturating_sub(input.len());
                let shifted = Bounds::new(
                    point(b.origin.x + px(offset as f32 * kit::PITCH), b.origin.y),
                    b.size,
                );
                kit::paint_wave(window, shifted, &input, &[ghost, ghost], 1., 1.);
                kit::paint_wave(window, shifted, &output, &signal, 1., 1.6);
                kit::paint_dot_mask(window, b, bg, kit::Anchor::Top, &images, "wave");
            },
        )
        .size_full();
        div()
            .child(kit::section(
                "Signal",
                Some(div().child("Mic · Omalive Mic").into_any_element()),
                window,
                cx,
            ))
            .child(
                div()
                    .h(rems(6.))
                    .debug_selector(|| "wave".into())
                    .child(graph),
            )
            .into_any_element()
    }

    fn output_section(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let v = &self.visuals;
        let running = self.model.voice_running();
        let muted = self.model.muted();
        let live = running && !muted;
        let level = |db: f32| if live { (db + 60.) / 60. } else { 0. };
        let reading = |value: f32| if running { db(value) } else { "– dB".into() };
        let noise = match (running, v.readout.noise_removed) {
            (true, Some(removed)) => format!("−{removed:.0} dB"),
            (true, None) => "measuring".into(),
            _ => "–".into(),
        };
        let latency = if self.model.settings.voice.suppression {
            "10 ms"
        } else {
            "5 ms"
        };
        let pair = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_1()
                .justify_between()
                .gap(rems(0.75))
                .child(div().text_color(p.dim).child(label))
                .child(div().text_color(p.title).whitespace_nowrap().child(value))
        };
        let row = |a: AnyElement, b: AnyElement| {
            div().flex().gap(rems(1.25)).py(rems(0.2)).child(a).child(b)
        };
        let (status, tone) = self.model.voice_status();
        let blocked = self
            .model
            .voice_blocker()
            .filter(|_| !running && !self.model.voice_busy());
        let right = div()
            .text_color(super::tone_color(tone, &p))
            .child(if running {
                reading(v.readout.peak)
            } else {
                status.to_string()
            });
        div()
            .child(kit::section(
                "Output",
                Some(right.into_any_element()),
                window,
                cx,
            ))
            .child(row(
                pair("Peak", reading(v.readout.hold)).into_any_element(),
                pair("RMS", reading(v.readout.rms)).into_any_element(),
            ))
            .child(row(
                pair(
                    "Reduction",
                    if running {
                        format!("−{:.1} dB", v.readout.reduction)
                    } else {
                        "– dB".into()
                    },
                )
                .into_any_element(),
                pair("Noise", noise).into_any_element(),
            ))
            .child(row(
                pair("Latency", latency.into()).into_any_element(),
                pair("Format", "48k mono".into()).into_any_element(),
            ))
            .child(div().mt(rems(0.5)).child(kit::level_bar(
                level(v.peak.value),
                live.then(|| level(v.hold)),
                cx,
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.75))
                    .mt(rems(0.6))
                    .child(kit::label("Mute · m", window, cx))
                    .child(
                        kit::switch("mute", muted, true, cx)
                            .debug_selector(|| "mute".into())
                            .when(!running, |s| s.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let effects = this.model.toggle_mute();
                                this.run(effects, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .children(blocked.map(|reason| div().text_color(p.yel).child(reason)))
                    .children(self.test_button(cx)),
            )
            .child(self.test_section(cx))
            .into_any_element()
    }

    fn module_summary(&self, module: Module) -> (String, Tone) {
        let voice = &self.model.settings.voice;
        let model = &self.model;
        let on = |enabled: bool, available: bool| match (enabled, available) {
            (_, false) => Tone::Bad,
            (true, true) => Tone::Good,
            (false, true) => Tone::Neutral,
        };
        match module {
            Module::Input => (
                model
                    .selected_input()
                    .map(|input| input.description.clone())
                    .unwrap_or_else(|| "Choose input".into()),
                match model.voice {
                    VoiceState::WaitingForMicrophone => Tone::Warning,
                    _ if model.selected_input().is_some() => Tone::Good,
                    _ => Tone::Warning,
                },
            ),
            Module::LowCut => (format!("{:.0} Hz", voice.highpass), Tone::Good),
            Module::Denoise => (
                if model.plugins.noise.is_none() {
                    "Not installed".into()
                } else if voice.suppression {
                    "RNNoise".into()
                } else {
                    "Bypassed".into()
                },
                on(voice.suppression, model.plugins.noise.is_some()),
            ),
            Module::Eq => (
                voice
                    .preset()
                    .map(|p| p.name().to_string())
                    .unwrap_or_else(|| "Custom".into()),
                Tone::Good,
            ),
            Module::Compressor => (
                if model.plugins.compressor.is_none() {
                    "Not installed".into()
                } else if voice.compression {
                    format!("{:.1}:1 · {:.0} dB", voice.ratio, voice.threshold).replace('-', "−")
                } else {
                    "Bypassed".into()
                },
                on(voice.compression, model.plugins.compressor.is_some()),
            ),
            Module::Output => (
                "Omalive Mic".into(),
                match model.voice {
                    VoiceState::Running { muted: true } => Tone::Bad,
                    VoiceState::Running { .. } | VoiceState::WaitingForMicrophone => Tone::Good,
                    _ => Tone::Neutral,
                },
            ),
        }
    }

    fn chain_section(&self, wide: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let activity = if self.model.voice_running() && !self.model.muted() {
            self.visuals.activity
        } else {
            0.
        };
        let phase = self.wire_phase;
        // Narrow windows wrap the chain into rows and drop the wires.
        let mut row = if wide {
            div().flex().items_center()
        } else {
            div().flex().flex_wrap().gap(rems(0.5))
        };
        let stages = p.stages();
        for (i, module) in Module::ALL.into_iter().enumerate() {
            let (value, tone) = self.module_summary(module);
            let selected = self.module == module;
            let hue = stages[i];
            row = row.child(
                div()
                    .id(module.id())
                    .debug_selector(move || module.id().to_string())
                    .flex()
                    .flex_col()
                    .gap(rems(0.25))
                    .flex_1()
                    .min_w(rems(if wide { 4.5 } else { 8. }))
                    .px(rems(0.7))
                    .py(rems(0.5))
                    .border_1()
                    .border_color(if selected {
                        hue
                    } else {
                        kit::mix(p.muted, hue, 0.35)
                    })
                    .when(selected, |m| m.bg(hue.opacity(0.12)))
                    .cursor_pointer()
                    .hover(|m| m.bg(p.fill))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.module = module;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(rems(0.4))
                            .child(kit::tracked(
                                vec![(module.name().into(), hue)],
                                0.58,
                                window,
                                cx,
                            ))
                            .child(kit::dot(
                                if tone == Tone::Neutral {
                                    p.dim
                                } else {
                                    super::tone_color(tone, &p)
                                },
                                tone != Tone::Neutral,
                            )),
                    )
                    .child(
                        div()
                            .text_color(p.title)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(value),
                    ),
            );
            if wide && i + 1 < Module::ALL.len() {
                row = row.child(wire(phase, activity, p.muted, (hue, stages[i + 1])));
            }
        }
        let right = if self.model.restart_needed() {
            div().text_color(p.yel).child("Changes need a restart · a")
        } else {
            div().text_color(p.dim).child("[ ] to move")
        };
        div()
            .child(kit::section(
                "Chain",
                Some(right.into_any_element()),
                window,
                cx,
            ))
            .child(row)
            .into_any_element()
    }

    fn voice_control(
        &self,
        key: &'static str,
        label: &'static str,
        bipolar: bool,
        disabled: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let range = voice_range(key);
        let value = self.model.settings.voice.get(key);
        let entity = cx.entity();
        let on_change: OnChange = Rc::new(move |value, _, cx| {
            entity.update(cx, |this, cx| {
                let effects = this.model.set_voice(key, value);
                this.run(effects, cx);
            });
        });
        kit::slider(
            key,
            label,
            value,
            format(key, value),
            Range {
                min: range.min,
                max: range.max,
                step: range.step,
                neutral: if bipolar {
                    0.
                } else {
                    omalive_core::VoiceSettings::default().get(key)
                },
                bipolar,
            },
            &self.focus_for(key),
            disabled,
            on_change,
            window,
            cx,
        )
    }

    fn module_section(
        &mut self,
        wide: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let module = self.module;
        let (summary, _) = self.module_summary(module);
        let voice = self.model.settings.voice.clone();
        let mut body = div().flex().flex_col();
        let mut toggle: Option<AnyElement> = None;
        match module {
            Module::Input => {
                if self.model.inputs.is_empty() {
                    body = body.child(div().text_color(p.dim).child(match self.model.connected {
                        None => "Connecting to PipeWire…",
                        Some(false) => "PipeWire is unavailable",
                        Some(true) => "No microphones found",
                    }));
                }
                for input in self.model.inputs.clone() {
                    let name = input.name.clone();
                    body = body.child(kit::list_row(
                        SharedString::from(format!("input-{}", input.name)),
                        input.description.clone(),
                        format!("{} ch", input.ports.len()),
                        input.name == voice.microphone,
                        cx.listener(move |this, _, _, cx| {
                            let effects = this.model.select_input(&name);
                            this.run(effects, cx);
                        }),
                        cx,
                    ));
                }
                if let Some(input) = self.model.selected_input().cloned()
                    && input.ports.len() > 1
                {
                    let mut choices = div().flex().gap(rems(0.6));
                    for port in input.ports {
                        let name = port.name.clone();
                        let selected = port.name == voice.port;
                        let id = SharedString::from(format!("port-{}", port.name));
                        let selector = id.to_string();
                        choices = choices.child(
                            kit::choice(id, port.label.clone(), selected, cx)
                                .debug_selector(move || selector.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let effects = this.model.select_port(&name);
                                    this.run(effects, cx);
                                })),
                        );
                    }
                    body = body
                        .child(
                            div()
                                .mt(rems(0.9))
                                .mb(rems(0.5))
                                .child(kit::label("Channel", window, cx)),
                        )
                        .child(choices);
                }
            }
            Module::LowCut => {
                body = body.child(kit::columns(
                    vec![self.voice_control("highpass", "Frequency", false, false, window, cx)],
                    wide,
                ))
            }
            Module::Denoise => {
                let available = self.model.plugins.noise.is_some();
                if available {
                    toggle = Some(self.module_switch("suppression", voice.suppression, cx));
                    body = body.child(kit::columns(
                        vec![self.voice_control(
                            "vad",
                            "Voice gate",
                            false,
                            !voice.suppression,
                            window,
                            cx,
                        )],
                        wide,
                    ));
                } else {
                    body = body.child(missing(
                        "RNNoise is not installed · noise-suppression-for-voice",
                        &p,
                    ));
                }
            }
            Module::Eq => {
                let preset = voice.preset();
                let mut choices = div().flex().gap(rems(0.6));
                for candidate in Preset::ALL {
                    let id = format!("preset-{}", candidate.name().to_lowercase());
                    let selector = id.clone();
                    choices = choices.child(
                        kit::choice(
                            SharedString::from(id),
                            candidate.name(),
                            preset == Some(candidate),
                            cx,
                        )
                        .debug_selector(move || selector.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let effects = this.model.apply_preset(candidate);
                            this.run(effects, cx);
                        })),
                    );
                }
                if preset.is_none() {
                    choices = choices.child(
                        kit::choice("preset-custom", "Custom", true, cx)
                            .debug_selector(|| "preset-custom".into()),
                    );
                }
                body = body
                    .child(div().mb(rems(0.5)).child(kit::label("Preset", window, cx)))
                    .child(div().mb(rems(1.)).child(choices))
                    .child(kit::columns(
                        vec![
                            self.voice_control("low", "Body · 150 Hz", true, false, window, cx),
                            self.voice_control("mud", "Mud · 300 Hz", true, false, window, cx),
                            self.voice_control("mid", "Presence · 3 kHz", true, false, window, cx),
                            self.voice_control("high", "Air · 10 kHz", true, false, window, cx),
                        ],
                        wide,
                    ));
            }
            Module::Compressor => {
                let available = self.model.plugins.compressor.is_some();
                if available {
                    let on = voice.compression;
                    toggle = Some(self.module_switch("compression", on, cx));
                    let controls = [
                        ("threshold", "Threshold"),
                        ("ratio", "Ratio"),
                        ("makeup", "Makeup"),
                        ("attack", "Attack"),
                        ("release", "Release"),
                    ]
                    .into_iter()
                    .map(|(key, label)| self.voice_control(key, label, false, !on, window, cx))
                    .collect();
                    body = body.child(kit::columns(controls, wide));
                } else {
                    body = body.child(missing("SC4 is not installed · swh-plugins", &p));
                }
            }
            Module::Output => {
                body = body
                    .child(kit::columns(
                        vec![self.voice_control("output", "Trim", true, false, window, cx)],
                        wide,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(rems(1.))
                            .child(div().text_color(p.dim).child("Ceiling"))
                            .child(
                                div().text_color(p.title).child(
                                    "−1.0 dB limiter · choose “Omalive Mic” in your call app",
                                ),
                            ),
                    );
            }
        }
        if self.model.restart_needed() {
            body = body.child(
                div()
                    .mt(rems(0.9))
                    .flex()
                    .flex_wrap()
                    .gap(rems(1.))
                    .items_center()
                    .text_color(p.yel)
                    .child("Input or effects changed · restarting briefly interrupts the mic")
                    .child(kit::button("apply", "Apply", "a", cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            let effects = this.model.start_voice();
                            this.run(effects, cx);
                        },
                    ))),
            );
        }
        let title = kit::tracked(
            vec![(format!("{} · ", module.name()), p.dim), (summary, p.fg)],
            0.7,
            window,
            cx,
        );
        div()
            .debug_selector(|| "module-panel".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(rems(0.9))
                    .child(title)
                    .children(toggle),
            )
            .child(body)
            .into_any_element()
    }

    /// The square switch that bypasses an effect module.
    fn module_switch(&self, key: &'static str, on: bool, cx: &mut Context<Self>) -> AnyElement {
        kit::switch(key, on, true, cx)
            .debug_selector(move || format!("toggle-{key}"))
            .track_focus(&self.focus_for(key))
            .on_click(cx.listener(move |this, _, _, cx| {
                let effects = match key {
                    "suppression" => this
                        .model
                        .set_suppression(!this.model.settings.voice.suppression),
                    _ => this
                        .model
                        .set_compression(!this.model.settings.voice.compression),
                };
                this.run(effects, cx);
            }))
            .into_any_element()
    }
}

fn missing(text: &'static str, p: &kit::Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(rems(0.6))
        .text_color(p.red)
        .child(kit::dot(p.red, true))
        .child(text)
}

/// The link between two chain modules: a rule blending one stage's colour
/// into the next, with pulses moving along it while the voice is live.
fn wire(phase: f32, activity: f32, muted: Hsla, (from, to): (Hsla, Hsla)) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let y = b.origin.y + b.size.height / 2.;
            window.paint_quad(fill(
                Bounds::new(point(b.origin.x, y), size(b.size.width, px(1.))),
                linear_gradient(
                    90.,
                    linear_color_stop(kit::mix(muted, from, 0.5), 0.),
                    linear_color_stop(kit::mix(muted, to, 0.5), 1.),
                ),
            ));
            if activity > 0.02 {
                let width = b.size.width / px(1.);
                for k in 0..2 {
                    let t = (phase + k as f32 * 0.5).fract();
                    let x = b.origin.x + px(t * (width - 3.));
                    let color = kit::mix(from, to, t);
                    window.paint_quad(fill(
                        Bounds::new(point(x - px(2.), y - px(2.5)), size(px(7.), px(6.))),
                        color.opacity(0.2 * activity),
                    ));
                    window.paint_quad(fill(
                        Bounds::new(point(x, y - px(1.)), size(px(3.), px(3.))),
                        color.opacity(0.35 + 0.65 * activity),
                    ));
                }
            }
        },
    )
    .w(rems(1.6))
    .flex_shrink_0()
    .h(rems(1.))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_axes_map_frequency_and_gain() {
        assert!(x_of(20.).abs() < 1e-6 && (x_of(20000.) - 1.).abs() < 1e-6);
        assert!((eq_y(0.) - 0.5).abs() < 1e-6);
        assert!(eq_y(EQ_RANGE) < 1e-6 && (eq_y(-99.) - 1.).abs() < 1e-6);
        assert_eq!(format("low", 2.), "+2.0 dB");
        assert_eq!(format("low", -0.01), "+0.0 dB");
        assert_eq!(format("ratio", 3.), "3.0 : 1");
        assert_eq!(db(-6.04), "−6.0 dB");
    }
}
