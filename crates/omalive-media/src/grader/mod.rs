//! Frame grading front end: the GPU path, with a CPU fallback that applies
//! the same baked lattice. The CPU path is also the reference in tests.
mod gpu;

use anyhow::Result;
use gpu::GpuGrader;
use omalive_core::{Lattice, color::YuvMatrix};

/// Size of one look swatch, in pixels.
pub const SWATCH: (u32, u32) = (192, 108);
/// Lattice size for swatch looks: plenty for a thumbnail.
pub const SWATCH_LUT: usize = 17;
/// The bar panel's camera picture, in pixels: sharp across the panel on a
/// 1.6× display.
pub const BAR: (u32, u32) = (640, 360);
/// Most swatches rendered at once (the GPU stacks their lattices in one
/// 3D texture, at most 256 deep).
pub const MAX_SWATCHES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputFormat {
    Nv12,
    Yuyv,
}

pub struct Input<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Bytes per row of the first plane.
    pub stride: u32,
    pub format: InputFormat,
}

/// Which outputs this frame needs. Nothing is computed for absent consumers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wants {
    pub nv12: bool,
    pub yuyv: bool,
    pub preview: Option<(u32, u32)>,
    /// Look swatches to render beside the preview.
    pub swatches: u32,
    /// The bar's picture, from frames graded for something else anyway.
    pub bar: bool,
}

impl Wants {
    pub fn any(self) -> bool {
        self.nv12 || self.yuyv || self.preview.is_some()
    }
}

#[derive(Default)]
pub struct Outputs {
    pub nv12: Vec<u8>,
    pub yuyv: Vec<u8>,
    /// BGRA, `preview_size.0 * 4` bytes per row.
    pub preview: Vec<u8>,
    /// The ungraded preview, same layout, for before/after comparison.
    pub original: Vec<u8>,
    pub preview_size: (u32, u32),
    /// BGRA [`SWATCH`]-sized images, one after another.
    pub swatches: Vec<u8>,
    /// R G B A, [`BAR`]-sized; empty unless wanted.
    pub bar: Vec<u8>,
}

pub enum Grader {
    Gpu(Box<GpuGrader>),
    Cpu(CpuGrader),
}

impl Grader {
    pub fn new() -> (Self, Option<String>) {
        match GpuGrader::new() {
            Ok(gpu) => (Self::Gpu(Box::new(gpu)), None),
            Err(error) => (
                Self::Cpu(CpuGrader::default()),
                Some(format!("Grading on the CPU: {error:#}")),
            ),
        }
    }

    pub fn device(&self) -> String {
        match self {
            Self::Gpu(gpu) => gpu.adapter.clone(),
            Self::Cpu(_) => "CPU".into(),
        }
    }

    pub fn set_lattice(&mut self, lattice: &Lattice) {
        match self {
            Self::Gpu(gpu) => gpu.set_lattice(lattice),
            Self::Cpu(cpu) => cpu.lattice = lattice.clone(),
        }
    }

    /// Looks for the swatches, all [`SWATCH_LUT`] in size; at most
    /// [`MAX_SWATCHES`] are kept.
    pub fn set_swatches(&mut self, lattices: &[Lattice]) {
        let lattices = &lattices[..lattices.len().min(MAX_SWATCHES)];
        match self {
            Self::Gpu(gpu) => gpu.set_swatches(lattices),
            Self::Cpu(cpu) => cpu.swatches = lattices.to_vec(),
        }
    }

    pub fn process(
        &mut self,
        input: &Input,
        matrix: YuvMatrix,
        wants: Wants,
        out: &mut Outputs,
    ) -> Result<()> {
        match self {
            Self::Gpu(gpu) => gpu.process(input, matrix, wants, out),
            Self::Cpu(cpu) => {
                cpu.process(input, matrix, wants, out);
                Ok(())
            }
        }
    }
}

pub struct CpuGrader {
    lattice: Lattice,
    swatches: Vec<Lattice>,
}

impl Default for CpuGrader {
    fn default() -> Self {
        Self::with_lattice(Lattice::identity(2))
    }
}

impl CpuGrader {
    pub fn with_lattice(lattice: Lattice) -> Self {
        Self {
            lattice,
            swatches: Vec::new(),
        }
    }

    fn source(&self, input: &Input, matrix: YuvMatrix, x: u32, y: u32) -> [f32; 3] {
        self.lattice.sample(decode(input, matrix, x, y))
    }
}

/// The ungraded R'G'B' value of one source pixel.
fn decode(input: &Input, matrix: YuvMatrix, x: u32, y: u32) -> [f32; 3] {
    let n = |v: u8| v as f32 / 255.;
    let stride = input.stride as usize;
    let (yv, cb, cr) = match input.format {
        InputFormat::Nv12 => {
            let luma = input.data[y as usize * stride + x as usize];
            let chroma =
                input.height as usize * stride + (y as usize / 2) * stride + (x as usize & !1);
            (luma, input.data[chroma], input.data[chroma + 1])
        }
        InputFormat::Yuyv => {
            let base = y as usize * stride + (x as usize / 2) * 4;
            let luma = input.data[base + if x & 1 == 1 { 2 } else { 0 }];
            (luma, input.data[base + 1], input.data[base + 3])
        }
    };
    matrix.to_rgb(n(yv), n(cb), n(cr))
}

impl CpuGrader {
    pub fn process(&self, input: &Input, matrix: YuvMatrix, wants: Wants, out: &mut Outputs) {
        let (width, height) = (input.width, input.height);
        let byte = |v: f32| (v * 255. + 0.5).clamp(0., 255.) as u8;
        let encode = YuvMatrix::BT601;
        if wants.nv12 || wants.yuyv {
            let pixels = width as usize * height as usize;
            out.nv12
                .resize(if wants.nv12 { pixels * 3 / 2 } else { 0 }, 0);
            out.yuyv.resize(if wants.yuyv { pixels * 2 } else { 0 }, 0);
            // Bands of row pairs graded in parallel.
            let threads = std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .min(8);
            let pairs = (height / 2) as usize;
            let per = pairs.div_ceil(threads).max(1);
            let results: Vec<Vec<[f32; 3]>> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..pairs)
                    .step_by(per)
                    .map(|start| {
                        scope.spawn(move || {
                            let mut band = Vec::with_capacity(per * 2 * width as usize);
                            for pair in start..(start + per).min(pairs) {
                                for row in 0..2 {
                                    for x in 0..width {
                                        let rgb =
                                            self.source(input, matrix, x, pair as u32 * 2 + row);
                                        band.push(encode.from_rgb(rgb));
                                    }
                                }
                            }
                            band
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap_or_default())
                    .collect()
            });
            let ycc: Vec<[f32; 3]> = results.concat();
            let at = |x: u32, y: u32| ycc[y as usize * width as usize + x as usize];
            for y in 0..height {
                for x in 0..width {
                    let v = at(x, y);
                    if wants.nv12 {
                        out.nv12[(y * width + x) as usize] = byte(v[0]);
                    }
                    if wants.yuyv {
                        out.yuyv[(y * width + x) as usize * 2] = byte(v[0]);
                    }
                }
            }
            for y in (0..height).step_by(2) {
                for x in (0..width).step_by(2) {
                    if wants.nv12 {
                        let quad = [at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1)];
                        let cb = quad.iter().map(|v| v[1]).sum::<f32>() / 4.;
                        let cr = quad.iter().map(|v| v[2]).sum::<f32>() / 4.;
                        let base = (width * height + (y / 2) * width + x) as usize;
                        out.nv12[base] = byte(cb);
                        out.nv12[base + 1] = byte(cr);
                    }
                    if wants.yuyv {
                        for row in [y, y + 1] {
                            let (a, b) = (at(x, row), at(x + 1, row));
                            let base = (row * width + x) as usize * 2;
                            out.yuyv[base + 1] = byte((a[1] + b[1]) / 2.);
                            out.yuyv[base + 3] = byte((a[2] + b[2]) / 2.);
                        }
                    }
                }
            }
        }
        if let Some((pw, ph)) = wants.preview {
            out.preview.clear();
            out.original.clear();
            for py in 0..ph {
                for px in 0..pw {
                    let x = ((px as f32 + 0.5) / pw as f32 * width as f32) as u32;
                    let y = ((py as f32 + 0.5) / ph as f32 * height as f32) as u32;
                    let original = decode(input, matrix, x.min(width - 1), y.min(height - 1));
                    let rgb = self.lattice.sample(original);
                    out.preview
                        .extend_from_slice(&[byte(rgb[2]), byte(rgb[1]), byte(rgb[0]), 255]);
                    out.original.extend_from_slice(&[
                        byte(original[2]),
                        byte(original[1]),
                        byte(original[0]),
                        255,
                    ]);
                }
            }
            out.preview_size = (pw, ph);
        } else {
            out.preview_size = (0, 0);
        }
        out.bar.clear();
        if wants.bar {
            // A box average over each pixel's footprint, so a full frame
            // shrinks without shimmer.
            let (bw, bh) = BAR;
            for by in 0..bh {
                let rows = span(by, bh, height);
                for bx in 0..bw {
                    let columns = span(bx, bw, width);
                    let mut sum = [0.; 3];
                    for y in rows.clone() {
                        for x in columns.clone() {
                            let rgb = decode(input, matrix, x, y);
                            (0..3).for_each(|c| sum[c] += rgb[c]);
                        }
                    }
                    let count = (rows.len() * columns.len()) as f32;
                    let rgb = self.lattice.sample(sum.map(|v| v / count));
                    out.bar
                        .extend_from_slice(&[byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), 255]);
                }
            }
        }
        out.swatches.clear();
        let (sw, sh) = SWATCH;
        for lattice in self.swatches.iter().take(wants.swatches as usize) {
            for sy in 0..sh {
                for sx in 0..sw {
                    let x = ((sx as f32 + 0.5) / sw as f32 * width as f32) as u32;
                    let y = ((sy as f32 + 0.5) / sh as f32 * height as f32) as u32;
                    let original = decode(input, matrix, x.min(width - 1), y.min(height - 1));
                    let rgb = lattice.sample(original);
                    out.swatches.extend_from_slice(&[
                        byte(rgb[2]),
                        byte(rgb[1]),
                        byte(rgb[0]),
                        255,
                    ]);
                }
            }
        }
    }
}

/// Source pixels under output pixel `i` of `n` across `size`, at least one.
fn span(i: u32, n: u32, size: u32) -> std::ops::Range<u32> {
    let start = (i * size / n).min(size - 1);
    start..((i + 1) * size / n).clamp(start + 1, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use omalive_core::{Grade, bake};

    /// A synthetic NV12 frame: horizontal luma ramp, vertical chroma ramp.
    pub fn nv12_frame(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.push((48 + x * 160 / width) as u8 ^ (y as u8 & 1));
            }
        }
        for y in 0..height / 2 {
            for x in 0..width / 2 {
                // Gentle chroma keeps every sample inside the RGB gamut.
                data.push((112 + y * 32 / (height / 2)) as u8);
                data.push((144 - x * 32 / (width / 2)) as u8);
            }
        }
        data
    }

    #[test]
    fn identity_round_trips_nv12_within_rounding() {
        let (w, h) = (16, 8);
        let data = nv12_frame(w, h);
        let input = Input {
            data: &data,
            width: w,
            height: h,
            stride: w,
            format: InputFormat::Nv12,
        };
        let mut out = Outputs::default();
        CpuGrader::with_lattice(Lattice::identity(33)).process(
            &input,
            YuvMatrix::BT601,
            Wants {
                nv12: true,
                yuyv: true,
                preview: Some((4, 2)),
                swatches: 0,
                bar: false,
            },
            &mut out,
        );
        let worst = data[..(w * h) as usize]
            .iter()
            .zip(&out.nv12)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 2, "luma error {worst}");
        assert_eq!(out.yuyv.len(), (w * h * 2) as usize);
        assert_eq!(out.yuyv[0], out.nv12[0]);
        assert_eq!(out.preview.len(), 4 * 2 * 4);
        assert_eq!(
            out.original, out.preview,
            "identity grade leaves the original unchanged"
        );
        assert!(out.preview.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn grade_changes_output() {
        let (w, h) = (8, 4);
        let data = nv12_frame(w, h);
        let input = Input {
            data: &data,
            width: w,
            height: h,
            stride: w,
            format: InputFormat::Nv12,
        };
        let dark = Grade {
            exposure: -2.,
            ..Default::default()
        };
        let mut out = Outputs::default();
        CpuGrader::with_lattice(bake(&dark, None, 0., 17)).process(
            &input,
            YuvMatrix::BT601,
            Wants {
                nv12: true,
                ..Default::default()
            },
            &mut out,
        );
        let mean = |v: &[u8]| v.iter().map(|b| *b as u32).sum::<u32>() / v.len() as u32;
        assert!(mean(&out.nv12[..32]) + 20 < mean(&data[..32]));
    }

    /// Swatches from both graders: the GPU stacks lattices in one texture,
    /// so a mix-up between looks shows as a wrong colour here.
    #[test]
    fn swatches_render_each_look() {
        let (w, h) = (64, 36);
        let data = nv12_frame(w, h);
        let input = Input {
            data: &data,
            width: w,
            height: h,
            stride: w,
            format: InputFormat::Nv12,
        };
        let looks = [
            Grade::default(),
            Grade {
                exposure: -2.,
                ..Default::default()
            },
            Grade {
                exposure: 1.,
                ..Default::default()
            },
        ]
        .map(|grade| bake(&grade, None, 0., SWATCH_LUT));
        let wants = Wants {
            preview: Some((8, 4)),
            swatches: 3,
            ..Default::default()
        };
        let mean = |pixels: &[u8]| {
            pixels.chunks(4).map(|p| p[1] as u32).sum::<u32>() / (pixels.len() / 4) as u32
        };
        let render = |grader: &mut Grader| {
            grader.set_swatches(&looks);
            let mut out = Outputs::default();
            grader
                .process(&input, YuvMatrix::BT601, wants, &mut out)
                .unwrap();
            let size = SWATCH.0 as usize * SWATCH.1 as usize * 4;
            assert_eq!(out.swatches.len(), size * 3);
            assert_eq!(out.preview.len(), 8 * 4 * 4);
            let means: Vec<u32> = out.swatches.chunks(size).map(mean).collect();
            (means, mean(&out.original))
        };
        let (cpu, original) = render(&mut Grader::Cpu(CpuGrader::default()));
        assert!(cpu[1] + 30 < cpu[0] && cpu[0] + 30 < cpu[2], "{cpu:?}");
        assert!(cpu[0].abs_diff(original) <= 3, "identity swatch {cpu:?}");
        match GpuGrader::new() {
            Ok(gpu) => {
                let (gpu, _) = render(&mut Grader::Gpu(Box::new(gpu)));
                for (a, b) in cpu.iter().zip(&gpu) {
                    assert!(a.abs_diff(*b) <= 4, "cpu {cpu:?} gpu {gpu:?}");
                }
            }
            Err(error) => eprintln!("no GPU for this test: {error:#}"),
        }
    }

    #[test]
    fn the_bar_picture_averages_fine_detail_on_cpu_and_gpu() {
        let (w, h) = (BAR.0 * 2, BAR.1 * 2);
        // One-pixel stripes, black and white: point sampling would alias
        // them into bands; averaging gives an even mid grey.
        let mut stripes = nv12_frame(w, h);
        for (i, luma) in stripes[..(w * h) as usize].iter_mut().enumerate() {
            *luma = if i % 2 == 0 { 16 } else { 235 };
        }
        stripes[(w * h) as usize..].fill(128);
        let ramp = nv12_frame(w, h);
        let wants = Wants {
            nv12: true,
            bar: true,
            ..Default::default()
        };
        let render = |grader: &mut Grader, data: &[u8]| {
            let input = Input {
                data,
                width: w,
                height: h,
                stride: w,
                format: InputFormat::Nv12,
            };
            let mut out = Outputs::default();
            grader
                .process(&input, YuvMatrix::BT601, wants, &mut out)
                .unwrap();
            assert_eq!(out.bar.len(), BAR.0 as usize * BAR.1 as usize * 4);
            out.bar
        };
        let even = |bar: &[u8]| {
            let greens: Vec<u8> = bar.chunks(4).map(|p| p[1]).collect();
            let (low, high) = (greens.iter().min().unwrap(), greens.iter().max().unwrap());
            assert!(
                *low >= 120 && *high <= 136,
                "stripes shimmer: {low}..{high}"
            );
        };
        let mut cpu = Grader::Cpu(CpuGrader::default());
        even(&render(&mut cpu, &stripes));
        let reference = render(&mut cpu, &ramp);
        match GpuGrader::new() {
            Ok(gpu) => {
                let mut gpu = Grader::Gpu(Box::new(gpu));
                even(&render(&mut gpu, &stripes));
                let worst = render(&mut gpu, &ramp)
                    .iter()
                    .zip(&reference)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(worst <= 6, "GPU differs from CPU by {worst}");
            }
            Err(error) => eprintln!("no GPU for this test: {error:#}"),
        }
        // Without a request there is no picture.
        let mut out = Outputs::default();
        let input = Input {
            data: &ramp,
            width: w,
            height: h,
            stride: w,
            format: InputFormat::Nv12,
        };
        cpu.process(
            &input,
            YuvMatrix::BT601,
            Wants {
                bar: false,
                ..wants
            },
            &mut out,
        )
        .unwrap();
        assert!(out.bar.is_empty());
    }
}
