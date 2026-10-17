//! V4L2 cameras and v4l2loopback output.
//!
//! Raw NV12 and YUYV go to the grader untouched. Cameras and capture cards
//! that only reach full rate in MJPEG are decoded on the camera thread into
//! YUYV, so the grader sees the same thing a raw camera sends.
use super::pipeline::{Captured, Source, SourceFormat};
use crate::grader::InputFormat;
use anyhow::{Context, Result, anyhow, bail};
use omalive_core::color::YuvMatrix;
use std::{path::PathBuf, time::Duration};
use turbojpeg::{Decompressor, Subsamp, YuvImage};
use v4l::{
    Device, Format, FourCC,
    buffer::Type,
    format::{Colorspace, Quantization},
    fraction::Fraction,
    frameinterval::FrameIntervalEnum,
    io::{mmap::Stream, traits::CaptureStream},
    video::{Capture, Output, capture::Parameters},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDevice {
    /// Stable identity (V4L2 bus info); survives /dev/video renumbering.
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    /// Landscape sizes the grader supports, largest first.
    pub sizes: Vec<(u32, u32)>,
}

const LOOPBACK_DRIVER: &str = "v4l2 loopback";
/// Formats the camera thread can read, cheapest first.
const FORMATS: [(&[u8; 4], Encoding); 3] = [
    (b"NV12", Encoding::Raw(InputFormat::Nv12)),
    (b"YUYV", Encoding::Raw(InputFormat::Yuyv)),
    (b"MJPG", Encoding::Mjpeg),
];
/// Calls run at 30 fps; capturing faster only costs CPU.
const TARGET_FPS: f32 = 30.;
/// Two buffers: one filling while one is graded. More only adds latency.
const BUFFERS: u32 = 2;

/// Capture devices with a raw format the grader reads. Querying a device
/// does not start streaming, so the camera light stays off.
pub fn devices() -> Vec<CameraDevice> {
    let mut devices: Vec<CameraDevice> = v4l::context::enum_devices()
        .into_iter()
        .filter_map(|node| {
            let device = Device::with_path(node.path()).ok()?;
            let caps = device.query_caps().ok()?;
            if caps.driver == LOOPBACK_DRIVER
                || !caps
                    .capabilities
                    .contains(v4l::capability::Flags::VIDEO_CAPTURE)
            {
                return None;
            }
            let formats = Capture::enum_formats(&device).ok()?;
            let mut sizes = Vec::new();
            for (fourcc, _) in FORMATS {
                if !formats.iter().any(|f| f.fourcc.repr == *fourcc) {
                    continue;
                }
                for size in
                    Capture::enum_framesizes(&device, FourCC::new(fourcc)).unwrap_or_default()
                {
                    for discrete in size.size.to_discrete() {
                        let (w, h) = (discrete.width, discrete.height);
                        if w >= h && w % 4 == 0 && h % 2 == 0 && w <= 3840 {
                            sizes.push((w, h));
                        }
                    }
                }
            }
            sizes.sort_by_key(|(w, h)| std::cmp::Reverse(w * h));
            sizes.dedup();
            (!sizes.is_empty()).then(|| CameraDevice {
                id: caps.bus.clone(),
                path: node.path().to_path_buf(),
                name: caps.card.clone(),
                sizes,
            })
        })
        .collect();
    devices.sort_by(|a, b| a.path.cmp(&b.path));
    // One physical camera often exposes several nodes; keep the first.
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

/// The first v4l2loopback device, if the module is loaded.
pub fn loopback() -> Option<(PathBuf, String)> {
    v4l::context::enum_devices().into_iter().find_map(|node| {
        let device = Device::with_path(node.path()).ok()?;
        let caps = device.query_caps().ok()?;
        (caps.driver == LOOPBACK_DRIVER).then(|| (node.path().to_path_buf(), caps.card))
    })
}

/// Configure a v4l2loopback device for YUYV output.
pub fn open_loopback(path: &std::path::Path, width: u32, height: u32, fps: u32) -> Result<Device> {
    let device = Device::with_path(path)?;
    let format = Output::set_format(&device, &Format::new(width, height, FourCC::new(b"YUYV")))?;
    if format.width != width || format.height != height || format.fourcc.repr != *b"YUYV" {
        bail!("it is already in use at another size");
    }
    let _ = Output::set_params(&device, &v4l::video::output::Parameters::with_fps(fps));
    Ok(device)
}

/// Decode matrix from the driver's reported colorimetry.
pub fn matrix(colorspace: Colorspace, quantization: Quantization) -> YuvMatrix {
    let base = match colorspace {
        Colorspace::Rec709 => YuvMatrix::BT709,
        _ => YuvMatrix::BT601,
    };
    base.full(
        matches!(quantization, Quantization::FullRange) || matches!(colorspace, Colorspace::JPEG),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Raw(InputFormat),
    Mjpeg,
}

/// One way to capture a size: a format at a frame interval.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mode {
    fourcc: [u8; 4],
    encoding: Encoding,
    /// Seconds per frame, as the driver states it.
    interval: (u32, u32),
}

impl Mode {
    fn fps(&self) -> f32 {
        self.interval.1 as f32 / self.interval.0.max(1) as f32
    }
}

/// The rate nearest 30 fps without going over wins (a camera that only
/// runs faster is still taken, at its slowest), then raw over MJPEG, which
/// costs a decode per frame.
fn best_mode(modes: &[Mode]) -> Option<Mode> {
    let rank = |mode: &Mode| {
        let fps = mode.fps();
        (
            fps.min(TARGET_FPS),
            -fps,
            matches!(mode.encoding, Encoding::Raw(_)),
        )
    };
    modes.iter().copied().max_by(|a, b| {
        rank(a)
            .partial_cmp(&rank(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Every format and rate the camera offers at one size.
fn modes(device: &Device, (width, height): (u32, u32)) -> Vec<Mode> {
    let available = Capture::enum_formats(device).unwrap_or_default();
    let mut modes = Vec::new();
    for (fourcc, encoding) in FORMATS {
        if !available.iter().any(|f| f.fourcc.repr == *fourcc) {
            continue;
        }
        let intervals = Capture::enum_frameintervals(device, FourCC::new(fourcc), width, height)
            .unwrap_or_default();
        for interval in intervals {
            let fractions = match interval.interval {
                FrameIntervalEnum::Discrete(f) => vec![f],
                // Continuous ranges: the fastest, the slowest, and 30 fps
                // when it lies between.
                FrameIntervalEnum::Stepwise(range) => {
                    let mut fractions = vec![range.min, range.max];
                    let target = Fraction::new(1, TARGET_FPS as u32);
                    let seconds = |f: Fraction| f.numerator as f32 / f.denominator.max(1) as f32;
                    if (seconds(range.min)..=seconds(range.max)).contains(&seconds(target)) {
                        fractions.push(target);
                    }
                    fractions
                }
            };
            modes.extend(fractions.into_iter().map(|f| Mode {
                fourcc: *fourcc,
                encoding,
                interval: (f.numerator, f.denominator),
            }));
        }
    }
    modes
}

/// Say plainly when another app has the camera; keep other errors as they are.
fn explain(error: std::io::Error, camera: &str) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::ResourceBusy {
        anyhow!("{camera} is in use by another app")
    } else {
        error.into()
    }
}

pub struct V4l2Source {
    name: String,
    device: Device,
    stream: Option<Stream<'static>>,
    format: SourceFormat,
    /// Present when the camera sends MJPEG.
    decoder: Option<Mjpeg>,
}

impl V4l2Source {
    pub fn open(camera: &CameraDevice, size: (u32, u32)) -> Result<Self> {
        let device = Device::with_path(&camera.path)
            .with_context(|| format!("Opening {}", camera.path.display()))?;
        let modes = modes(&device, size);
        let mode = best_mode(&modes).with_context(|| {
            format!(
                "{} offers no NV12, YUYV or MJPEG at {}×{}",
                camera.name, size.0, size.1
            )
        })?;
        let name = String::from_utf8_lossy(&mode.fourcc).into_owned();
        let format = Capture::set_format(
            &device,
            &Format::new(size.0, size.1, FourCC::new(&mode.fourcc)),
        )
        .map_err(|error| explain(error, &camera.name))
        .with_context(|| format!("Asking {} for {}×{} {name}", camera.name, size.0, size.1))?;
        if format.fourcc.repr != mode.fourcc || (format.width, format.height) != size {
            bail!("{} refused {}×{} {name}", camera.name, size.0, size.1);
        }
        // Drivers keep the previous rate otherwise. Failing to set it only
        // costs efficiency, so carry on with whatever the driver picked.
        let _ = Capture::set_params(
            &device,
            &Parameters::new(Fraction::new(mode.interval.0, mode.interval.1)),
        );
        let fps = Capture::params(&device)
            .ok()
            .map(|p| p.interval)
            .filter(|i| i.numerator > 0)
            .map(|i| (i.denominator / i.numerator).clamp(1, 120))
            .unwrap_or(30);
        let (input, stride, matrix, decoder) = match mode.encoding {
            Encoding::Raw(input) => {
                let stride = match (format.stride, input) {
                    (0, InputFormat::Nv12) => format.width,
                    (0, InputFormat::Yuyv) => format.width * 2,
                    (stride, _) => stride,
                };
                (
                    input,
                    stride,
                    matrix(format.colorspace, format.quantization),
                    None,
                )
            }
            // JPEG is full-range BT.601 whatever the driver reports.
            Encoding::Mjpeg => (
                InputFormat::Yuyv,
                format.width * 2,
                YuvMatrix::BT601.full(true),
                Some(Mjpeg::new(format.width, format.height)?),
            ),
        };
        Ok(Self {
            format: SourceFormat {
                width: format.width,
                height: format.height,
                stride,
                fps,
                format: input,
                matrix,
            },
            name: camera.name.clone(),
            device,
            stream: None,
            decoder,
        })
    }
}

/// Decodes MJPEG frames into a reused YUYV buffer. libjpeg-turbo decodes
/// straight to YUV planes (no colour conversion or chroma upsampling) and
/// fills in the standard Huffman tables UVC cameras leave out.
struct Mjpeg {
    decompressor: Decompressor,
    width: usize,
    height: usize,
    /// Decoder output: Y, Cb and Cr planes, rows unpadded.
    planes: Vec<u8>,
    yuyv: Vec<u8>,
}

impl Mjpeg {
    fn new(width: u32, height: u32) -> Result<Self> {
        let (width, height) = (width as usize, height as usize);
        Ok(Self {
            decompressor: Decompressor::new().context("Starting the JPEG decoder")?,
            width,
            height,
            planes: Vec::new(),
            yuyv: vec![0; width * height * 2],
        })
    }

    /// Decode one frame; `None` for a corrupt or wrongly sized one.
    fn decode(&mut self, jpeg: &[u8]) -> Option<&[u8]> {
        let header = self.decompressor.read_header(jpeg).ok()?;
        if (header.width, header.height) != (self.width, self.height) {
            return None;
        }
        let length = turbojpeg::yuv_pixels_len(self.width, 1, self.height, header.subsamp).ok()?;
        self.planes.resize(length, 0);
        let image = YuvImage {
            pixels: self.planes.as_mut_slice(),
            width: self.width,
            align: 1,
            height: self.height,
            subsamp: header.subsamp,
        };
        self.decompressor.decompress_to_yuv(jpeg, image).ok()?;
        pack_yuyv(
            &self.planes,
            self.width,
            self.height,
            header.subsamp,
            &mut self.yuyv,
        )?;
        Some(&self.yuyv)
    }
}

/// Interleave Y, Cb and Cr planes at any JPEG subsampling into YUYV.
/// Each pixel pair takes the chroma sample at its left pixel.
fn pack_yuyv(
    planes: &[u8],
    width: usize,
    height: usize,
    subsamp: Subsamp,
    yuyv: &mut [u8],
) -> Option<()> {
    let (luma, chroma) = planes.split_at_checked(width * height)?;
    if subsamp == Subsamp::Gray {
        for (pair, out) in luma.chunks_exact(2).zip(yuyv.chunks_exact_mut(4)) {
            out.copy_from_slice(&[pair[0], 128, pair[1], 128]);
        }
        return Some(());
    }
    let (sx, sy) = subsamp.size();
    let chroma_width = width.div_ceil(sx);
    let plane = chroma_width * height.div_ceil(sy);
    let (cb, cr) = chroma.get(..plane * 2)?.split_at(plane);
    for (row, out) in yuyv.chunks_exact_mut(width * 2).take(height).enumerate() {
        let luma = &luma[row * width..][..width];
        let at = row / sy * chroma_width;
        let (cb, cr) = (&cb[at..][..chroma_width], &cr[at..][..chroma_width]);
        if sx == 2 {
            // 4:2:2 and 4:2:0, nearly every camera: straight interleave.
            for (((pair, out), u), v) in luma
                .chunks_exact(2)
                .zip(out.chunks_exact_mut(4))
                .zip(cb)
                .zip(cr)
            {
                out.copy_from_slice(&[pair[0], *u, pair[1], *v]);
            }
        } else {
            for (x, out) in out.chunks_exact_mut(4).enumerate() {
                let c = x * 2 / sx;
                out.copy_from_slice(&[luma[x * 2], cb[c], luma[x * 2 + 1], cr[c]]);
            }
        }
    }
    Some(())
}

impl Source for V4l2Source {
    fn format(&self) -> SourceFormat {
        self.format
    }

    fn set_streaming(&mut self, streaming: bool) -> Result<()> {
        if streaming && self.stream.is_none() {
            let mut stream = Stream::with_buffers(&self.device, Type::VideoCapture, BUFFERS)
                .map_err(|error| explain(error, &self.name))
                .with_context(|| format!("Starting {}", self.name))?;
            stream.set_timeout(Duration::from_millis(250));
            self.stream = Some(stream);
        } else if !streaming {
            // Dropping the stream issues STREAMOFF and frees buffers.
            self.stream = None;
        }
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Captured<'_>>> {
        let Some(stream) = &mut self.stream else {
            std::thread::sleep(Duration::from_millis(50));
            return Ok(None);
        };
        match stream.next() {
            Ok((data, meta)) => {
                let used = (meta.bytesused as usize).min(data.len());
                let timestamp = (meta.timestamp.sec as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(meta.timestamp.usec as u64 * 1000);
                let data = match &mut self.decoder {
                    // A frame that fails to decode reaches the pipeline
                    // empty and is counted as dropped.
                    Some(decoder) => decoder.decode(&data[..used]).unwrap_or_default(),
                    None => &data[..used],
                };
                Ok(Some(Captured { data, timestamp }))
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error).context("Reading the camera"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(fourcc: &[u8; 4], fps: u32) -> Mode {
        let encoding = FORMATS.iter().find(|(f, _)| *f == fourcc).unwrap().1;
        Mode {
            fourcc: *fourcc,
            encoding,
            interval: (1, fps),
        }
    }

    #[test]
    fn a_busy_camera_is_named() {
        let busy = explain(std::io::ErrorKind::ResourceBusy.into(), "Capture card");
        assert_eq!(busy.to_string(), "Capture card is in use by another app");
        let other = explain(std::io::Error::from_raw_os_error(22), "Capture card");
        assert!(other.to_string().contains("os error 22"));
    }

    #[test]
    fn modes_prefer_full_rate_then_raw() {
        // A capture card: raw YUYV crawls at 1080p, MJPEG runs at 60 or 30.
        let card = [
            mode(b"MJPG", 60),
            mode(b"MJPG", 30),
            mode(b"MJPG", 10),
            mode(b"YUYV", 5),
        ];
        assert_eq!(best_mode(&card), Some(mode(b"MJPG", 30)));
        // At small sizes raw keeps up, so no decode is needed.
        let small = [mode(b"MJPG", 30), mode(b"YUYV", 30), mode(b"YUYV", 15)];
        assert_eq!(best_mode(&small), Some(mode(b"YUYV", 30)));
        // Only faster than wanted: take the slowest.
        assert_eq!(
            best_mode(&[mode(b"NV12", 120), mode(b"NV12", 60)]),
            Some(mode(b"NV12", 60))
        );
        assert_eq!(best_mode(&[]), None);
    }

    #[test]
    fn planes_interleave_at_every_subsampling() {
        // 4×2 image: luma 0..8, chroma numbered per plane position.
        let luma: Vec<u8> = (0..8).collect();
        let pack = |subsamp, chroma: &[u8]| {
            let planes = [luma.as_slice(), chroma].concat();
            let mut yuyv = [0; 16];
            pack_yuyv(&planes, 4, 2, subsamp, &mut yuyv).map(|()| yuyv)
        };
        // 4:2:2: two chroma samples per row.
        assert_eq!(
            pack(Subsamp::Sub2x1, &[10, 11, 12, 13, 20, 21, 22, 23]),
            Some([0, 10, 1, 20, 2, 11, 3, 21, 4, 12, 5, 22, 6, 13, 7, 23])
        );
        // 4:2:0: both rows share one chroma row.
        assert_eq!(
            pack(Subsamp::Sub2x2, &[10, 11, 20, 21]),
            Some([0, 10, 1, 20, 2, 11, 3, 21, 4, 10, 5, 20, 6, 11, 7, 21])
        );
        // 4:4:4: the left pixel of each pair.
        let full: Vec<u8> = (10..18).chain(20..28).collect();
        assert_eq!(
            pack(Subsamp::None, &full),
            Some([0, 10, 1, 20, 2, 12, 3, 22, 4, 14, 5, 24, 6, 16, 7, 26])
        );
        assert_eq!(
            pack(Subsamp::Gray, &[]),
            Some([
                0, 128, 1, 128, 2, 128, 3, 128, 4, 128, 5, 128, 6, 128, 7, 128
            ])
        );
        // Short planes are refused, never read past.
        assert_eq!(pack(Subsamp::Sub2x1, &[10, 11]), None);
    }

    #[test]
    fn mjpeg_without_huffman_tables_decodes_to_yuyv() {
        // 16×16 solid rgb(200, 100, 50), 4:2:2, DHT and APP segments
        // removed the way UVC cameras send it.
        let jpeg = include_bytes!("testdata/mjpeg-16x16.jpg");
        let mut decoder = Mjpeg::new(16, 16).unwrap();
        let yuyv = decoder.decode(jpeg).expect("decodes").to_vec();
        assert_eq!(yuyv.len(), 16 * 16 * 2);
        // Full-range BT.601 of rgb(200, 100, 50): Y 124, Cb 86, Cr 182.
        for pixel in yuyv.chunks_exact(4) {
            for (got, want) in pixel.iter().zip([124, 86, 124, 182]) {
                assert!(got.abs_diff(want) <= 3, "{pixel:?}");
            }
        }
        // A frame of another size, or garbage, is refused rather than misread.
        assert!(Mjpeg::new(32, 16).unwrap().decode(jpeg).is_none());
        assert!(decoder.decode(&jpeg[..40]).is_none());
    }
}
