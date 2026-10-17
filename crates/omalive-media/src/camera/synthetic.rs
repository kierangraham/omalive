//! A moving NV12 test pattern: colour bars, a grey ramp and a sweeping
//! marker. Used by tests and by demo mode, which needs no camera. Demo mode
//! can show a slowly panning picture instead, which shows off a look far
//! better than colour bars.
use super::pipeline::{Captured, Source, SourceFormat, monotonic_ns};
use crate::grader::InputFormat;
use anyhow::Result;
use omalive_core::color::YuvMatrix;
use std::time::{Duration, Instant};

/// An sRGB picture for the demo camera, larger than the frame so it can pan.
#[derive(Debug)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// R G B, row by row.
    pub rgb: Vec<u8>,
}

pub struct Synthetic {
    format: SourceFormat,
    /// The static pattern, encoded once; frames copy it and add the marker.
    base: Vec<u8>,
    /// A picture to pan across instead, encoded once as NV12.
    picture: Option<(u32, u32, Vec<u8>)>,
    frame: Vec<u8>,
    index: u64,
    /// Pace frames at the nominal rate (demo); tests run unpaced.
    paced: Option<Instant>,
    streaming: bool,
}

const BARS: [[f32; 3]; 7] = [
    [0.75, 0.75, 0.75],
    [0.75, 0.75, 0.],
    [0., 0.75, 0.75],
    [0., 0.75, 0.],
    [0.75, 0., 0.75],
    [0.75, 0., 0.],
    [0., 0., 0.75],
];

impl Synthetic {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            format: SourceFormat {
                width,
                height,
                stride: width,
                fps,
                format: InputFormat::Nv12,
                matrix: YuvMatrix::BT601,
            },
            base: Vec::new(),
            picture: None,
            frame: Vec::new(),
            index: 0,
            paced: None,
            streaming: true,
        }
    }

    pub fn paced(mut self) -> Self {
        self.paced = Some(Instant::now());
        self
    }

    /// Pan slowly across `picture` (at least the frame size) instead of
    /// showing colour bars.
    pub fn with_picture(mut self, picture: &Picture) -> Self {
        let (w, h) = (picture.width & !1, picture.height & !1);
        if w >= self.format.width && h >= self.format.height {
            let rgb = |x: usize, y: usize| {
                let i = (y * picture.width as usize + x) * 3;
                [0, 1, 2].map(|c| picture.rgb[i + c] as f32 / 255.)
            };
            self.picture = Some((
                w,
                h,
                encode_with(w as usize, h as usize, self.format.matrix, rgb),
            ));
        }
        self
    }

    fn render(&mut self) {
        let (w, h) = (self.format.width as usize, self.format.height as usize);
        if let Some((pw, ph, nv12)) = &self.picture {
            let (pw, ph) = (*pw as usize, *ph as usize);
            // A slow drift, like a camera on a gentle slider.
            let t = self.index as f32 / self.format.fps as f32;
            let x = ((pw - w) as f32 * (0.5 + 0.5 * (t * 0.11).sin())) as usize & !1;
            let y = ((ph - h) as f32 * (0.5 + 0.5 * (t * 0.07).cos())) as usize & !1;
            self.frame.clear();
            for row in y..y + h {
                self.frame
                    .extend_from_slice(&nv12[row * pw + x..row * pw + x + w]);
            }
            for row in y / 2..y / 2 + h / 2 {
                let start = pw * ph + row * pw + x;
                self.frame.extend_from_slice(&nv12[start..start + w]);
            }
            return;
        }
        if self.base.is_empty() {
            self.base = encode(w, h, self.format.matrix);
        }
        self.frame.clear();
        self.frame.extend_from_slice(&self.base);
        // A white marker sweeps the grey ramp so motion is visible.
        let marker = (self.index as usize * 4) % w.max(1);
        let white = (YuvMatrix::BT601.from_rgb([1., 1., 1.])[0] * 255. + 0.5) as u8;
        for y in h * 2 / 3..h {
            for x in marker.saturating_sub(2)..(marker + 3).min(w) {
                self.frame[y * w + x] = white;
            }
        }
    }
}

/// Colour bars over a grey ramp, encoded as NV12.
fn encode(w: usize, h: usize, matrix: YuvMatrix) -> Vec<u8> {
    encode_with(w, h, matrix, |x, y| {
        if y < h * 2 / 3 {
            BARS[x * BARS.len() / w]
        } else {
            let v = x as f32 / (w - 1).max(1) as f32;
            [v, v, v]
        }
    })
}

/// Encode an image given pixel by pixel as NV12.
fn encode_with(
    w: usize,
    h: usize,
    matrix: YuvMatrix,
    colour: impl Fn(usize, usize) -> [f32; 3],
) -> Vec<u8> {
    let byte = |v: f32| (v * 255. + 0.5).clamp(0., 255.) as u8;
    let mut frame = vec![0; w * h * 3 / 2];
    for y in 0..h {
        for x in 0..w {
            frame[y * w + x] = byte(matrix.from_rgb(colour(x, y))[0]);
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let ycc = matrix.from_rgb(colour(x * 2, y * 2));
            let base = w * h + y * w + x * 2;
            frame[base] = byte(ycc[1]);
            frame[base + 1] = byte(ycc[2]);
        }
    }
    frame
}

impl Source for Synthetic {
    fn format(&self) -> SourceFormat {
        self.format
    }

    fn set_streaming(&mut self, streaming: bool) -> Result<()> {
        self.streaming = streaming;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Captured<'_>>> {
        if !self.streaming {
            std::thread::sleep(Duration::from_millis(50));
            return Ok(None);
        }
        if let Some(start) = self.paced {
            let due = start + Duration::from_secs_f64(self.index as f64 / self.format.fps as f64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        self.render();
        self.index += 1;
        Ok(Some(Captured {
            data: &self.frame,
            timestamp: monotonic_ns(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_complete_and_move() {
        let mut source = Synthetic::new(32, 16, 30);
        let size = source.format().frame_size();
        let first = source.next().unwrap().unwrap().data.to_vec();
        assert_eq!(first.len(), size);
        let second = source.next().unwrap().unwrap().data.to_vec();
        assert_ne!(first, second, "marker sweeps");
        source.set_streaming(false).unwrap();
        assert!(source.next().unwrap().is_none());
    }

    #[test]
    fn a_picture_pans_and_keeps_its_colour() {
        let picture = Picture {
            width: 48,
            height: 24,
            rgb: [204, 102, 51].repeat(48 * 24),
        };
        let mut source = Synthetic::new(32, 16, 30).with_picture(&picture);
        let size = source.format().frame_size();
        let frame = source.next().unwrap().unwrap().data.to_vec();
        assert_eq!(frame.len(), size);
        let [y, cb, cr] = YuvMatrix::BT601
            .from_rgb([0.8, 0.4, 0.2])
            .map(|v| (v * 255. + 0.5) as u8);
        assert!(frame[..32 * 16].iter().all(|&v| v == y));
        assert_eq!(&frame[32 * 16..32 * 16 + 2], &[cb, cr]);
        // Too small to pan: falls back to the pattern rather than failing.
        let tiny = Picture {
            width: 8,
            height: 8,
            rgb: vec![0; 8 * 8 * 3],
        };
        let mut source = Synthetic::new(32, 16, 30).with_picture(&tiny);
        assert_eq!(source.next().unwrap().unwrap().data.len(), size);
    }
}
