//! omalive's media engine: the PipeWire thread (graph, voice chain, meter,
//! camera source) and the camera thread (capture, grading, outputs).
//! The UI holds a [`Media`] and receives [`Event`]s through its callback.
pub mod camera;
pub mod frames;
pub mod grader;
pub mod meter;
pub mod pw;

use omalive_core::{Input, Plugins, VoiceSettings};
use std::sync::Arc;

pub use camera::{CameraDevice, Look, SourceSpec};
pub use meter::{Levels, Meter, Tap, Taps};

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub enum VoiceState {
    Off,
    Starting,
    /// The chain is up and the virtual microphone stays available to call
    /// apps; it relinks automatically when the input returns.
    WaitingForMicrophone,
    Running {
        muted: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum CameraState {
    Off,
    Starting,
    Running {
        device: String,
        width: u32,
        height: u32,
        fps: u32,
        /// GPU adapter name, or "CPU".
        grader: String,
        /// v4l2loopback device being fed, if any.
        loopback: Option<String>,
    },
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub fps: f32,
    pub process_ms: f32,
    pub worst_ms: f32,
    /// Capture timestamp → outputs handed off, median and 95th percentile.
    pub latency_ms: Option<f32>,
    pub latency_p95_ms: Option<f32>,
    pub dropped: u32,
    /// Frames captured while no output wanted them (no work done).
    pub idle: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Connected(bool),
    Inputs(Vec<Input>),
    Voice(VoiceState),
    Camera(CameraState),
    /// The cameras present, at start and after each plug or unplug.
    Cameras(Vec<camera::CameraDevice>),
    CameraStats(Stats),
    /// Whether a PipeWire consumer is streaming the camera.
    CameraViewers(bool),
    /// How many apps are reading the virtual microphone.
    VoiceListeners(u32),
    /// Whether the camera is capturing (false while on-demand standby).
    CameraActive(bool),
    /// A new preview frame is in [`Media::preview`].
    Preview,
    Error(String),
    Warning(String),
}

pub struct Media {
    pipewire: pw::PipeWire,
    camera: Option<camera::Camera>,
    pub meter: Arc<Meter>,
    pub taps: Arc<Taps>,
    pub preview: Arc<frames::Slot>,
    pub thumb: Arc<camera::thumb::Thumb>,
    swatches: Vec<Option<Arc<omalive_core::Cube>>>,
    video: Arc<frames::Slot>,
    emit: Emit,
}

impl Media {
    pub fn start(emit: Emit) -> Self {
        let meter = Arc::new(Meter::default());
        let taps = Arc::new(Taps::default());
        let video = Arc::new(frames::Slot::default());
        let pipewire =
            pw::PipeWire::start(emit.clone(), meter.clone(), taps.clone(), video.clone());
        camera::watch(emit.clone());
        Self {
            pipewire,
            camera: None,
            meter,
            taps,
            preview: Arc::new(frames::Slot::default()),
            thumb: Arc::default(),
            swatches: Vec::new(),
            video,
            emit,
        }
    }

    pub fn start_voice(&self, settings: VoiceSettings, plugins: Plugins) {
        self.pipewire
            .send(pw::Command::VoiceStart(settings, plugins));
    }

    pub fn stop_voice(&self) {
        self.pipewire.send(pw::Command::VoiceStop);
    }

    pub fn voice_controls(&self, controls: Vec<(String, f32)>) {
        if !controls.is_empty() {
            self.pipewire.send(pw::Command::VoiceControls(controls));
        }
    }

    pub fn mute(&self, muted: bool) {
        self.pipewire.send(pw::Command::VoiceMute(muted));
    }

    pub fn meter(&self, visible: bool) {
        self.pipewire.send(pw::Command::Meter(visible));
    }

    /// Feed the bar a level meter while an app listens to the microphone.
    /// (The camera picture is wanted through [`Media::thumb`].)
    pub fn bar(&self, on: bool) {
        self.pipewire.send(pw::Command::BarMeter(on));
    }

    pub fn start_camera(
        &mut self,
        source: SourceSpec,
        look: Look,
        preview: Option<(u32, u32)>,
        on_demand: bool,
    ) {
        self.camera = None;
        self.camera = Some(camera::Camera::start(
            source,
            look,
            preview,
            self.swatches.clone(),
            on_demand,
            camera::Shared {
                emit: self.emit.clone(),
                video: self.video.clone(),
                preview: self.preview.clone(),
                thumb: self.thumb.clone(),
                pipewire: self.pipewire.sender(),
            },
        ));
    }

    pub fn stop_camera(&mut self) {
        self.camera = None;
    }

    pub fn camera_running(&self) -> bool {
        self.camera.is_some()
    }

    pub fn look(&self, look: Look) {
        if let Some(camera) = &self.camera {
            camera.send(camera::Command::Look(look));
        }
    }

    pub fn on_demand(&self, on_demand: bool) {
        if let Some(camera) = &self.camera {
            camera.send(camera::Command::OnDemand(on_demand));
        }
    }

    /// LUTs to show as swatches beside the preview (`None` is no LUT).
    pub fn swatches(&mut self, cubes: Vec<Option<Arc<omalive_core::Cube>>>) {
        if let Some(camera) = &self.camera {
            camera.send(camera::Command::Swatches(cubes.clone()));
        }
        self.swatches = cubes;
    }

    pub fn preview(&self, size: Option<(u32, u32)>) {
        if let Some(camera) = &self.camera {
            camera.send(camera::Command::Preview(size));
        }
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        // Stop the camera before the PipeWire thread it publishes to.
        self.camera = None;
    }
}
