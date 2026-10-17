//! Single-slot, latest-wins frame handoff between threads. A slow reader
//! never queues frames: it always gets the newest one, so a hiccup cannot
//! add lasting latency.
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

/// Pixel layouts the grader can produce for outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PixelFormat {
    None = 0,
    /// Y plane then interleaved CbCr at half resolution.
    Nv12 = 1,
    /// Packed 4:2:2, Y0 Cb Y1 Cr.
    Yuyv = 2,
}

impl PixelFormat {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Nv12,
            2 => Self::Yuyv,
            _ => Self::None,
        }
    }

    pub fn frame_size(self, width: u32, height: u32) -> usize {
        let pixels = width as usize * height as usize;
        match self {
            Self::None => 0,
            Self::Nv12 => pixels * 3 / 2,
            Self::Yuyv => pixels * 2,
        }
    }

    pub fn stride(self, width: u32) -> u32 {
        match self {
            Self::None => 0,
            Self::Nv12 => width,
            Self::Yuyv => width * 2,
        }
    }
}

#[derive(Default)]
pub struct Frame {
    pub data: Vec<u8>,
    /// A second image of the same size (the ungraded preview).
    pub extra: Vec<u8>,
    /// Look swatches (the preview path), BGRA, one after another.
    pub swatches: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Increments for every published frame.
    pub sequence: u64,
    /// Capture time on CLOCK_MONOTONIC, in nanoseconds (0 when unknown).
    pub captured: u64,
}

/// A frame slot plus the format its reader currently wants.
pub struct Slot {
    frame: Mutex<Frame>,
    format: AtomicU8,
    /// The reader took the last frame; the writer may produce another.
    taken: AtomicBool,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            frame: Mutex::new(Frame::default()),
            format: AtomicU8::new(0),
            taken: AtomicBool::new(true),
        }
    }
}

impl Slot {
    pub fn wanted(&self) -> PixelFormat {
        PixelFormat::from_u8(self.format.load(Ordering::Acquire))
    }

    pub fn want(&self, format: PixelFormat) {
        self.format.store(format as u8, Ordering::Release);
    }

    /// Whether the reader has consumed the previous frame. Writers that only
    /// need to keep up with a visible display (the preview) check this to
    /// avoid producing frames nobody draws.
    pub fn is_taken(&self) -> bool {
        self.taken.load(Ordering::Acquire)
    }

    /// Swap `frame` in, handing the previous buffer back for reuse.
    pub fn publish(&self, frame: &mut Frame) {
        if let Ok(mut slot) = self.frame.lock() {
            frame.sequence = slot.sequence + 1;
            std::mem::swap(&mut *slot, frame);
        }
        self.taken.store(false, Ordering::Release);
    }

    /// Run `read` on the newest frame if it is newer than `seen`.
    pub fn read<R>(&self, seen: u64, read: impl FnOnce(&Frame) -> R) -> Option<R> {
        let slot = self.frame.lock().ok()?;
        self.taken.store(true, Ordering::Release);
        (slot.sequence > seen).then(|| read(&slot))
    }

    /// Take ownership of the newest frame's pixels (the preview path).
    pub fn take(&self, seen: u64) -> Option<Frame> {
        let mut slot = self.frame.lock().ok()?;
        self.taken.store(true, Ordering::Release);
        if slot.sequence <= seen {
            return None;
        }
        let sequence = slot.sequence;
        let mut frame = std::mem::take(&mut *slot);
        slot.sequence = sequence;
        frame.sequence = sequence;
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_wins_and_reuses_buffers() {
        let slot = Slot::default();
        assert!(slot.is_taken());
        let mut frame = Frame {
            data: vec![1],
            ..Default::default()
        };
        slot.publish(&mut frame);
        assert!(!slot.is_taken());
        let mut frame = Frame {
            data: vec![2],
            ..Default::default()
        };
        slot.publish(&mut frame);
        assert_eq!(frame.data, [1], "previous buffer handed back");
        assert_eq!(
            slot.read(0, |f| (f.sequence, f.data.clone())),
            Some((2, vec![2]))
        );
        assert!(slot.is_taken());
        assert_eq!(slot.read(2, |_| ()), None);
        let taken = slot.take(1).unwrap();
        assert_eq!(taken.data, [2]);
        assert!(slot.take(2).is_none(), "sequence survives take");
    }

    #[test]
    fn formats() {
        assert_eq!(PixelFormat::Nv12.frame_size(1280, 720), 1_382_400);
        assert_eq!(PixelFormat::Yuyv.stride(1280), 2560);
        let slot = Slot::default();
        slot.want(PixelFormat::Yuyv);
        assert_eq!(slot.wanted(), PixelFormat::Yuyv);
    }
}
