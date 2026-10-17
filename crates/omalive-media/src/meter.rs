//! Output level of the virtual microphone, written by the audio thread and
//! read by the UI at display rate. Lock-free: the RT callback never blocks.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Default)]
pub struct Meter {
    peak: AtomicU32,
    rms: AtomicU32,
    /// Loudest sample since the last [`Meter::take_peak`], so a slow reader
    /// still sees short syllables. Non-negative f32 bits order like the
    /// floats themselves, so `fetch_max` on the bits is a float max.
    held: AtomicU32,
    /// Monotonic count of analysed buffers; unchanged means no fresh audio.
    buffers: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// dBFS, floored at [`FLOOR`].
    pub peak: f32,
    pub rms: f32,
}

pub const FLOOR: f32 = -90.;

impl Meter {
    /// Analyse one buffer of little-endian f32 samples.
    pub fn analyse(&self, bytes: &[u8]) {
        let (peak, sum, count) = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .map(|s| if s.is_finite() { s } else { 0. })
            .fold((0f32, 0f64, 0usize), |(peak, sum, count), s| {
                (peak.max(s.abs()), sum + (s as f64) * (s as f64), count + 1)
            });
        if count == 0 {
            return;
        }
        let rms = (sum / count as f64).sqrt() as f32;
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
        self.rms.store(rms.to_bits(), Ordering::Relaxed);
        self.held.fetch_max(peak.to_bits(), Ordering::Relaxed);
        self.buffers.fetch_add(1, Ordering::Release);
    }

    /// The loudest peak since the last call, in dBFS, and reset it.
    pub fn take_peak(&self) -> f32 {
        db(f32::from_bits(self.held.swap(0, Ordering::Relaxed)))
    }

    pub fn buffers(&self) -> u64 {
        self.buffers.load(Ordering::Acquire)
    }

    pub fn levels(&self) -> Levels {
        Levels {
            peak: db(f32::from_bits(self.peak.load(Ordering::Relaxed))),
            rms: db(f32::from_bits(self.rms.load(Ordering::Relaxed))),
        }
    }

    pub fn reset(&self) {
        self.peak.store(0, Ordering::Relaxed);
        self.rms.store(0, Ordering::Relaxed);
        self.held.store(0, Ordering::Relaxed);
    }
}

/// Samples kept per tap: a 2048-point spectrum plus headroom for a slow frame.
const RING: usize = 1 << 14;

/// A lock-free ring of recent mono samples. The realtime callback writes;
/// the UI copies out whatever arrived since it last looked. A reader that
/// races the writer may see a few samples from the next lap, which is
/// harmless for display and keeps the audio thread wait-free.
pub struct Tap {
    ring: Box<[AtomicU32]>,
    written: AtomicU64,
}

impl Default for Tap {
    fn default() -> Self {
        Self {
            ring: (0..RING).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
        }
    }
}

impl Tap {
    pub fn push_bytes(&self, bytes: &[u8]) {
        let mut written = self.written.load(Ordering::Relaxed);
        for chunk in bytes.chunks_exact(4) {
            let sample = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            self.ring[written as usize & (RING - 1)].store(sample.to_bits(), Ordering::Relaxed);
            written += 1;
        }
        self.written.store(written, Ordering::Release);
    }

    pub fn push(&self, samples: &[f32]) {
        let mut written = self.written.load(Ordering::Relaxed);
        for sample in samples {
            self.ring[written as usize & (RING - 1)].store(sample.to_bits(), Ordering::Relaxed);
            written += 1;
        }
        self.written.store(written, Ordering::Release);
    }

    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Append samples written after position `from` (at most one ring) to
    /// `out`, returning the new position.
    pub fn read_since(&self, from: u64, out: &mut Vec<f32>) -> u64 {
        let written = self.written();
        let start = from.max(written.saturating_sub(RING as u64 - 1024));
        out.extend(
            (start..written).map(|i| {
                f32::from_bits(self.ring[i as usize & (RING - 1)].load(Ordering::Relaxed))
            }),
        );
        written
    }

    /// The most recent `count` samples, oldest first.
    pub fn latest(&self, count: usize, out: &mut Vec<f32>) {
        out.clear();
        let written = self.written();
        self.read_since(written.saturating_sub(count as u64), out);
        if out.len() < count {
            out.splice(0..0, std::iter::repeat_n(0., count - out.len()));
        }
    }
}

/// The voice chain's two taps: the raw microphone and Omalive Mic.
#[derive(Default)]
pub struct Taps {
    pub input: std::sync::Arc<Tap>,
    pub output: std::sync::Arc<Tap>,
}

pub fn db(linear: f32) -> f32 {
    (20. * linear.max(1e-9).log10()).max(FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn real_rms_and_nonfinite_samples() {
        let meter = Meter::default();
        meter.analyse(&bytes(&[0.5, -0.5]));
        let levels = meter.levels();
        assert!((levels.rms - -6.0206).abs() < 1e-3);
        assert_eq!(levels.rms, levels.peak);
        meter.analyse(&bytes(&[f32::NAN, f32::INFINITY]));
        assert_eq!(meter.levels().peak, FLOOR);
        assert_eq!(meter.buffers(), 2);
        meter.analyse(&[]);
        assert_eq!(meter.buffers(), 2);
    }

    #[test]
    fn held_peak_survives_quieter_buffers_until_taken() {
        let meter = Meter::default();
        meter.analyse(&bytes(&[0.5]));
        meter.analyse(&bytes(&[0.1]));
        assert!((meter.take_peak() - -6.0206).abs() < 1e-3);
        assert_eq!(meter.take_peak(), FLOOR);
    }

    #[test]
    fn taps_read_new_samples_and_the_latest_window() {
        let tap = Tap::default();
        let mut out = Vec::new();
        assert_eq!(tap.read_since(0, &mut out), 0);
        tap.push_bytes(&bytes(&[0.25, 0.5]));
        tap.push(&[0.75]);
        let position = tap.read_since(0, &mut out);
        assert_eq!((position, out.as_slice()), (3, &[0.25, 0.5, 0.75][..]));
        out.clear();
        assert_eq!(tap.read_since(position, &mut out), 3);
        assert!(out.is_empty());
        tap.latest(5, &mut out);
        assert_eq!(out, [0., 0., 0.25, 0.5, 0.75]);
        // A reader that fell a whole ring behind gets the recent part only.
        let lots = vec![1.; RING * 2];
        tap.push(&lots);
        out.clear();
        tap.read_since(0, &mut out);
        assert!(out.len() < RING);
    }
}
