//! Signal analysis for the visualisations: spectrum, loudness, noise floor
//! and an estimate of compressor gain reduction. Allocation-free after
//! construction so it can run every frame.
use std::f32::consts::PI;

/// Hann-windowed power spectrum of `N` samples, in dB relative to a
/// full-scale sine, for bins `0..N/2`.
pub struct Fft {
    size: usize,
    window: Vec<f32>,
    twiddles: Vec<(f32, f32)>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Window gain normalisation, so a full-scale sine reads 0 dB.
    norm: f32,
}

impl Fft {
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two() && size >= 16);
        let window: Vec<f32> = (0..size)
            .map(|i| 0.5 - 0.5 * (2. * PI * i as f32 / size as f32).cos())
            .collect();
        let sum: f32 = window.iter().sum();
        Self {
            size,
            twiddles: (0..size / 2)
                .map(|k| {
                    let angle = -2. * PI * k as f32 / size as f32;
                    (angle.cos(), angle.sin())
                })
                .collect(),
            window,
            re: vec![0.; size],
            im: vec![0.; size],
            norm: 2. / sum,
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Magnitudes in dBFS for bins `0..size/2`, written into `out`.
    pub fn power_db(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        let n = self.size;
        let offset = samples.len().saturating_sub(n);
        for i in 0..n {
            let sample = samples.get(offset + i).copied().unwrap_or(0.);
            self.re[i] = if sample.is_finite() { sample } else { 0. } * self.window[i];
            self.im[i] = 0.;
        }
        // Iterative radix-2: bit reversal, then butterflies.
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                self.re.swap(i, j);
                self.im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = self.twiddles[k * step];
                    let (a, b) = (start + k, start + k + len / 2);
                    let tr = self.re[b] * wr - self.im[b] * wi;
                    let ti = self.re[b] * wi + self.im[b] * wr;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
            }
            len *= 2;
        }
        out.clear();
        out.extend((0..n / 2).map(|k| {
            let magnitude = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * self.norm;
            20. * magnitude.max(1e-6).log10()
        }));
    }
}

/// Resample FFT bins onto `columns` log-spaced frequency columns between
/// `low` and `high` Hz: the loudest bin in each column, interpolating where
/// a column is narrower than a bin.
pub fn log_columns(
    bins: &[f32],
    sample_rate: f32,
    low: f32,
    high: f32,
    columns: usize,
    out: &mut Vec<f32>,
) {
    out.clear();
    if bins.is_empty() || columns == 0 {
        return;
    }
    let bin_hz = sample_rate / (2. * bins.len() as f32);
    let at = |f: f32| {
        let position = (f / bin_hz).clamp(0., (bins.len() - 1) as f32);
        let i = position.floor() as usize;
        let next = (i + 1).min(bins.len() - 1);
        bins[i] + (bins[next] - bins[i]) * (position - i as f32)
    };
    let ratio = (high / low).ln();
    for c in 0..columns {
        let f0 = low * (ratio * c as f32 / columns as f32).exp();
        let f1 = low * (ratio * (c + 1) as f32 / columns as f32).exp();
        let (b0, b1) = (
            (f0 / bin_hz).ceil() as usize,
            (f1 / bin_hz).floor() as usize,
        );
        let value = if b1 > b0 && b1 < bins.len() {
            bins[b0..=b1].iter().copied().fold(f32::MIN, f32::max)
        } else {
            at((f0 * f1).sqrt())
        };
        out.push(value);
    }
}

/// ITU-R BS.1770 K-weighting at 48 kHz and short-term (3 s) loudness.
pub struct Loudness {
    stages: [Biquad; 2],
    block: f64,
    block_count: usize,
    blocks: Vec<f64>,
    next: usize,
    filled: usize,
}

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

const BLOCK: usize = 4800; // 100 ms at 48 kHz
const SHORT_TERM_BLOCKS: usize = 30; // 3 s

impl Default for Loudness {
    fn default() -> Self {
        Self {
            stages: [
                Biquad {
                    b: [
                        1.535_124_859_586_97,
                        -2.691_696_189_406_38,
                        1.198_392_810_852_85,
                    ],
                    a: [-1.690_659_293_182_41, 0.732_480_774_215_85],
                    z: [0.; 2],
                },
                Biquad {
                    b: [1., -2., 1.],
                    a: [-1.990_047_454_833_98, 0.990_072_250_366_21],
                    z: [0.; 2],
                },
            ],
            block: 0.,
            block_count: 0,
            blocks: vec![0.; SHORT_TERM_BLOCKS],
            next: 0,
            filled: 0,
        }
    }
}

impl Loudness {
    pub fn push(&mut self, samples: &[f32]) {
        for sample in samples {
            let x = if sample.is_finite() {
                *sample as f64
            } else {
                0.
            };
            let shelf = self.stages[0].process(x);
            let y = self.stages[1].process(shelf);
            self.block += y * y;
            self.block_count += 1;
            if self.block_count == BLOCK {
                self.blocks[self.next] = self.block / BLOCK as f64;
                self.next = (self.next + 1) % SHORT_TERM_BLOCKS;
                self.filled = (self.filled + 1).min(SHORT_TERM_BLOCKS);
                self.block = 0.;
                self.block_count = 0;
            }
        }
    }

    /// K-weight samples without measuring them (for integrated loudness).
    pub fn weigh(&mut self, samples: &[f32]) -> Vec<f64> {
        samples
            .iter()
            .map(|sample| {
                let x = if sample.is_finite() {
                    *sample as f64
                } else {
                    0.
                };
                let shelf = self.stages[0].process(x);
                self.stages[1].process(shelf)
            })
            .collect()
    }

    /// Short-term loudness in LUFS, or `None` before a block is complete.
    /// Mono is weighted as a single front channel.
    pub fn short_term(&self) -> Option<f32> {
        if self.filled == 0 {
            return None;
        }
        let mean = self.blocks.iter().take(self.filled).sum::<f64>() / self.filled as f64;
        Some((-0.691 + 10. * mean.max(1e-12).log10()) as f32)
    }
}

/// Level percentiles over a rolling window of short blocks; the 10th
/// percentile approximates the noise floor between words.
pub struct Floor {
    levels: Vec<f32>,
    next: usize,
    filled: usize,
    scratch: Vec<f32>,
}

impl Floor {
    pub fn new(blocks: usize) -> Self {
        Self {
            levels: vec![-120.; blocks],
            next: 0,
            filled: 0,
            scratch: Vec::with_capacity(blocks),
        }
    }

    pub fn push(&mut self, db: f32) {
        self.levels[self.next] = db;
        self.next = (self.next + 1) % self.levels.len();
        self.filled = (self.filled + 1).min(self.levels.len());
    }

    pub fn percentile(&mut self, p: f32) -> Option<f32> {
        if self.filled < self.levels.len() / 4 {
            return None;
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.levels[..self.filled]);
        self.scratch.sort_by(|a, b| a.total_cmp(b));
        let index = ((self.filled - 1) as f32 * p).round() as usize;
        Some(self.scratch[index])
    }
}

/// RMS of a block in dBFS.
pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.;
    }
    let sum: f64 = samples
        .iter()
        .map(|s| {
            if s.is_finite() {
                (*s as f64) * (*s as f64)
            } else {
                0.
            }
        })
        .sum();
    (10. * (sum / samples.len() as f64).max(1e-12).log10()) as f32
}

/// Gain reduction implied by a hard-knee compressor's static curve, given
/// the level at its output: out = in − GR + makeup, GR = (in − T)(1 − 1/R).
pub fn gain_reduction(output_db: f32, threshold: f32, ratio: f32, makeup: f32) -> f32 {
    let before_makeup = output_db - makeup;
    if before_makeup <= threshold || ratio <= 1. {
        return 0.;
    }
    let input = threshold + (before_makeup - threshold) * ratio;
    (input - threshold) * (1. - 1. / ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, amplitude: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amplitude * (2. * PI * frequency * i as f32 / 48000.).sin())
            .collect()
    }

    #[test]
    fn full_scale_sine_reads_zero_db_at_its_bin() {
        let mut fft = Fft::new(2048);
        let mut out = Vec::new();
        // 1500 Hz sits exactly on bin 64 at 48 kHz / 2048.
        fft.power_db(&sine(1500., 1., 2048), &mut out);
        let (peak, value) = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert_eq!(peak, 64);
        assert!(value.abs() < 0.5, "{value}");
        assert!(out[300] < -60.);
        fft.power_db(&sine(1500., 0.1, 2048), &mut out);
        assert!((out[64] + 20.).abs() < 0.5);
    }

    #[test]
    fn log_columns_place_energy_by_frequency() {
        let mut fft = Fft::new(2048);
        let (mut bins, mut columns) = (Vec::new(), Vec::new());
        fft.power_db(&sine(1000., 0.5, 2048), &mut bins);
        log_columns(&bins, 48000., 20., 20000., 100, &mut columns);
        let loudest = columns
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        // 1 kHz is (ln 50 / ln 1000) of the way along a 20 Hz–20 kHz axis.
        let expected = ((1000f32 / 20.).ln() / 1000f32.ln() * 100.) as usize;
        assert!(loudest.abs_diff(expected) <= 1, "{loudest} vs {expected}");
    }

    #[test]
    fn k_weighted_loudness_of_a_reference_tone() {
        // BS.1770: a 997 Hz sine at −20 dBFS reads about −23 LUFS (mono,
        // single channel), allowing for the filter's gain near 1 kHz.
        let mut loudness = Loudness::default();
        assert!(loudness.short_term().is_none());
        loudness.push(&sine(997., 0.1, 48000 * 3));
        let lufs = loudness.short_term().unwrap();
        assert!((lufs + 23.).abs() < 0.6, "{lufs}");
    }

    #[test]
    fn floor_and_reduction_estimates() {
        let mut floor = Floor::new(40);
        assert!(floor.percentile(0.1).is_none());
        for i in 0..40 {
            floor.push(if i % 4 == 0 { -60. } else { -20. });
        }
        assert_eq!(floor.percentile(0.1), Some(-60.));
        assert_eq!(gain_reduction(-30., -18., 3., 3.), 0.);
        // Output −12 with 3 dB makeup → −15 after the curve → input −9 → GR 6.
        assert!((gain_reduction(-12., -18., 3., 3.) - 6.).abs() < 1e-4);
        assert!((rms_db(&[0.5, -0.5]) + 6.0206).abs() < 1e-3);
    }
}
