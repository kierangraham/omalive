//! Animated state behind the visualisations, advanced once per frame from
//! the engine's sample taps. Free of GPUI so the physics and analysis are
//! testable; views only read it.
use omalive_core::{
    VoiceSettings,
    analysis::{Fft, Floor, Loudness, gain_reduction, log_columns, rms_db},
};
use omalive_media::Taps;
use std::collections::VecDeque;

/// Spectrum columns analysed per frame; views resample to their width.
pub const COLUMNS: usize = 192;
pub const FLOOR_DB: f32 = -84.;
/// Samples per waveform column (≈21 ms at 48 kHz).
const BLOCK: usize = 1024;
/// Waveform history columns (≈5 s).
pub const HISTORY: usize = 240;
const FFT_SIZE: usize = 2048;

/// A damped spring: values overshoot slightly and settle, which reads as
/// "bouncy" without lagging the signal.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    pub value: f32,
    velocity: f32,
}

impl Spring {
    pub fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.,
        }
    }

    /// At rest on `target`: no frame needs to be drawn for it.
    pub fn settled(&self, target: f32) -> bool {
        (self.value - target).abs() < 1e-3 && self.velocity.abs() < 1e-3
    }

    pub fn step(&mut self, target: f32, dt: f32) {
        const STIFFNESS: f32 = 900.;
        const DAMPING: f32 = 36.; // damping ratio ≈ 0.6
        // Sub-step so long frames stay stable.
        let steps = (dt / (1. / 240.)).ceil().clamp(1., 8.) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let acceleration = STIFFNESS * (target - self.value) - DAMPING * self.velocity;
            self.velocity += acceleration * h;
            self.value += self.velocity * h;
        }
    }
}

/// The numbers shown beside the graphs. They refresh a few times a second
/// like a hardware meter: readable rather than a blur of decimals, and each
/// new string costs a text shaping pass on the UI thread.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Readout {
    pub peak: f32,
    pub hold: f32,
    pub rms: f32,
    pub reduction: f32,
    pub lufs: Option<f32>,
    pub noise_removed: Option<f32>,
}

/// Seconds between readout refreshes.
const READOUT_PERIOD: f32 = 0.2;

/// Raw and processed samples kept while a voice test records.
#[derive(Default)]
pub struct Recording {
    pub input: Vec<f32>,
    pub output: Vec<f32>,
}

pub struct AudioVisuals {
    /// Set while a voice test records; filled as samples arrive.
    pub recording: Option<Recording>,
    fft: Fft,
    window: Vec<f32>,
    bins: Vec<f32>,
    columns: Vec<f32>,
    /// Raw microphone spectrum, dB per column (smoothed).
    pub input: Vec<f32>,
    /// Omalive Mic spectrum, dB per column (springs).
    pub output: Vec<Spring>,
    /// Falling peak marks over the output spectrum.
    pub peaks: Vec<f32>,
    /// Waveform history: peak amplitude 0…1 per column, input and output.
    pub wave_input: VecDeque<f32>,
    pub wave_output: VecDeque<f32>,
    input_position: u64,
    output_position: u64,
    input_block: Vec<f32>,
    output_block: Vec<f32>,
    scratch: Vec<f32>,
    loudness: Loudness,
    floor_input: Floor,
    floor_output: Floor,
    pub lufs: Option<f32>,
    pub noise_removed: Option<f32>,
    pub peak: Spring,
    pub rms: Spring,
    pub hold: f32,
    hold_age: f32,
    pub reduction: Spring,
    /// Recent output activity 0…1, for pulses along the chain.
    pub activity: f32,
    pub readout: Readout,
    readout_age: f32,
}

impl Default for AudioVisuals {
    fn default() -> Self {
        Self {
            recording: None,
            fft: Fft::new(FFT_SIZE),
            window: Vec::with_capacity(FFT_SIZE),
            bins: Vec::with_capacity(FFT_SIZE / 2),
            columns: Vec::with_capacity(COLUMNS),
            input: vec![FLOOR_DB; COLUMNS],
            output: vec![Spring::at(FLOOR_DB); COLUMNS],
            peaks: vec![FLOOR_DB; COLUMNS],
            wave_input: VecDeque::from(vec![0.; HISTORY]),
            wave_output: VecDeque::from(vec![0.; HISTORY]),
            input_position: 0,
            output_position: 0,
            input_block: Vec::with_capacity(BLOCK),
            output_block: Vec::with_capacity(BLOCK),
            scratch: Vec::with_capacity(16384),
            loudness: Loudness::default(),
            floor_input: Floor::new(160),
            floor_output: Floor::new(160),
            lufs: None,
            noise_removed: None,
            peak: Spring::at(-60.),
            rms: Spring::at(-60.),
            hold: -60.,
            hold_age: 0.,
            reduction: Spring::default(),
            activity: 0.,
            readout: Readout {
                peak: -60.,
                hold: -60.,
                rms: -60.,
                ..Readout::default()
            },
            readout_age: READOUT_PERIOD,
        }
    }
}

impl AudioVisuals {
    /// Start reading from the taps' current position, so a reopened page
    /// does not replay old audio.
    pub fn resync(&mut self, taps: &Taps) {
        self.input_position = taps.input.written();
        self.output_position = taps.output.written();
    }

    pub fn update(&mut self, taps: &Taps, voice: &VoiceSettings, dt: f32) {
        let dt = dt.clamp(1. / 240., 0.1);
        // New samples: waveform blocks, loudness and noise floors.
        self.scratch.clear();
        self.output_position = taps
            .output
            .read_since(self.output_position, &mut self.scratch);
        self.loudness.push(&self.scratch);
        if let Some(recording) = &mut self.recording {
            recording.output.extend_from_slice(&self.scratch);
        }
        let mut block_rms = None;
        for sample in &self.scratch {
            self.output_block.push(*sample);
            if self.output_block.len() == BLOCK {
                let peak = self.output_block.iter().fold(0f32, |m, s| m.max(s.abs()));
                let rms = rms_db(&self.output_block);
                push_history(&mut self.wave_output, peak.min(1.));
                self.floor_output.push(rms);
                block_rms = Some(rms);
                self.output_block.clear();
            }
        }
        self.scratch.clear();
        self.input_position = taps
            .input
            .read_since(self.input_position, &mut self.scratch);
        if let Some(recording) = &mut self.recording {
            recording.input.extend_from_slice(&self.scratch);
        }
        for sample in &self.scratch {
            self.input_block.push(*sample);
            if self.input_block.len() == BLOCK {
                let peak = self.input_block.iter().fold(0f32, |m, s| m.max(s.abs()));
                push_history(&mut self.wave_input, peak.min(1.));
                self.floor_input.push(rms_db(&self.input_block));
                self.input_block.clear();
            }
        }
        self.lufs = self.loudness.short_term();
        self.noise_removed = match (
            self.floor_input.percentile(0.1),
            self.floor_output.percentile(0.1),
        ) {
            (Some(input), Some(output)) => Some((input - output).max(0.)),
            _ => None,
        };

        // Spectra.
        taps.input.latest(FFT_SIZE, &mut self.window);
        self.fft.power_db(&self.window, &mut self.bins);
        log_columns(&self.bins, 48000., 20., 20000., COLUMNS, &mut self.columns);
        let smoothing = 1. - (-dt * 18.).exp();
        for (smoothed, target) in self.input.iter_mut().zip(&self.columns) {
            *smoothed += (target.max(FLOOR_DB) - *smoothed) * smoothing;
        }
        taps.output.latest(FFT_SIZE, &mut self.window);
        let peak_db = 20.
            * self.window[FFT_SIZE - BLOCK..]
                .iter()
                .fold(0f32, |m, s| m.max(s.abs()))
                .max(1e-9)
                .log10();
        let rms = rms_db(&self.window[FFT_SIZE - BLOCK..]);
        self.fft.power_db(&self.window, &mut self.bins);
        log_columns(&self.bins, 48000., 20., 20000., COLUMNS, &mut self.columns);
        for ((spring, peak), target) in self
            .output
            .iter_mut()
            .zip(self.peaks.iter_mut())
            .zip(&self.columns)
        {
            spring.step(target.max(FLOOR_DB), dt);
            spring.value = spring.value.clamp(FLOOR_DB, 0.);
            *peak = (*peak - 24. * dt).max(spring.value);
        }

        // Meters.
        self.peak.step(peak_db.clamp(-60., 0.), dt);
        self.rms.step(rms.clamp(-60., 0.), dt);
        self.peak.value = self.peak.value.clamp(-60., 0.);
        self.rms.value = self.rms.value.clamp(-60., 0.);
        self.hold_age += dt;
        if self.peak.value >= self.hold {
            self.hold = self.peak.value;
            self.hold_age = 0.;
        } else if self.hold_age > 1.2 {
            self.hold = (self.hold - 30. * dt).max(-60.);
        }
        let reduction = if voice.compression {
            block_rms
                .map(|rms| gain_reduction(rms, voice.threshold, voice.ratio, voice.makeup))
                .unwrap_or(self.reduction.value)
        } else {
            0.
        };
        self.reduction.step(reduction, dt);
        self.reduction.value = self.reduction.value.max(0.);
        let target = ((rms + 50.) / 40.).clamp(0., 1.);
        self.activity += (target - self.activity) * (1. - (-dt * 8.).exp());
        self.readout_age += dt;
        if self.readout_age >= READOUT_PERIOD {
            self.readout_age = 0.;
            self.readout = Readout {
                peak: self.peak.value,
                hold: self.hold,
                rms: self.rms.value,
                reduction: self.reduction.value,
                lufs: self.lufs,
                noise_removed: self.noise_removed,
            };
        }
    }
}

fn push_history(history: &mut VecDeque<f32>, value: f32) {
    if history.len() == HISTORY {
        history.pop_front();
    }
    history.push_back(value);
}

/// RGB parade of a BGRA image: for each of `columns` columns, the share of
/// pixels at each of `rows` levels, per channel (B, G, R order kept).
pub fn parade(bgra: &[u8], width: u32, height: u32, columns: usize, rows: usize) -> Vec<[f32; 3]> {
    let mut counts = vec![[0u32; 3]; columns * rows];
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        return vec![[0.; 3]; columns * rows];
    }
    // A sparse sample is plenty for a scope and keeps this well under 1 ms.
    for y in (0..h).step_by(3) {
        for x in (0..w).step_by(3) {
            let i = (y * w + x) * 4;
            let column = x * columns / w;
            for (channel, value) in [bgra[i + 2], bgra[i + 1], bgra[i]].into_iter().enumerate() {
                let row = value as usize * rows / 256;
                counts[row * columns + column][channel] += 1;
            }
        }
    }
    let samples_per_column = ((h / 3 + 1) * (w / 3 + 1) / columns).max(1) as f32;
    counts
        .into_iter()
        .map(|c| c.map(|n| (n as f32 / samples_per_column * 6.).min(1.)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(frequency: f32, amplitude: f32, n: usize, offset: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amplitude * (2. * PI * frequency * (i + offset) as f32 / 48000.).sin())
            .collect()
    }

    #[test]
    fn springs_overshoot_then_settle() {
        let mut spring = Spring::at(0.);
        let mut peak = 0f32;
        for _ in 0..120 {
            spring.step(1., 1. / 60.);
            peak = peak.max(spring.value);
        }
        assert!(peak > 1.02, "bouncy: {peak}");
        assert!(
            (spring.value - 1.).abs() < 0.01,
            "settled: {}",
            spring.value
        );
        let mut stable = Spring::at(0.);
        stable.step(1., 0.1);
        assert!(stable.value.is_finite() && stable.value < 2.);
    }

    #[test]
    fn taps_drive_spectrum_wave_and_meters() {
        let taps = Taps::default();
        let mut visuals = AudioVisuals::default();
        let voice = VoiceSettings::default();
        let mut offset = 0;
        let mut seed = 0x2545_f491u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2. - 1.
        };
        for _ in 0..240 {
            // One 60 fps frame of audio: a 1 kHz tone, louder at the input
            // with broadband noise on top.
            let output = tone(1000., 0.25, 800, offset);
            let input: Vec<f32> = output.iter().map(|s| s * 1.2 + 0.02 * noise()).collect();
            offset += 800;
            taps.output.push(&output);
            taps.input.push(&input);
            visuals.update(&taps, &voice, 1. / 60.);
        }
        let loudest = (0..COLUMNS)
            .max_by(|a, b| {
                visuals.output[*a]
                    .value
                    .total_cmp(&visuals.output[*b].value)
            })
            .unwrap();
        let expected = ((1000f32 / 20.).ln() / 1000f32.ln() * COLUMNS as f32) as usize;
        assert!(loudest.abs_diff(expected) <= 2, "{loudest} vs {expected}");
        assert!(
            visuals.input[COLUMNS - 10] > visuals.output[COLUMNS - 10].value + 10.,
            "noise only at the input"
        );
        assert!(
            visuals
                .wave_output
                .iter()
                .rev()
                .take(10)
                .all(|v| (v - 0.25).abs() < 0.02)
        );
        assert!(
            (visuals.peak.value + 12.).abs() < 1.,
            "{}",
            visuals.peak.value
        );
        assert!(visuals.lufs.is_some());
        assert!(visuals.activity > 0.5);
    }

    #[test]
    fn parade_counts_levels_per_channel() {
        // A 6×6 image: pure red on the left half, black on the right.
        let mut bgra = Vec::new();
        for _ in 0..6 {
            for x in 0..6 {
                bgra.extend_from_slice(if x < 3 {
                    &[0, 0, 255, 255]
                } else {
                    &[0, 0, 0, 255]
                });
            }
        }
        let scope = parade(&bgra, 6, 6, 2, 4);
        assert!(scope[3 * 2][0] > 0., "red is high on the left");
        assert_eq!(scope[3 * 2 + 1][0], 0., "nothing bright on the right");
        assert!(scope[1][1] > 0., "green sits at black on the right");
    }
}
