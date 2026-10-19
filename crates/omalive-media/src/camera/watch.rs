//! Cameras come and go: docks reconnect and capture cards are plugged in.
//! Watch `/dev` for video nodes and report the camera list when it changes.
use super::v4l2::devices;
use crate::{Emit, Event};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::inotify,
    io::Errno,
    time::Timespec,
};
use std::{
    mem::MaybeUninit,
    path::Path,
    time::{Duration, Instant},
};

/// udev creates a node, then sets its owner and ACL, and one camera brings
/// several nodes. Wait for the burst to settle before probing.
const SETTLE: Duration = Duration::from_millis(500);

/// Emit [`Event::Cameras`] now and whenever the list changes.
pub fn watch(emit: Emit) {
    let _ = std::thread::Builder::new()
        .name("omalive-cameras".into())
        .spawn(move || {
            let changed = |cameras| {
                emit(Event::Cameras(cameras));
                true
            };
            if let Err(error) = follow(Path::new("/dev"), devices, changed) {
                emit(Event::Warning(format!(
                    "Cameras plugged in later won't appear until omalive restarts: {error}"
                )));
            }
        });
}

/// Call `changed` with `list()` once, then again each time a `video*` entry
/// in `dir` comes, goes or changes permissions and the list differs. Stops
/// when `changed` returns false.
fn follow<T: PartialEq + Clone>(
    dir: &Path,
    list: impl Fn() -> T,
    mut changed: impl FnMut(T) -> bool,
) -> rustix::io::Result<()> {
    use inotify::{CreateFlags, WatchFlags};
    let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
    inotify::add_watch(
        &fd,
        dir,
        WatchFlags::CREATE | WatchFlags::DELETE | WatchFlags::ATTRIB,
    )?;
    let mut buffer = [MaybeUninit::uninit(); 4096];
    let mut events = inotify::Reader::new(&fd, &mut buffer);
    let mut known = list();
    if !changed(known.clone()) {
        return Ok(());
    }
    let mut due: Option<Instant> = None;
    loop {
        let timeout = due
            .map(|due| Timespec::try_from(due.saturating_duration_since(Instant::now())))
            .transpose()
            .map_err(|_| Errno::INVAL)?;
        match poll(&mut [PollFd::new(&fd, PollFlags::IN)], timeout.as_ref()) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => return Err(error),
        }
        loop {
            match events.next() {
                // No name means the queue overflowed: assume a change.
                Ok(event) => {
                    if event
                        .file_name()
                        .is_none_or(|name| name.to_bytes().starts_with(b"video"))
                    {
                        due = Some(Instant::now() + SETTLE);
                    }
                }
                Err(Errno::INTR) => {}
                Err(Errno::WOULDBLOCK) => break,
                Err(error) => return Err(error),
            }
        }
        if due.is_some_and(|due| Instant::now() >= due) {
            due = None;
            let now = list();
            if now != known {
                known = now.clone();
                if !changed(now) {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn videos(dir: &Path) -> Vec<String> {
        // The directory goes when the test ends; the watcher then stops.
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.starts_with("video"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn plugging_and_unplugging_reports_the_new_list_once_settled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let (sender, lists) = mpsc::channel();
        let watched = path.clone();
        std::thread::spawn(move || {
            follow(
                &watched,
                || videos(&watched),
                |list| sender.send(list).is_ok(),
            )
        });
        let next = || lists.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(next().is_empty(), "the starting list");

        // A capture card: two nodes in a burst, reported together.
        std::fs::write(path.join("video1"), "").unwrap();
        std::fs::write(path.join("video2"), "").unwrap();
        std::fs::write(path.join("media0"), "").unwrap();
        let started = Instant::now();
        assert_eq!(next(), ["video1", "video2"]);
        assert!(started.elapsed() >= SETTLE);

        // Unrelated nodes and unchanged lists stay quiet.
        std::fs::write(path.join("hidraw7"), "").unwrap();
        std::fs::remove_file(path.join("media0")).unwrap();
        assert!(lists.recv_timeout(SETTLE * 2).is_err());

        std::fs::remove_file(path.join("video1")).unwrap();
        assert_eq!(next(), ["video2"]);
    }
}
