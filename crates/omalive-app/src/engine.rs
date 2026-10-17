//! What the UI needs from the media engine, as a trait, so views can be
//! driven by the real engine, a device-free demo, or a test recorder.
use crate::model::Effect;
use omalive_core::{Cube, Plugins};
use omalive_media::{
    CameraDevice, Emit, Event, Media, Meter, Taps, VoiceState,
    camera::{self, Camera, thumb::Thumb},
    frames::{Frame, Slot},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub trait Engine {
    fn apply(&mut self, effect: Effect);
    /// Run the level meter while it is visible (it keeps the chain awake).
    fn set_meter(&mut self, visible: bool);
    /// Preview size while the camera page is visible.
    fn set_preview(&mut self, size: Option<(u32, u32)>);
    /// LUTs to render as swatches with each preview frame (`None` is no LUT).
    fn set_swatches(&mut self, cubes: Vec<Option<Arc<Cube>>>);
    /// The newest preview frame after `seen`, if any.
    fn take_preview(&self, seen: u64) -> Option<Frame>;
    fn cameras(&self) -> Vec<CameraDevice>;
    fn plugins(&self) -> Plugins;
    /// Raw-microphone and output sample taps, live while the meter runs.
    fn taps(&self) -> Option<Arc<Taps>>;
    /// Feed the bar a level meter while an app listens to the microphone.
    fn set_bar(&mut self, on: bool);
    /// The output level meter and the camera picture, for the bar.
    fn bar_sources(&self) -> (Option<Arc<Meter>>, Option<Arc<Thumb>>);
}

/// The real engine.
pub struct Live {
    media: Media,
    meter_on: bool,
}

impl Live {
    pub fn start(emit: Emit) -> Self {
        Self {
            media: Media::start(emit),
            meter_on: false,
        }
    }
}

impl Engine for Live {
    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::StartVoice(settings, plugins) => self.media.start_voice(settings, plugins),
            Effect::StopVoice => self.media.stop_voice(),
            Effect::Controls(controls) => self.media.voice_controls(controls),
            Effect::Mute(muted) => self.media.mute(muted),
            Effect::StartCamera {
                source,
                look,
                on_demand,
            } => self.media.start_camera(source, look, None, on_demand),
            Effect::StopCamera => self.media.stop_camera(),
            Effect::Look(look) => self.media.look(look),
            Effect::OnDemand(on) => self.media.on_demand(on),
            Effect::Save => {}
        }
    }

    fn set_meter(&mut self, visible: bool) {
        if visible != self.meter_on {
            self.meter_on = visible;
            self.media.meter(visible);
        }
    }

    fn set_preview(&mut self, size: Option<(u32, u32)>) {
        self.media.preview(size);
    }

    fn set_swatches(&mut self, cubes: Vec<Option<Arc<Cube>>>) {
        self.media.swatches(cubes);
    }

    fn take_preview(&self, seen: u64) -> Option<Frame> {
        self.media.preview.take(seen)
    }

    fn cameras(&self) -> Vec<CameraDevice> {
        camera::devices()
    }

    fn plugins(&self) -> Plugins {
        Plugins::discover()
    }

    fn taps(&self) -> Option<Arc<Taps>> {
        Some(self.media.taps.clone())
    }

    fn set_bar(&mut self, on: bool) {
        self.media.bar(on);
    }

    fn bar_sources(&self) -> (Option<Arc<Meter>>, Option<Arc<Thumb>>) {
        (
            Some(self.media.meter.clone()),
            Some(self.media.thumb.clone()),
        )
    }
}

/// Demo mode: no PipeWire session, microphone or camera is touched. Voice
/// state is simulated and the camera is a graded test pattern.
pub struct Demo {
    emit: Emit,
    camera: Option<Camera>,
    preview: Arc<Slot>,
    preview_size: Option<(u32, u32)>,
    swatches: Vec<Option<Arc<Cube>>>,
    running: Arc<AtomicBool>,
    taps: Arc<Taps>,
    meter: Arc<Meter>,
    thumb: Arc<Thumb>,
}

impl Demo {
    pub fn start(emit: Emit) -> Self {
        emit(Event::Connected(true));
        emit(Event::Inputs(vec![
            demo_input(
                "demo.interface",
                "Scarlett 2i2 USB",
                &["capture_FL", "capture_FR"],
            ),
            demo_input("demo.laptop", "MacBook Pro Microphone", &["capture_MONO"]),
        ]));
        let running = Arc::new(AtomicBool::new(false));
        let taps = Arc::new(Taps::default());
        let meter = Arc::new(Meter::default());
        let generator = (running.clone(), taps.clone(), meter.clone());
        let _ = std::thread::Builder::new()
            .name("omalive-demo-audio".into())
            .spawn(move || demo_audio(&generator.0, &generator.1, &generator.2));
        Self {
            emit,
            camera: None,
            preview: Arc::new(Slot::default()),
            preview_size: None,
            swatches: Vec::new(),
            running,
            taps,
            meter,
            thumb: Arc::default(),
        }
    }
}

/// A synthetic talker for demo mode: voiced syllables with formants over
/// mains hum and hiss at the "microphone", a cleaner copy at the output.
fn demo_audio(running: &AtomicBool, taps: &Taps, meter: &Meter) {
    const RATE: f32 = 48000.;
    const CHUNK: usize = 480;
    let mut seed = 0x1234_5678u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2. - 1.
    };
    let (mut input, mut output) = (vec![0.; CHUNK], vec![0.; CHUNK]);
    let mut n = 0u64;
    let start = std::time::Instant::now();
    loop {
        if !running.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }
        for i in 0..CHUNK {
            let t = n as f32 / RATE;
            let syllable = ((t * 4.6).sin() * (t * 1.3 + 1.).sin()).max(0.);
            let envelope = (syllable * 2.2).min(1.).powf(1.5);
            let pitch = 118. + 18. * (t * 2.9).sin();
            let mut voice = 0.;
            for harmonic in 1..24 {
                let f = pitch * harmonic as f32;
                let formant = (-((f - 650.) / 260.).powi(2)).exp()
                    + 0.6 * (-((f - 1700.) / 380.).powi(2)).exp()
                    + 0.25 * (-((f - 3100.) / 500.).powi(2)).exp();
                voice += (std::f32::consts::TAU * f * t).sin() * (0.05 + formant) / harmonic as f32;
            }
            voice *= 0.22 * envelope;
            let hum = 0.012 * (std::f32::consts::TAU * 50. * t).sin()
                + 0.006 * (std::f32::consts::TAU * 100. * t).sin();
            let hiss = 0.01 * noise();
            input[i] = voice + hum + hiss;
            output[i] = voice * 1.25 + hiss * 0.04;
            n += 1;
        }
        taps.input.push(&input);
        taps.output.push(&output);
        let bytes: Vec<u8> = output.iter().flat_map(|s| s.to_le_bytes()).collect();
        meter.analyse(&bytes);
        // Pace in real time.
        let due = start + std::time::Duration::from_secs_f64(n as f64 / RATE as f64);
        if let Some(wait) = due.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(wait);
        }
    }
}

fn demo_input(name: &str, description: &str, ports: &[&str]) -> omalive_core::Input {
    omalive_core::Input {
        name: name.into(),
        description: description.into(),
        ports: ports
            .iter()
            .map(|port| omalive_core::Channel {
                name: (*port).into(),
                label: port.replace("capture_", ""),
            })
            .collect(),
    }
}

impl Engine for Demo {
    fn apply(&mut self, effect: Effect) {
        match effect {
            // A pretend call app listens whenever the demo voice runs.
            Effect::StartVoice(..) => {
                self.running.store(true, Ordering::Relaxed);
                (self.emit)(Event::Voice(VoiceState::Running { muted: false }));
                (self.emit)(Event::VoiceListeners(1));
            }
            Effect::StopVoice => {
                self.running.store(false, Ordering::Relaxed);
                (self.emit)(Event::Voice(VoiceState::Off));
                (self.emit)(Event::VoiceListeners(0));
            }
            Effect::Mute(muted) => (self.emit)(Event::Voice(VoiceState::Running { muted })),
            Effect::StartCamera {
                source,
                look,
                on_demand,
            } => {
                // The camera thread publishes to a PipeWire sender; in demo
                // mode nothing receives it. The pretend call app watches,
                // so the camera grades frames and the bar shows them.
                let (pipewire, _receiver) = pipewire::channel::channel();
                let video = Arc::new(Slot::default());
                video.want(omalive_media::frames::PixelFormat::Nv12);
                self.camera = Some(Camera::start(
                    source,
                    look,
                    self.preview_size,
                    self.swatches.clone(),
                    on_demand,
                    camera::Shared {
                        emit: self.emit.clone(),
                        video,
                        preview: self.preview.clone(),
                        thumb: self.thumb.clone(),
                        pipewire,
                    },
                ));
                (self.emit)(Event::CameraViewers(true));
            }
            Effect::StopCamera => self.camera = None,
            Effect::Look(look) => {
                if let Some(camera) = &self.camera {
                    camera.send(camera::Command::Look(look));
                }
            }
            Effect::OnDemand(on) => {
                if let Some(camera) = &self.camera {
                    camera.send(camera::Command::OnDemand(on));
                }
            }
            Effect::Controls(_) | Effect::Save => {}
        }
    }

    fn set_meter(&mut self, _: bool) {}

    fn set_preview(&mut self, size: Option<(u32, u32)>) {
        self.preview_size = size;
        if let Some(camera) = &self.camera {
            camera.send(camera::Command::Preview(size));
        }
    }

    fn set_swatches(&mut self, cubes: Vec<Option<Arc<Cube>>>) {
        if let Some(camera) = &self.camera {
            camera.send(camera::Command::Swatches(cubes.clone()));
        }
        self.swatches = cubes;
    }

    fn take_preview(&self, seen: u64) -> Option<Frame> {
        self.preview.take(seen)
    }

    fn cameras(&self) -> Vec<CameraDevice> {
        vec![CameraDevice {
            id: "demo".into(),
            path: "/dev/null".into(),
            name: "Test pattern".into(),
            sizes: vec![(1280, 720)],
        }]
    }

    fn plugins(&self) -> Plugins {
        Plugins {
            noise: Some("demo".into()),
            compressor: Some("demo".into()),
        }
    }

    fn taps(&self) -> Option<Arc<Taps>> {
        Some(self.taps.clone())
    }

    fn set_bar(&mut self, _: bool) {}

    fn bar_sources(&self) -> (Option<Arc<Meter>>, Option<Arc<Thumb>>) {
        (Some(self.meter.clone()), Some(self.thumb.clone()))
    }
}

/// Records effects for UI tests.
#[cfg(test)]
pub struct Recorder {
    pub effects: std::rc::Rc<std::cell::RefCell<Vec<Effect>>>,
    pub meter: std::rc::Rc<std::cell::Cell<bool>>,
    pub bar: std::rc::Rc<std::cell::Cell<bool>>,
    pub preview: std::rc::Rc<std::cell::Cell<Option<(u32, u32)>>>,
    /// Whether each requested swatch has a LUT.
    pub swatches: std::rc::Rc<std::cell::RefCell<Vec<bool>>>,
}

#[cfg(test)]
impl Engine for Recorder {
    fn apply(&mut self, effect: Effect) {
        if !matches!(effect, Effect::Save) {
            self.effects.borrow_mut().push(effect);
        }
    }
    fn set_meter(&mut self, visible: bool) {
        self.meter.set(visible);
    }
    fn set_preview(&mut self, size: Option<(u32, u32)>) {
        self.preview.set(size);
    }
    fn set_swatches(&mut self, cubes: Vec<Option<Arc<Cube>>>) {
        *self.swatches.borrow_mut() = cubes.iter().map(Option::is_some).collect();
    }
    fn take_preview(&self, _: u64) -> Option<Frame> {
        None
    }
    fn cameras(&self) -> Vec<CameraDevice> {
        vec![CameraDevice {
            id: "isp".into(),
            path: "/dev/video0".into(),
            name: "FaceTime HD Camera".into(),
            sizes: vec![(1920, 1080), (1280, 720)],
        }]
    }
    fn plugins(&self) -> Plugins {
        Plugins {
            noise: Some("rnnoise".into()),
            compressor: None,
        }
    }
    fn taps(&self) -> Option<Arc<Taps>> {
        None
    }
    fn set_bar(&mut self, on: bool) {
        self.bar.set(on);
    }
    fn bar_sources(&self) -> (Option<Arc<Meter>>, Option<Arc<Thumb>>) {
        (None, None)
    }
}
