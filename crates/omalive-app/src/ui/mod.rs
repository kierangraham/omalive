//! The omalive window in Omarchy Quattro's panel language: a header with
//! the page icon, title, live status, page tabs and a power switch; the
//! Voice and Camera pages; and a status and key-hint footer. Views read the [`Model`] and the animated
//! [`AudioVisuals`]; user actions go through the model and its [`Effect`]s
//! go to the [`Engine`].
mod camera;
mod kit;
mod test;
#[cfg(test)]
mod tests;
mod voice;

use crate::{
    control::{self, Command, Hub, Status},
    engine::Engine,
    model::{Effect, Model, Page, Tone},
    visuals::{AudioVisuals, Spring, parade},
};
use gpui_kit::{
    AnyElement, App, AppContext, Bounds, Context, FocusHandle, Focusable, Hsla, IntoElement,
    KeyBinding, Pixels, Render, RenderImage, Window, actions, div, prelude::*, px, rems,
};
use gpui_omarchy::{IconName, focus_scope, icon};
pub use kit::init as init_theme;
use kit::{Palette, palette};
use omalive_media::{CameraState, Event, VoiceState, grader::SWATCH};
use std::{
    cell::Cell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
pub use voice::Module;

actions!(
    omalive,
    [
        VoicePage,
        CameraPage,
        Toggle,
        Mute,
        Compare,
        Apply,
        NextModule,
        PreviousModule,
        Import,
        OpenFolder,
        ResetGrade,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Test,
        Quit
    ]
);

pub fn bind_keys(cx: &mut App) {
    let normal = Some("Omalive && !Input && !Popover");
    cx.bind_keys([
        KeyBinding::new("1", VoicePage, normal),
        KeyBinding::new("2", CameraPage, normal),
        KeyBinding::new("s", Toggle, normal),
        KeyBinding::new("m", Mute, normal),
        KeyBinding::new("b", Compare, normal),
        KeyBinding::new("a", Apply, normal),
        KeyBinding::new("]", NextModule, normal),
        KeyBinding::new("[", PreviousModule, normal),
        KeyBinding::new("i", Import, normal),
        KeyBinding::new("o", OpenFolder, normal),
        KeyBinding::new("r", ResetGrade, normal),
        KeyBinding::new("t", Test, normal),
        KeyBinding::new("q", Quit, normal),
        KeyBinding::new("ctrl-q", Quit, Some("Omalive")),
        KeyBinding::new("ctrl-=", ZoomIn, Some("Omalive")),
        KeyBinding::new("ctrl--", ZoomOut, Some("Omalive")),
        KeyBinding::new("ctrl-0", ZoomReset, Some("Omalive")),
    ]);
}

/// Horizontal window padding, in rems.
pub(crate) const PAD_X: f32 = 1.6;
/// Upper bound for animation frames per second.
const FRAME_RATE: f32 = 30.;
/// Preview size requested from the camera thread.
const PREVIEW: (u32, u32) = (960, 540);
/// Keys of every adjustable control, so each gets a stable focus handle.
const FOCUS_KEYS: [&str; 24] = [
    "vad",
    "highpass",
    "low",
    "mud",
    "mid",
    "high",
    "threshold",
    "ratio",
    "attack",
    "release",
    "makeup",
    "output",
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "warmth",
    "tint",
    "vibrance",
    "saturation",
    "strength",
    "suppression",
    "compression",
    "on-demand",
];

pub(crate) fn tone_color(tone: Tone, p: &Palette) -> Hsla {
    match tone {
        Tone::Neutral => p.dim,
        Tone::Good => p.grn,
        Tone::Warning => p.yel,
        Tone::Bad => p.red,
    }
}

pub struct Omalive {
    pub(crate) model: Model,
    engine: Box<dyn Engine>,
    demo: bool,
    /// Where settings are saved; `None` keeps them in memory (tests, demo).
    config: Option<PathBuf>,
    focus: FocusHandle,
    focus_handles: HashMap<&'static str, FocusHandle>,
    pub(crate) zoom: f32,
    pub(crate) module: Module,
    pub(crate) visuals: AudioVisuals,
    spectrum_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag_node: Option<usize>,
    wire_phase: f32,
    last_frame: Option<Instant>,
    /// A capped animation frame is already scheduled.
    frame_pending: bool,
    /// Cached graph masks and dot fields.
    images: kit::Images,
    meter_on: bool,
    preview_size: Option<(u32, u32)>,
    pub(crate) preview: Option<Arc<RenderImage>>,
    original: Option<Arc<RenderImage>>,
    preview_seen: u64,
    parade: Vec<[f32; 3]>,
    /// The looks carousel: the first look in view, the looks the camera
    /// renders swatches for, and the newest swatch images.
    looks_from: usize,
    /// The chosen look when the carousel last scrolled to it.
    revealed: Option<String>,
    swatch_files: Vec<String>,
    swatches: Vec<Arc<RenderImage>>,
    split: Spring,
    split_target: f32,
    pad: (Spring, Spring),
    pad_bounds: Rc<Cell<Bounds<Pixels>>>,
    preview_bounds: Rc<Cell<Bounds<Pixels>>>,
    cpu: Option<f32>,
    cpu_sample: Option<(Instant, u64)>,
    save_generation: u64,
    /// The window is open; work that only feeds the screen runs only then.
    visible: bool,
    /// The control socket's status feed for the bar.
    hub: Option<Arc<Hub>>,
    pub(crate) test: test::VoiceTest,
    playback: Option<test::Playback>,
}

impl Omalive {
    pub fn new(
        model: Model,
        engine: Box<dyn Engine>,
        events: async_channel::Receiver<Event>,
        demo: bool,
        config: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |this, cx| this.event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // The CPU figure ticks every two seconds; nothing else polls.
        cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.tick_cpu(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        })
        .detach();
        // Release devices before the process exits.
        cx.on_app_quit(|this, _| {
            this.stop_playback();
            this.engine.apply(Effect::StopCamera);
            this.engine.apply(Effect::StopVoice);
            this.save_now();
            async {}
        })
        .detach();
        // The bar mirrors the theme, so tell it when the theme changes.
        cx.observe_global::<kit::ThemeSlots>(|this, cx| this.publish(cx))
            .detach();
        let focus = cx.focus_handle();
        let focus_handles = FOCUS_KEYS
            .iter()
            .map(|key| (*key, cx.focus_handle()))
            .collect();
        let mut model = model;
        model.cameras = engine.cameras();
        let grade = model.settings.camera.grade;
        Self {
            model,
            engine,
            demo,
            config,
            focus,
            focus_handles,
            zoom: 1.,
            module: Module::Input,
            visuals: AudioVisuals::default(),
            spectrum_bounds: Rc::default(),
            drag_node: None,
            wire_phase: 0.,
            last_frame: None,
            frame_pending: false,
            images: kit::Images::default(),
            meter_on: false,
            preview_size: None,
            preview: None,
            original: None,
            preview_seen: 0,
            parade: Vec::new(),
            looks_from: 0,
            revealed: None,
            swatch_files: Vec::new(),
            swatches: Vec::new(),
            split: Spring::at(0.5),
            split_target: 0.5,
            pad: (
                Spring::at(0.5 + grade.warmth / 2.),
                Spring::at(0.5 - grade.tint / 2.),
            ),
            pad_bounds: Rc::default(),
            preview_bounds: Rc::default(),
            cpu: None,
            cpu_sample: None,
            save_generation: 0,
            visible: false,
            hub: None,
            test: test::VoiceTest::Idle,
            playback: None,
        }
    }

    /// Serve status to the bar through `hub`.
    pub fn set_hub(&mut self, hub: Arc<Hub>, cx: &mut Context<Self>) {
        self.hub = Some(hub);
        self.publish(cx);
    }

    /// The window closed: processing carries on, but everything that only
    /// feeds the screen stops, and the camera may sleep again.
    pub fn hidden(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.sync_activity(cx);
        self.preview = None;
        self.original = None;
        self.swatches.clear();
        self.publish(cx);
    }

    /// Whether the background note was already shown; marks it shown.
    pub fn background_explained(&mut self, cx: &mut Context<Self>) -> bool {
        let app = &mut self.model.settings.app;
        let before = app.background_explained;
        app.background_explained = true;
        if !before {
            self.schedule_save(cx);
        }
        before
    }

    fn toggle_start_at_login(&mut self, cx: &mut Context<Self>) {
        let on = !self.model.settings.app.start_at_login;
        let binary = std::env::current_exe().unwrap_or_else(|_| "omalive".into());
        match crate::autostart::set(&crate::autostart::dir(), on, &binary) {
            Ok(()) => {
                self.model.settings.app.start_at_login = on;
                self.model.say(
                    if on {
                        "omalive will start in the background when you log in."
                    } else {
                        "omalive will no longer start at login."
                    },
                    Tone::Neutral,
                );
                self.run(vec![Effect::Save], cx);
            }
            Err(error) => {
                self.model.say(format!("{error:#}"), Tone::Bad);
                cx.notify();
            }
        }
    }

    /// Restart voice and camera if they were on when omalive last ran.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        let app = &self.model.settings.app;
        if !app.resume {
            return;
        }
        let (voice, camera) = (app.voice_on, app.camera_on);
        let mut effects = Vec::new();
        if voice {
            effects.extend(self.model.start_voice());
        }
        if camera {
            effects.extend(self.model.toggle_camera(self.demo));
        }
        self.run(effects, cx);
    }

    /// A command from the bar.
    pub fn command(&mut self, command: Command, cx: &mut Context<Self>) {
        let effects = match command {
            Command::ToggleVoice => self.model.toggle_voice(),
            Command::ToggleMute => self.model.toggle_mute(),
            Command::ToggleCamera => self.model.toggle_camera(self.demo),
            Command::Bypass => self.model.toggle_compare(),
            Command::Look { file } => {
                let known = file.is_empty() || self.model.luts.iter().any(|l| l.file == file);
                if known {
                    self.model.select_lut(&file)
                } else {
                    Vec::new()
                }
            }
            Command::Clients(count) => {
                self.engine.set_bar(count > 0);
                Vec::new()
            }
            // Pictures are handled by the control server itself.
            Command::Open | Command::Quit | Command::Picture { .. } => Vec::new(),
        };
        self.run(effects, cx);
    }

    /// Send the bar the current state; the hub drops unchanged repeats.
    fn publish(&self, cx: &App) {
        if let Some(hub) = &self.hub {
            hub.publish(self.status(cx));
        }
    }

    fn status(&self, cx: &App) -> Status {
        let model = &self.model;
        let voice = match model.voice {
            VoiceState::Off => "off",
            VoiceState::Starting => "starting",
            VoiceState::WaitingForMicrophone => "no-input",
            VoiceState::Running { muted: true } => "muted",
            VoiceState::Running { muted: false } => "live",
        };
        let (camera, format) = match &model.camera {
            CameraState::Off => ("off", String::new()),
            CameraState::Starting => ("starting", String::new()),
            CameraState::Failed(_) => ("failed", String::new()),
            CameraState::Running {
                width, height, fps, ..
            } => (
                if model.viewers {
                    "live"
                } else if model.camera_active {
                    "ready"
                } else {
                    "standby"
                },
                format!("{width}×{height} · {fps} fps"),
            ),
        };
        let camera_settings = &model.settings.camera;
        let look = model
            .luts
            .iter()
            .find(|l| l.file == camera_settings.lut)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        let p = palette(cx);
        let hex = |c: Hsla| {
            let c = gpui_kit::Rgba::from(c);
            let byte = |v: f32| (v.clamp(0., 1.) * 255. + 0.5) as u8;
            format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
        };
        Status {
            voice,
            listeners: model.listeners,
            camera,
            format,
            look,
            bypass: camera_settings.bypass,
            window: self.visible,
            theme: control::Theme {
                fg: hex(p.fg),
                muted: hex(p.muted),
                accent: hex(p.hi),
                cyan: hex(p.cyn),
                blue: hex(p.blu),
                magenta: hex(p.mag),
                red: hex(p.red),
                orange: hex(p.org),
                yellow: hex(p.yel),
                green: hex(p.grn),
            },
        }
    }

    pub(crate) fn focus_for(&self, key: &str) -> FocusHandle {
        self.focus_handles
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.focus.clone())
    }

    pub(crate) fn event(&mut self, event: Event, cx: &mut Context<Self>) {
        if let Event::Cameras(cameras) = event {
            let effects = self.model.set_cameras(cameras, self.demo);
            self.run(effects, cx);
            return;
        }
        let preview = matches!(event, Event::Preview);
        match &event {
            // A fresh camera thread needs the preview size again.
            Event::Camera(CameraState::Running { .. }) => self.preview_size = None,
            Event::Voice(_) => {
                if let Some(taps) = self.engine.taps() {
                    self.visuals.resync(&taps);
                }
            }
            _ => {}
        }
        self.model.event(event);
        // Preview frames only matter while they are on screen.
        if !preview {
            self.publish(cx);
        }
        if !preview || self.model.page == Page::Camera {
            cx.notify();
        }
    }

    pub(crate) fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        let mut save = false;
        for effect in effects {
            // Remember what is on, so a restart can resume it.
            let app = &mut self.model.settings.app;
            let was = (app.voice_on, app.camera_on);
            match &effect {
                Effect::StartVoice(..) => app.voice_on = true,
                Effect::StopVoice => app.voice_on = false,
                Effect::StartCamera { .. } => app.camera_on = true,
                Effect::StopCamera => app.camera_on = false,
                _ => {}
            }
            save |= was != (app.voice_on, app.camera_on);
            match effect {
                Effect::Save => save = true,
                effect => self.engine.apply(effect),
            }
        }
        if save {
            self.schedule_save(cx);
        }
        self.publish(cx);
        cx.notify();
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.config.is_none() {
            return;
        }
        self.save_generation += 1;
        let generation = self.save_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |this, _| {
                if this.save_generation == generation {
                    this.save_now();
                }
            });
        })
        .detach();
    }

    fn save_now(&mut self) {
        if let Some(dir) = &self.config
            && let Err(error) = self.model.settings.save(dir)
        {
            self.model
                .say(format!("Settings could not be saved: {error:#}"), Tone::Bad);
        }
    }

    pub(crate) fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.model.page != page {
            self.model.page = page;
            self.last_frame = None;
            cx.notify();
        }
    }

    /// Two columns when the window is wide enough.
    pub(crate) fn wide(&self, window: &Window) -> bool {
        window.viewport_size().width / px(1.) >= 760. * self.zoom
    }

    fn tick_cpu(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            self.cpu_sample = None;
            return;
        }
        let before = self.cpu.map(|c| (c * 10.).round());
        if let Some(ticks) = cpu_ticks() {
            let now = Instant::now();
            if let Some((then, before)) = self.cpu_sample {
                let elapsed = (now - then).as_secs_f32();
                if elapsed > 0. {
                    // CLK_TCK is 100 on Linux.
                    self.cpu = Some(ticks.saturating_sub(before) as f32 / 100. / elapsed * 100.);
                }
            }
            self.cpu_sample = Some((now, ticks));
        }
        // Repaint only when the shown figure changes.
        if self.cpu.map(|c| (c * 10.).round()) != before {
            cx.notify();
        }
    }

    /// Run only the work that is on screen: the voice meter and taps, the
    /// camera preview, and per-frame animation.
    fn sync_activity(&mut self, cx: &mut Context<Self>) {
        let voice_live =
            self.visible && self.model.page == Page::Voice && self.model.voice_running();
        if voice_live != self.meter_on {
            self.meter_on = voice_live;
            self.engine.set_meter(voice_live);
            if let Some(taps) = self.engine.taps() {
                self.visuals.resync(&taps);
            }
        }
        let preview =
            (self.visible && self.model.page == Page::Camera && self.model.camera_running())
                .then_some(PREVIEW);
        if preview != self.preview_size {
            self.preview_size = preview;
            self.engine.set_preview(preview);
        }
        self.sync_swatches();
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(1. / 60., |t| (now - t).as_secs_f32());
        let mut animate = false;
        if voice_live && let Some(taps) = self.engine.taps() {
            self.visuals.update(&taps, &self.model.settings.voice, dt);
            self.wire_phase = (self.wire_phase + dt * 0.9).fract();
            animate = true;
        }
        self.advance_test(voice_live && !self.model.muted());
        if self.visible && self.model.page == Page::Camera {
            let grade = self.model.settings.camera.grade;
            let targets = [
                self.split_target,
                0.5 + grade.warmth / 2.,
                0.5 - grade.tint / 2.,
            ];
            for (spring, target) in [&mut self.split, &mut self.pad.0, &mut self.pad.1]
                .into_iter()
                .zip(targets)
            {
                if !spring.settled(target) {
                    spring.step(target, dt);
                    animate = true;
                }
            }
        }
        if animate {
            self.last_frame = Some(now);
            self.schedule_frame(cx);
        } else {
            self.last_frame = None;
        }
    }

    /// Animate at [`FRAME_RATE`] at most. Displays refresh at 60–120 Hz and
    /// a frame of this UI costs a few milliseconds of main-thread time, so
    /// following the display would multiply the cost for no visible gain.
    fn schedule_frame(&mut self, cx: &mut Context<Self>) {
        if self.frame_pending {
            return;
        }
        self.frame_pending = true;
        let interval = Duration::from_secs_f32(1. / FRAME_RATE);
        let wait = self
            .last_frame
            .map_or(interval, |t| interval.saturating_sub(t.elapsed()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                this.frame_pending = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn take_preview(&mut self, window: &mut Window) {
        if self.model.page != Page::Camera {
            return;
        }
        let Some(frame) = self.engine.take_preview(self.preview_seen) else {
            return;
        };
        self.preview_seen = frame.sequence;
        self.parade = parade(
            &frame.data,
            frame.width,
            frame.height,
            camera::PARADE.0,
            camera::PARADE.1,
        );
        let image = |width: u32, height: u32, data: Vec<u8>| {
            image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(width, height, data).map(
                |buffer| {
                    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                        buffer
                    )]))
                },
            )
        };
        let (sw, sh) = SWATCH;
        let swatches = frame
            .swatches
            .chunks_exact(sw as usize * sh as usize * 4)
            .filter_map(|pixels| image(sw, sh, pixels.to_vec()))
            .collect();
        for old in std::mem::replace(&mut self.swatches, swatches) {
            let _ = window.drop_image(old);
        }
        let (width, height) = (frame.width, frame.height);
        let graded = image(width, height, frame.data);
        let original = image(width, height, frame.extra);
        // Release the previous frames' atlas entries so the preview never leaks.
        let old = [
            std::mem::replace(&mut self.preview, graded),
            std::mem::replace(&mut self.original, original),
        ];
        for old in old.into_iter().flatten() {
            let _ = window.drop_image(old);
        }
    }

    fn import_lut(&mut self, cx: &mut Context<Self>) {
        let pick = cx.background_executor().spawn(async {
            rfd::FileDialog::new()
                .set_title("Import looks")
                .add_filter("3D LUT or Hald CLUT", &["cube", "CUBE", "png", "PNG"])
                .pick_files()
        });
        cx.spawn(async move |this, cx| {
            if let Some(paths) = pick.await {
                let _ = this.update(cx, |this, cx| {
                    let effects = this.model.import_luts(&paths);
                    this.swatch_files.clear();
                    this.run(effects, cx);
                });
            }
        })
        .detach();
    }

    fn open_lut_folder(&mut self, cx: &mut Context<Self>) {
        let dir = self.model.lut_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        cx.open_url(&format!("file://{}", dir.display()));
        self.model.refresh_luts();
        self.swatch_files.clear();
        cx.notify();
    }

    fn header(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let (voice, voice_tone) = self.model.voice_status();
        let (camera, camera_tone) = self.model.camera_status();
        let tone = |tone: Tone| {
            if tone == Tone::Neutral {
                p.dim
            } else {
                tone_color(tone, &p)
            }
        };
        let mut status = vec![
            ("Voice ".to_string(), p.dim),
            (voice.to_string(), tone(voice_tone)),
            (" · Camera ".to_string(), p.dim),
            (camera.to_string(), tone(camera_tone)),
        ];
        if let Some(cpu) = self.cpu {
            status.push((format!(" · CPU {cpu:.1}%"), p.dim));
        }
        if self.demo {
            status.push((" · Demo".into(), p.yel));
        }
        // Narrow windows put the tabs on their own row and drop the icon.
        let compact = window.viewport_size().width / px(1.) < 520. * self.zoom;
        let (page_icon, on) = match self.model.page {
            Page::Voice => (
                if self.model.muted() {
                    IconName::MicOff
                } else {
                    IconName::Mic
                },
                self.model.voice_running() || self.model.voice_busy(),
            ),
            Page::Camera => (IconName::Webcam, self.model.camera_running()),
        };
        let title = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(rems(0.15))
            .child(kit::heading("omalive", cx))
            .child(
                div()
                    .debug_selector(|| "status".into())
                    .overflow_hidden()
                    .child(kit::tracked(status, 0.66, window, cx)),
            );
        let tabs = div()
            .flex()
            .gap(rems(1.1))
            .child(self.tab("tab-voice", "Voice", Page::Voice, window, cx))
            .child(self.tab("tab-camera", "Camera", Page::Camera, window, cx));
        let power = kit::switch("power", on, compact, cx)
            .debug_selector(|| "power".into())
            .on_click(cx.listener(|this, _, _, cx| {
                let effects = match this.model.page {
                    Page::Voice => this.model.toggle_voice(),
                    Page::Camera => this.model.toggle_camera(this.demo),
                };
                this.run(effects, cx);
            }));
        if compact {
            return div()
                .flex()
                .flex_col()
                .gap(rems(0.6))
                .flex_shrink_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rems(0.8))
                        .child(title)
                        .child(power),
                )
                .child(tabs)
                .into_any_element();
        }
        div()
            .flex()
            .items_center()
            .gap(rems(1.1))
            .flex_shrink_0()
            .child(icon(page_icon).size(rems(2.1)).text_color(p.fg))
            .child(title)
            .child(tabs)
            .child(power.ml(rems(0.6)))
            .into_any_element()
    }

    fn tab(
        &self,
        id: &'static str,
        label: &'static str,
        page: Page,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let selected = self.model.page == page;
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .pt(rems(0.35))
            .pb(rems(0.3))
            .border_b_2()
            .border_color(if selected {
                p.hi
            } else {
                gpui_kit::transparent_black()
            })
            .cursor_pointer()
            .hover(|t| t.border_color(if selected { p.hi } else { p.muted }))
            .child(kit::tracked(
                vec![(label.into(), if selected { p.title } else { p.dim })],
                0.75,
                window,
                cx,
            ))
            .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
            .into_any_element()
    }

    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let (text, tone) = match &self.model.message {
            Some(message) => (message.text.clone(), message.tone),
            None if self.model.connected == Some(false) => {
                ("PipeWire is unavailable".into(), Tone::Bad)
            }
            None => (String::new(), Tone::Neutral),
        };
        let keys: &[(&str, &str)] = match self.model.page {
            Page::Voice => &[
                ("1 2", "page"),
                ("s", "start"),
                ("m", "mute"),
                ("t", "test"),
                ("[ ]", "module"),
                ("q", "quit"),
            ],
            Page::Camera => &[
                ("1 2", "page"),
                ("s", "start"),
                ("b", "compare"),
                ("r", "reset"),
                ("i", "import"),
                ("q", "quit"),
            ],
        };
        div()
            .flex()
            .items_center()
            .gap(rems(1.1))
            .pt(rems(0.9))
            .flex_shrink_0()
            .whitespace_nowrap()
            .text_size(rems(0.7))
            .child(
                div()
                    .debug_selector(|| "status-message".into())
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(if tone == Tone::Neutral {
                        p.fg
                    } else {
                        tone_color(tone, &p)
                    })
                    .child(text),
            )
            .when(self.config.is_some(), |footer| {
                let on = self.model.settings.app.start_at_login;
                footer.child(
                    div()
                        .id("login")
                        .debug_selector(|| "login".into())
                        .flex()
                        .items_center()
                        .gap(rems(0.4))
                        .cursor_pointer()
                        .text_color(if on { p.fg } else { p.dim })
                        .hover(|t| t.text_color(p.title))
                        .child(kit::dot(if on { p.grn } else { p.muted }, on))
                        .child("start at login")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_start_at_login(cx))),
                )
            })
            .children(keys.iter().map(|(key, label)| {
                div()
                    .flex()
                    .gap(rems(0.4))
                    .child(div().text_color(p.fg).child(*key))
                    .child(div().text_color(p.dim).child(*label))
            }))
            .into_any_element()
    }

    /// `[` and `]`: the previous or next module on the Voice page, look on
    /// the Camera page.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        match self.model.page {
            Page::Voice => self.step_module(by, cx),
            Page::Camera => self.step_look(by, cx),
        }
    }

    fn step_module(&mut self, by: isize, cx: &mut Context<Self>) {
        let index = Module::ALL
            .iter()
            .position(|m| *m == self.module)
            .unwrap_or(0) as isize;
        self.module = Module::ALL[(index + by).rem_euclid(Module::ALL.len() as isize) as usize];
        cx.notify();
    }
}

/// This process's user and system CPU time in clock ticks.
fn cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the parenthesised command name; utime and stime are 14 and 15.
    let rest = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

impl Focusable for Omalive {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Omalive {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            self.visible = true;
            self.publish(cx);
        }
        window.set_rem_size(px(16. * self.zoom));
        // When the focused control disappears (a page or module switch),
        // return focus to the window so shortcuts keep working.
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        let p = palette(cx);
        self.sync_activity(cx);
        self.take_preview(window);
        let page = match self.model.page {
            Page::Voice => self.voice_page(window, cx),
            Page::Camera => self.camera_page(window, cx),
        };
        let header = self.header(window, cx);
        let footer = self.footer(cx);
        focus_scope("omalive")
            .track_focus(&self.focus)
            .key_context("Omalive OmarchyFocusScope")
            .size_full()
            .flex()
            .flex_col()
            .px(rems(PAD_X))
            .pt(rems(1.35))
            .pb(rems(0.9))
            .bg(p.bg)
            .text_color(p.fg)
            .font_family(p.mono.clone())
            .text_size(rems(0.8))
            .on_action(cx.listener(|this, _: &VoicePage, _, cx| this.go(Page::Voice, cx)))
            .on_action(cx.listener(|this, _: &CameraPage, _, cx| this.go(Page::Camera, cx)))
            .on_action(cx.listener(|this, _: &Toggle, _, cx| {
                let effects = match this.model.page {
                    Page::Voice => this.model.toggle_voice(),
                    Page::Camera => this.model.toggle_camera(this.demo),
                };
                this.run(effects, cx);
            }))
            .on_action(cx.listener(|this, _: &Mute, _, cx| {
                let effects = this.model.toggle_mute();
                this.run(effects, cx);
            }))
            .on_action(cx.listener(|this, _: &Compare, _, cx| {
                let effects = this.model.toggle_compare();
                this.run(effects, cx);
            }))
            .on_action(cx.listener(|this, _: &Apply, _, cx| {
                if this.model.restart_needed() {
                    let effects = this.model.start_voice();
                    this.run(effects, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextModule, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousModule, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &Import, _, cx| this.import_lut(cx)))
            .on_action(cx.listener(|this, _: &Test, _, cx| this.start_test(cx)))
            .on_action(cx.listener(|this, _: &OpenFolder, _, cx| this.open_lut_folder(cx)))
            .on_action(cx.listener(|this, _: &ResetGrade, _, cx| {
                if this.model.page == Page::Camera {
                    let effects = this.model.reset_grade();
                    this.run(effects, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.zoom = (this.zoom + 0.1).min(1.8);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.zoom = (this.zoom - 0.1).max(0.7);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| {
                this.zoom = 1.;
                cx.notify();
            }))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .child(header)
            .child(kit::divider(cx))
            .child(page)
            .child(footer)
    }
}

// `AppContext` brings `cx.new` into scope for the page modules.
#[allow(unused_imports)]
use AppContext as _;
