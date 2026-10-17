//! The voice test: a short recording of the raw microphone and of Omalive
//! Mic side by side, measured against podcast and call targets, with plain
//! advice about what to change.
use crate::analysis::{Floor, Loudness, rms_db};

pub const RATE: usize = 48_000;
/// How long a test records.
pub const SECONDS: usize = 10;

/// Loudness the processed voice should land in, in LUFS: podcast platforms
/// ask for about −16 (stereo) to −19 (mono); calls sit in the same range.
pub const LOUDNESS_TARGET: (f32, f32) = (-19., -15.);
/// Where speech should peak at the interface, in dBFS, before processing.
pub const INPUT_PEAK_TARGET: (f32, f32) = (-18., -6.);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Good,
    Warning,
    Bad,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub verdict: Verdict,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assessment {
    /// Raw microphone peak, dBFS.
    pub input_peak: f32,
    /// Omalive Mic peak, dBFS.
    pub output_peak: f32,
    /// Omalive Mic integrated loudness (BS.1770, gated), LUFS.
    pub loudness: Option<f32>,
    /// Background level between words, dBFS: the quietest tenth of 100 ms
    /// blocks.
    pub input_floor: Option<f32>,
    pub output_floor: Option<f32>,
    pub notes: Vec<Note>,
}

/// Peak in dBFS, floored at −90.
pub fn peak_db(samples: &[f32]) -> f32 {
    let peak = samples
        .iter()
        .filter(|s| s.is_finite())
        .fold(0f32, |m, s| m.max(s.abs()));
    (20. * peak.max(1e-9).log10()).max(-90.)
}

/// Integrated loudness of mono 48 kHz audio per ITU-R BS.1770-4: 400 ms
/// blocks every 100 ms, an absolute gate at −70 LUFS and a relative gate
/// 10 LU below the absolutely-gated mean. `None` when nothing passes.
pub fn integrated_loudness(samples: &[f32]) -> Option<f32> {
    const HOP: usize = RATE / 10;
    let mut weighting = Loudness::default();
    let mut weighted = Vec::with_capacity(samples.len());
    for chunk in samples.chunks(HOP) {
        weighted.extend(weighting.weigh(chunk));
    }
    let hops: Vec<f64> = weighted
        .chunks_exact(HOP)
        .map(|hop| hop.iter().map(|s| s * s).sum::<f64>() / HOP as f64)
        .collect();
    let blocks: Vec<f64> = hops
        .windows(4)
        .map(|w| w.iter().sum::<f64>() / 4.)
        .collect();
    let lufs = |power: f64| -0.691 + 10. * power.max(1e-12).log10();
    let absolute: Vec<f64> = blocks.into_iter().filter(|&b| lufs(b) > -70.).collect();
    if absolute.is_empty() {
        return None;
    }
    let mean = absolute.iter().sum::<f64>() / absolute.len() as f64;
    let relative: Vec<f64> = absolute
        .into_iter()
        .filter(|&b| lufs(b) > lufs(mean) - 10.)
        .collect();
    let gated = relative.iter().sum::<f64>() / relative.len().max(1) as f64;
    Some(lufs(gated) as f32)
}

/// Background level: the 10th percentile of 100 ms RMS blocks.
fn floor(samples: &[f32]) -> Option<f32> {
    let mut floor = Floor::new(samples.len() / (RATE / 10) + 1);
    for block in samples.chunks_exact(RATE / 10) {
        floor.push(rms_db(block));
    }
    floor.percentile(0.1)
}

/// Measure a test recording and say what to change.
pub fn assess(input: &[f32], output: &[f32]) -> Assessment {
    let input_peak = peak_db(input);
    let output_peak = peak_db(output);
    let loudness = integrated_loudness(output);
    let input_floor = floor(input);
    let output_floor = floor(output);
    let mut notes = Vec::new();
    let mut say = |verdict, text: String| notes.push(Note { verdict, text });

    if input_peak < -40. {
        say(
            Verdict::Bad,
            "No speech came through. Check the input, then talk for the whole test.".into(),
        );
    } else {
        let (low, high) = INPUT_PEAK_TARGET;
        if input_peak > -1. {
            say(
                Verdict::Bad,
                format!(
                    "Your input clips (peaks at {input_peak:.0} dBFS). Turn the interface gain down until speech peaks around −10 dBFS."
                ),
            );
        } else if input_peak < low {
            say(
                Verdict::Warning,
                format!(
                    "Your input is quiet (peaks at {input_peak:.0} dBFS). Turn the interface gain up, or move closer, until speech peaks around −12 to −10 dBFS."
                ),
            );
        } else if input_peak > high {
            say(
                Verdict::Warning,
                format!(
                    "Your input is hot (peaks at {input_peak:.0} dBFS). A little less gain leaves room for laughs and plosives."
                ),
            );
        } else {
            say(
                Verdict::Good,
                format!("Input level is healthy (peaks at {input_peak:.0} dBFS)."),
            );
        }
        match loudness {
            Some(l) if l < LOUDNESS_TARGET.0 - 3. => say(
                Verdict::Warning,
                format!(
                    "You come out quiet ({l:.0} LUFS; aim for −16 to −19). Raise the compressor's makeup gain or the output trim."
                ),
            ),
            Some(l) if l > LOUDNESS_TARGET.1 + 2. => say(
                Verdict::Warning,
                format!(
                    "You come out loud ({l:.0} LUFS; aim for −16 to −19). Lower the makeup gain or the output trim."
                ),
            ),
            Some(l) => say(
                Verdict::Good,
                format!("Loudness is on target ({l:.0} LUFS)."),
            ),
            None => {}
        }
        match output_floor {
            Some(f) if f > -55. => say(
                Verdict::Warning,
                format!(
                    "Background noise is audible between words ({f:.0} dBFS). Turn on noise suppression or raise its voice gate."
                ),
            ),
            Some(f) => say(Verdict::Good, format!("Quiet between words ({f:.0} dBFS).")),
            None => {}
        }
    }
    Assessment {
        input_peak,
        output_peak,
        loudness,
        input_floor,
        output_floor,
        notes,
    }
}

/// Mono 32-bit float WAV bytes.
pub fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 4) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, amplitude: f32, seconds: f32) -> Vec<f32> {
        (0..(RATE as f32 * seconds) as usize)
            .map(|n| amplitude * (std::f32::consts::TAU * frequency * n as f32 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn a_reference_tone_reads_its_bs1770_loudness() {
        // A full-scale 997 Hz sine in one channel is −3.01 LUFS; at −20 dBFS
        // peak it is −23.01.
        let tone = sine(997., 0.1, 5.);
        let l = integrated_loudness(&tone).unwrap();
        assert!((l - -23.01).abs() < 0.2, "{l}");
        assert_eq!(integrated_loudness(&vec![0.; RATE]), None);
    }

    #[test]
    fn gating_ignores_silence_between_words() {
        let mut speech = sine(997., 0.1, 2.);
        speech.extend(vec![0.; RATE * 3]);
        speech.extend(sine(997., 0.1, 2.));
        let l = integrated_loudness(&speech).unwrap();
        assert!((l - -23.01).abs() < 0.5, "{l}");
    }

    /// Speech-like: bursts of tone with near-silence between them.
    fn talk(frequency: f32, amplitude: f32) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..4 {
            out.extend(sine(frequency, amplitude, 0.8));
            out.extend(vec![1e-4; RATE * 2 / 5]);
        }
        out
    }

    #[test]
    fn advice_follows_the_numbers() {
        let silent = vec![0.; RATE * 2];
        let notes = assess(&silent, &silent).notes;
        assert!(notes[0].text.starts_with("No speech"));

        // Quiet input, quiet result, clean background.
        let quiet = sine(300., 0.05, 3.);
        let a = assess(&quiet, &quiet);
        assert!(
            a.notes
                .iter()
                .any(|n| n.text.starts_with("Your input is quiet"))
        );
        assert!(
            a.notes
                .iter()
                .any(|n| n.text.starts_with("You come out quiet"))
        );

        // Clipping input.
        let hot = sine(300., 1.0, 3.);
        let a = assess(&hot, &sine(300., 0.25, 3.));
        assert!(a.notes[0].text.starts_with("Your input clips"));
        assert_eq!(a.notes[0].verdict, Verdict::Bad);

        // Healthy: peaks around −12 dBFS in, about −17 LUFS out, quiet
        // between words.
        let a = assess(&talk(300., 0.25), &talk(997., 0.2));
        assert!(
            a.notes.iter().all(|n| n.verdict == Verdict::Good),
            "{:?}",
            a.notes
        );
        assert_eq!(a.notes.len(), 3);

        // Noise between words.
        let mut noisy = talk(997., 0.2);
        for (i, s) in noisy.iter_mut().enumerate() {
            *s += 0.01 * ((i * 7919 % 101) as f32 / 50. - 1.);
        }
        let a = assess(&talk(300., 0.25), &noisy);
        assert!(
            a.notes
                .iter()
                .any(|n| n.text.starts_with("Background noise"))
        );
    }

    #[test]
    fn wav_header_describes_float_mono() {
        let bytes = wav(&[0.5, -0.5], 48_000);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3);
        assert_eq!(
            u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            48_000
        );
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(
            f32::from_le_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]),
            0.5
        );
    }
}
