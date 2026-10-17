//! Per-frame routing, independent of where frames come from.
//!
//! A [`Source`] yields captured frames; the [`Pipeline`] grades each one once
//! and hands the result to whichever [`Outlets`] currently want it. Buffers
//! are swapped, not copied, between the grader and the latest-wins slots, so
//! the only full-frame copies are the GPU readback and the final copy into a
//! consumer's buffer.
use super::thumb::{Sampler, Thumb};
use crate::{
    Stats,
    frames::{Frame, PixelFormat, Slot},
    grader::{Grader, Input, InputFormat, MAX_SWATCHES, Outputs, Wants},
};
use anyhow::Result;
use omalive_core::{Lattice, color::YuvMatrix};
use std::{
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceFormat {
    pub width: u32,
    pub height: u32,
    /// Bytes per row of the first plane.
    pub stride: u32,
    pub fps: u32,
    pub format: InputFormat,
    pub matrix: YuvMatrix,
}

impl SourceFormat {
    pub fn frame_size(&self) -> usize {
        match self.format {
            InputFormat::Nv12 => self.stride as usize * self.height as usize * 3 / 2,
            InputFormat::Yuyv => self.stride as usize * self.height as usize,
        }
    }
}

pub struct Captured<'a> {
    pub data: &'a [u8],
    /// Capture time on CLOCK_MONOTONIC in nanoseconds, 0 when unknown.
    pub timestamp: u64,
}

/// Something that produces frames: a V4L2 camera or a synthetic pattern.
pub trait Source {
    fn format(&self) -> SourceFormat;
    /// Pause or resume capture (the camera light follows this).
    fn set_streaming(&mut self, streaming: bool) -> Result<()>;
    /// The next frame, or `None` after a short timeout without one.
    fn next(&mut self) -> Result<Option<Captured<'_>>>;
}

/// Where graded frames go.
pub struct Outlets {
    /// The PipeWire camera; its reader sets the wanted format.
    pub video: Arc<Slot>,
    pub video_ready: Box<dyn Fn() + Send>,
    /// A v4l2loopback device, fed YUYV.
    pub loopback: Option<Box<dyn Write + Send>>,
    pub preview: Arc<Slot>,
    pub preview_ready: Box<dyn Fn() + Send>,
    /// The bar's picture, rendered with frames graded for something else.
    pub thumb: Arc<Thumb>,
}

pub struct Pipeline {
    pub grader: Grader,
    format: SourceFormat,
    outputs: Outputs,
    video_frame: Frame,
    preview_frame: Frame,
    stats: StatsWindow,
    thumbs: Sampler,
    /// Preview size while the preview is on screen.
    pub preview: Option<(u32, u32)>,
    swatches: u32,
}

pub enum Routed {
    Graded,
    /// No outlet wanted this frame; nothing was computed.
    Idle,
    /// Short or corrupt buffer.
    Dropped,
}

impl Pipeline {
    pub fn new(grader: Grader, format: SourceFormat) -> Self {
        Self {
            grader,
            format,
            outputs: Outputs::default(),
            video_frame: Frame::default(),
            preview_frame: Frame::default(),
            stats: StatsWindow::new(),
            thumbs: Sampler::default(),
            preview: None,
            swatches: 0,
        }
    }

    /// The looks to render as swatches with each preview frame.
    pub fn set_swatches(&mut self, lattices: &[Lattice]) {
        self.grader.set_swatches(lattices);
        self.swatches = lattices.len().min(MAX_SWATCHES) as u32;
    }

    /// What the current consumers need from the next frame.
    pub fn wants(&self, outlets: &Outlets) -> Wants {
        self.wants_for(outlets.video.wanted(), outlets)
    }

    fn wants_for(&self, video: PixelFormat, outlets: &Outlets) -> Wants {
        Wants {
            nv12: video == PixelFormat::Nv12,
            yuyv: video == PixelFormat::Yuyv || outlets.loopback.is_some(),
            // Only render a preview once the UI drew the last one: a hidden
            // window takes no frames, so it costs nothing.
            preview: self.preview.filter(|_| outlets.preview.is_taken()),
            swatches: self.swatches,
            bar: false,
        }
    }

    /// Whether anyone could use frames right now (for on-demand capture).
    pub fn demanded(&self, outlets: &Outlets) -> bool {
        outlets.video.wanted() != PixelFormat::None
            || outlets.loopback.is_some()
            || self.preview.is_some()
    }

    pub fn frame(&mut self, captured: Captured, outlets: &mut Outlets) -> Result<Routed> {
        let format = self.format;
        if captured.data.len() < format.frame_size() {
            self.stats.dropped += 1;
            return Ok(Routed::Dropped);
        }
        // Read the negotiated format once so routing matches what was graded.
        let video = outlets.video.wanted();
        let wants = self.wants_for(video, outlets);
        if !wants.any() {
            self.stats.idle += 1;
            return Ok(Routed::Idle);
        }
        let wants = Wants {
            bar: self.thumbs.due(&outlets.thumb),
            ..wants
        };
        let started = Instant::now();
        let input = Input {
            data: captured.data,
            width: format.width,
            height: format.height,
            stride: format.stride,
            format: format.format,
        };
        self.grader
            .process(&input, format.matrix, wants, &mut self.outputs)?;
        let elapsed = started.elapsed();
        if wants.bar {
            self.thumbs
                .publish(&outlets.thumb, std::mem::take(&mut self.outputs.bar));
        }

        if video != PixelFormat::None {
            let graded = match video {
                PixelFormat::Yuyv if outlets.loopback.is_some() => {
                    // The loopback also needs these bytes; copy rather than move.
                    self.video_frame.data.clear();
                    self.video_frame.data.extend_from_slice(&self.outputs.yuyv);
                    None
                }
                PixelFormat::Yuyv => Some(&mut self.outputs.yuyv),
                _ => Some(&mut self.outputs.nv12),
            };
            if let Some(graded) = graded {
                std::mem::swap(&mut self.video_frame.data, graded);
            }
            self.video_frame.width = format.width;
            self.video_frame.height = format.height;
            self.video_frame.captured = captured.timestamp;
            outlets.video.publish(&mut self.video_frame);
            (outlets.video_ready)();
        }
        if let Some(loopback) = &mut outlets.loopback
            && loopback.write_all(&self.outputs.yuyv).is_err()
        {
            outlets.loopback = None;
        }
        if wants.preview.is_some() {
            std::mem::swap(&mut self.preview_frame.data, &mut self.outputs.preview);
            std::mem::swap(&mut self.preview_frame.extra, &mut self.outputs.original);
            std::mem::swap(&mut self.preview_frame.swatches, &mut self.outputs.swatches);
            (self.preview_frame.width, self.preview_frame.height) = self.outputs.preview_size;
            self.preview_frame.captured = captured.timestamp;
            outlets.preview.publish(&mut self.preview_frame);
            (outlets.preview_ready)();
        }
        self.stats.frame(elapsed, captured.timestamp);
        Ok(Routed::Graded)
    }

    pub fn report(&mut self) -> Option<Stats> {
        self.stats.report()
    }
}

pub fn monotonic_ns() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64
}

/// Rolling one-second statistics.
struct StatsWindow {
    since: Instant,
    frames: u32,
    process: Duration,
    worst: Duration,
    latency: Vec<u64>,
    dropped: u32,
    idle: u32,
}

impl StatsWindow {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
            process: Duration::ZERO,
            worst: Duration::ZERO,
            latency: Vec::with_capacity(64),
            dropped: 0,
            idle: 0,
        }
    }

    fn frame(&mut self, process: Duration, captured: u64) {
        self.frames += 1;
        self.process += process;
        self.worst = self.worst.max(process);
        // Only meaningful with monotonic driver timestamps; discard anything
        // implausible rather than report nonsense.
        if captured > 0
            && let Some(age) = monotonic_ns()
                .checked_sub(captured)
                .filter(|age| *age < 2_000_000_000)
        {
            self.latency.push(age);
        }
    }

    fn report(&mut self) -> Option<Stats> {
        let elapsed = self.since.elapsed();
        if elapsed < Duration::from_secs(1) {
            return None;
        }
        self.latency.sort_unstable();
        let percentile = |p: f32| {
            self.latency
                .get(((self.latency.len() as f32 - 1.) * p).round() as usize)
                .map(|ns| *ns as f32 / 1e6)
        };
        let stats = Stats {
            fps: self.frames as f32 / elapsed.as_secs_f32(),
            process_ms: if self.frames > 0 {
                self.process.as_secs_f32() * 1000. / self.frames as f32
            } else {
                0.
            },
            worst_ms: self.worst.as_secs_f32() * 1000.,
            latency_ms: percentile(0.5),
            latency_p95_ms: percentile(0.95),
            dropped: self.dropped,
            idle: self.idle,
        };
        *self = Self::new();
        Some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{camera::synthetic::Synthetic, grader::CpuGrader};
    use std::sync::{
        Mutex,
        atomic::{AtomicU32, Ordering},
    };

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let mut data = self.0.lock().unwrap();
            data.clear();
            data.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn setup(loopback: Option<Sink>) -> (Pipeline, Outlets, Synthetic, Arc<AtomicU32>) {
        let source = Synthetic::new(64, 32, 30);
        let pipeline = Pipeline::new(
            Grader::Cpu(CpuGrader::with_lattice(Lattice::identity(9))),
            source.format(),
        );
        let ready = Arc::new(AtomicU32::new(0));
        let counter = ready.clone();
        let outlets = Outlets {
            video: Arc::new(Slot::default()),
            video_ready: Box::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            }),
            loopback: loopback.map(|s| Box::new(s) as Box<dyn Write + Send>),
            preview: Arc::new(Slot::default()),
            preview_ready: Box::new(|| {}),
            thumb: Arc::default(),
        };
        (pipeline, outlets, source, ready)
    }

    fn step(pipeline: &mut Pipeline, outlets: &mut Outlets, source: &mut Synthetic) -> Routed {
        let captured = source.next().unwrap().unwrap();
        pipeline.frame(captured, outlets).unwrap()
    }

    #[test]
    fn no_consumers_means_no_work() {
        let (mut pipeline, mut outlets, mut source, ready) = setup(None);
        assert!(!pipeline.demanded(&outlets));
        assert!(matches!(
            step(&mut pipeline, &mut outlets, &mut source),
            Routed::Idle
        ));
        assert_eq!(ready.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn pipewire_gets_its_negotiated_format() {
        let (mut pipeline, mut outlets, mut source, ready) = setup(None);
        outlets.video.want(PixelFormat::Nv12);
        assert!(pipeline.demanded(&outlets));
        assert!(matches!(
            step(&mut pipeline, &mut outlets, &mut source),
            Routed::Graded
        ));
        assert_eq!(ready.load(Ordering::Relaxed), 1);
        let size = outlets.video.read(0, |f| (f.data.len(), f.width)).unwrap();
        assert_eq!(size, (64 * 32 * 3 / 2, 64));
        outlets.video.want(PixelFormat::Yuyv);
        step(&mut pipeline, &mut outlets, &mut source);
        assert_eq!(outlets.video.read(1, |f| f.data.len()), Some(64 * 32 * 2));
    }

    #[test]
    fn loopback_and_pipewire_share_one_yuyv_grade() {
        let sink = Sink::default();
        let (mut pipeline, mut outlets, mut source, _) = setup(Some(sink.clone()));
        outlets.video.want(PixelFormat::Yuyv);
        step(&mut pipeline, &mut outlets, &mut source);
        let pipewire = outlets.video.read(0, |f| f.data.clone()).unwrap();
        assert_eq!(*sink.0.lock().unwrap(), pipewire);
        assert_eq!(pipewire.len(), 64 * 32 * 2);
    }

    #[test]
    fn preview_waits_for_the_ui_to_draw() {
        let (mut pipeline, mut outlets, mut source, _) = setup(None);
        pipeline.preview = Some((16, 8));
        assert!(matches!(
            step(&mut pipeline, &mut outlets, &mut source),
            Routed::Graded
        ));
        // Not drawn yet: the next frame skips the preview and does no work.
        assert!(matches!(
            step(&mut pipeline, &mut outlets, &mut source),
            Routed::Idle
        ));
        let frame = outlets.preview.take(0).unwrap();
        assert_eq!(
            (frame.width, frame.height, frame.data.len()),
            (16, 8, 16 * 8 * 4)
        );
        assert!(matches!(
            step(&mut pipeline, &mut outlets, &mut source),
            Routed::Graded
        ));
    }

    #[test]
    fn the_bar_thumbnail_rides_on_graded_frames_only() {
        let (mut pipeline, mut outlets, mut source, _) = setup(None);
        outlets.thumb.want(true);
        // Wanting a thumbnail alone never wakes the camera or the grader.
        assert!(!pipeline.demanded(&outlets));
        step(&mut pipeline, &mut outlets, &mut source);
        assert!(outlets.thumb.latest(0).is_none());
        outlets.video.want(PixelFormat::Nv12);
        step(&mut pipeline, &mut outlets, &mut source);
        assert_eq!(outlets.thumb.latest(0).map(|(n, _)| n), Some(1));
    }

    #[test]
    fn short_buffers_are_dropped() {
        let (mut pipeline, mut outlets, _, _) = setup(None);
        outlets.video.want(PixelFormat::Nv12);
        let routed = pipeline
            .frame(
                Captured {
                    data: &[0; 10],
                    timestamp: 0,
                },
                &mut outlets,
            )
            .unwrap();
        assert!(matches!(routed, Routed::Dropped));
    }
}
