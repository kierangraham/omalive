//! Camera capture, grading and output on one dedicated thread.
//!
//! The thread owns a [`Source`](pipeline::Source) and a
//! [`Pipeline`](pipeline::Pipeline). In on-demand mode the source only
//! streams while something wants frames, so the camera (and its light)
//! sleeps between calls.
pub mod pipeline;
pub mod synthetic;
pub mod thumb;
pub mod v4l2;
mod watch;

use crate::{
    CameraState, Emit, Event,
    grader::{Grader, SWATCH_LUT},
    pw,
};
use anyhow::Result;
use omalive_core::{Cube, Grade, Lattice, bake, grade::BAKE_SIZE};
use pipeline::{Outlets, Pipeline, Source};
use std::{
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub use v4l2::{CameraDevice, devices, loopback};
pub use watch::watch;

/// How long capture keeps running after the last consumer leaves, so a
/// call app renegotiating does not flicker the camera light.
const STANDBY_GRACE: Duration = Duration::from_secs(3);

/// Everything that determines the graded image. Baked on the camera thread,
/// newest look only, so dragging a slider never queues work.
#[derive(Clone, Debug, Default)]
pub struct Look {
    pub grade: Grade,
    pub cube: Option<Arc<Cube>>,
    pub strength: f32,
    pub bypass: bool,
}

impl Look {
    pub fn bake(&self) -> Lattice {
        if self.bypass || (self.grade.is_neutral() && self.cube.is_none()) {
            return Lattice::identity(2);
        }
        bake(&self.grade, self.cube.as_deref(), self.strength, BAKE_SIZE)
    }

    /// Small lattices for the look swatches: this grade with each LUT in
    /// place of the chosen one (`None` is no LUT), ignoring bypass.
    pub fn swatches(&self, cubes: &[Option<Arc<Cube>>]) -> Vec<Lattice> {
        cubes
            .iter()
            .map(|cube| bake(&self.grade, cube.as_deref(), self.strength, SWATCH_LUT))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum SourceSpec {
    Device(CameraDevice, (u32, u32)),
    /// Moving test pattern, or a panning picture (demo mode and tests).
    Synthetic {
        width: u32,
        height: u32,
        fps: u32,
        picture: Option<Arc<synthetic::Picture>>,
    },
}

pub enum Command {
    Look(Look),
    /// Preview size while the preview is on screen.
    Preview(Option<(u32, u32)>),
    /// LUTs to render as swatches with the preview (`None` is no LUT).
    Swatches(Vec<Option<Arc<Cube>>>),
    OnDemand(bool),
    Stop,
}

pub struct Shared {
    pub emit: Emit,
    pub video: Arc<crate::frames::Slot>,
    pub preview: Arc<crate::frames::Slot>,
    pub thumb: Arc<thumb::Thumb>,
    pub pipewire: pipewire::channel::Sender<pw::Command>,
}

pub struct Camera {
    commands: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Camera {
    pub fn start(
        spec: SourceSpec,
        look: Look,
        preview: Option<(u32, u32)>,
        swatches: Vec<Option<Arc<Cube>>>,
        on_demand: bool,
        shared: Shared,
    ) -> Self {
        let (commands, receiver) = mpsc::channel();
        for command in [
            Command::Look(look),
            Command::Preview(preview),
            Command::Swatches(swatches),
            Command::OnDemand(on_demand),
        ] {
            let _ = commands.send(command);
        }
        let thread = std::thread::Builder::new()
            .name("omalive-camera".into())
            .spawn(move || {
                (shared.emit)(Event::Camera(CameraState::Starting));
                match run(spec, &receiver, &shared) {
                    Ok(()) => (shared.emit)(Event::Camera(CameraState::Off)),
                    Err(error) => {
                        (shared.emit)(Event::Camera(CameraState::Failed(format!("{error:#}"))))
                    }
                }
                let _ = shared.pipewire.send(pw::Command::VideoStop);
            })
            .ok();
        Self { commands, thread }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Control {
    look: Option<Look>,
    swatches: Option<Vec<Option<Arc<Cube>>>>,
    on_demand: bool,
    stop: bool,
}

impl Control {
    fn apply(&mut self, command: Command, pipeline: &mut Pipeline) {
        match command {
            Command::Look(look) => self.look = Some(look),
            Command::Preview(size) => pipeline.preview = size,
            Command::Swatches(cubes) => self.swatches = Some(cubes),
            Command::OnDemand(on_demand) => self.on_demand = on_demand,
            Command::Stop => self.stop = true,
        }
    }
}

fn run(spec: SourceSpec, commands: &mpsc::Receiver<Command>, shared: &Shared) -> Result<()> {
    let (mut source, name): (Box<dyn Source>, String) = match &spec {
        SourceSpec::Device(device, size) => (
            Box::new(v4l2::V4l2Source::open(device, *size)?),
            device.name.clone(),
        ),
        SourceSpec::Synthetic {
            width,
            height,
            fps,
            picture,
        } => (
            Box::new({
                let source = synthetic::Synthetic::new(*width, *height, *fps).paced();
                match picture {
                    Some(picture) => source.with_picture(picture),
                    None => source,
                }
            }),
            "Test pattern".into(),
        ),
    };
    let format = source.format();
    let (grader, warning) = Grader::new();
    if let Some(warning) = warning {
        (shared.emit)(Event::Warning(warning));
    }
    let mut loopback_path = None;
    let loopback = match loopback() {
        Some((path, _)) => {
            match v4l2::open_loopback(&path, format.width, format.height, format.fps) {
                Ok(device) => {
                    loopback_path = Some(path.display().to_string());
                    Some(Box::new(device) as Box<dyn std::io::Write + Send>)
                }
                Err(error) => {
                    (shared.emit)(Event::Warning(format!(
                        "Virtual V4L2 camera unavailable ({}): {error:#}",
                        path.display()
                    )));
                    None
                }
            }
        }
        None => None,
    };
    let _ = shared.pipewire.send(pw::Command::VideoStart {
        width: format.width,
        height: format.height,
        fps: format.fps,
    });
    let video_sender = shared.pipewire.clone();
    let preview_emit = shared.emit.clone();
    let mut outlets = Outlets {
        video: shared.video.clone(),
        video_ready: Box::new(move || {
            let _ = video_sender.send(pw::Command::VideoFrame);
        }),
        loopback,
        preview: shared.preview.clone(),
        preview_ready: Box::new(move || preview_emit(Event::Preview)),
        thumb: shared.thumb.clone(),
    };
    let device_label = grader.device();
    let mut pipeline = Pipeline::new(grader, format);
    (shared.emit)(Event::Camera(CameraState::Running {
        device: name,
        width: format.width,
        height: format.height,
        fps: format.fps,
        grader: device_label,
        loopback: loopback_path,
    }));

    let mut control = Control {
        look: None,
        swatches: None,
        on_demand: false,
        stop: false,
    };
    let mut look = Look::default();
    let mut swatches = Vec::new();
    let mut streaming = false;
    let mut last_demand = Instant::now();
    loop {
        for command in commands.try_iter() {
            control.apply(command, &mut pipeline);
        }
        if control.stop {
            return Ok(());
        }
        let restyled = control.look.is_some() || control.swatches.is_some();
        if let Some(newest) = control.look.take() {
            pipeline.grader.set_lattice(&newest.bake());
            look = newest;
        }
        if let Some(newest) = control.swatches.take() {
            swatches = newest;
        }
        if restyled {
            pipeline.set_swatches(&look.swatches(&swatches));
        }
        if !control.on_demand || pipeline.demanded(&outlets) {
            last_demand = Instant::now();
        }
        let active = last_demand.elapsed() < STANDBY_GRACE;
        if active != streaming {
            source.set_streaming(active)?;
            streaming = active;
            (shared.emit)(Event::CameraActive(active));
        }
        if !streaming {
            // Standby: sleep until a command arrives, checking demand a few
            // times a second (consumers appear through the shared slots).
            if let Ok(command) = commands.recv_timeout(Duration::from_millis(200)) {
                control.apply(command, &mut pipeline);
            }
            continue;
        }
        if let Some(captured) = source.next()? {
            pipeline.frame(captured, &mut outlets)?;
        }
        if let Some(stats) = pipeline.report() {
            (shared.emit)(Event::CameraStats(stats));
        }
    }
}
