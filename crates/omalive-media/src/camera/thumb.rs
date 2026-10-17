//! The bar panel's camera picture, refreshed at up to the camera's frame
//! rate while the panel is open. The grader renders it on the GPU beside
//! output it produces anyway, so it never wakes the camera by itself.
use crate::grader::BAR;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Picture size in pixels.
pub const WIDTH: usize = BAR.0 as usize;
pub const HEIGHT: usize = BAR.1 as usize;
/// Most pictures per second.
const RATE: f32 = 30.;

/// The latest picture, shared between the camera thread and its reader.
#[derive(Default)]
pub struct Thumb {
    wanted: AtomicBool,
    latest: Mutex<(u64, Arc<Vec<u8>>)>,
    fresh: Condvar,
}

impl Thumb {
    pub fn want(&self, on: bool) {
        self.wanted.store(on, Ordering::Release);
    }

    pub fn wanted(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
    }

    /// The newest picture as R G B A bytes, row by row, if newer than `seen`.
    pub fn latest(&self, seen: u64) -> Option<(u64, Arc<Vec<u8>>)> {
        self.next(seen, Duration::ZERO)
    }

    /// Like [`Thumb::latest`], waiting up to `timeout` for one to arrive.
    pub fn next(&self, seen: u64, timeout: Duration) -> Option<(u64, Arc<Vec<u8>>)> {
        let latest = self.latest.lock().ok()?;
        let (latest, _) = self
            .fresh
            .wait_timeout_while(latest, timeout, |latest| latest.0 <= seen)
            .ok()?;
        (latest.0 > seen).then(|| (latest.0, latest.1.clone()))
    }

    fn publish(&self, rgba: Vec<u8>) {
        if let Ok(mut latest) = self.latest.lock() {
            latest.0 += 1;
            latest.1 = Arc::new(rgba);
        }
        self.fresh.notify_all();
    }
}

/// Rate-limits pictures on the camera thread.
#[derive(Default)]
pub struct Sampler {
    last: Option<Instant>,
}

impl Sampler {
    /// Whether a picture is wanted with this frame. The slack keeps a
    /// 30 fps camera from skipping every other frame on jitter.
    pub fn due(&self, thumb: &Thumb) -> bool {
        thumb.wanted()
            && self
                .last
                .is_none_or(|t| t.elapsed() >= Duration::from_secs_f32(0.8 / RATE))
    }

    /// Hand over a picture the grader rendered.
    pub fn publish(&mut self, thumb: &Thumb, rgba: Vec<u8>) {
        if rgba.len() != WIDTH * HEIGHT * 4 {
            return;
        }
        self.last = Some(Instant::now());
        thumb.publish(rgba);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_publish_only_when_wanted_and_rate_limited() {
        let thumb = Thumb::default();
        let mut sampler = Sampler::default();
        assert!(!sampler.due(&thumb));
        thumb.want(true);
        assert!(sampler.due(&thumb));
        sampler.publish(&thumb, vec![0; 16]);
        assert!(thumb.latest(0).is_none(), "wrong size is ignored");
        sampler.publish(&thumb, vec![7; WIDTH * HEIGHT * 4]);
        assert!(!sampler.due(&thumb), "rate limited");
        let (sequence, rgba) = thumb.latest(0).unwrap();
        assert_eq!((sequence, rgba[0]), (1, 7));
        assert!(thumb.latest(sequence).is_none());
    }

    #[test]
    fn a_reader_wakes_as_soon_as_a_picture_lands() {
        let thumb = Arc::new(Thumb::default());
        let started = Instant::now();
        assert!(thumb.next(0, Duration::from_millis(20)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(20));
        let writer = thumb.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            Sampler::default().publish(&writer, vec![1; WIDTH * HEIGHT * 4]);
        });
        let started = Instant::now();
        let (sequence, _) = thumb.next(0, Duration::from_secs(5)).unwrap();
        assert_eq!(sequence, 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
