//! The local control socket: how the Omarchy bar (and a second `omalive`
//! launch) talks to the running app.
//!
//! JSON lines over `$XDG_RUNTIME_DIR/omalive.sock`. Each client gets the
//! full status on connect and whenever it changes; while voice runs it also
//! gets mic levels at 12 Hz. A client that asks for the camera picture
//! (the bar's open panel) gets each one the camera renders, up to 30 a
//! second while it captures: raw PPM files beside the socket, announced by
//! path.
//! Clients send one command per line. With no client connected nothing
//! here does any work.
use anyhow::{Context, Result};
use omalive_media::{
    Meter,
    camera::thumb::{self, Thumb},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

/// Level updates per second while the voice chain runs.
const LEVEL_RATE: u32 = 12;

pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OMALIVE_SOCKET") {
        return path.into();
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("omalive.sock")
}

/// What a client may ask for.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Command {
    Open,
    ToggleVoice,
    ToggleMute,
    ToggleCamera,
    Bypass,
    Look {
        file: String,
    },
    /// Send this client the camera picture (while on), or stop.
    Picture {
        on: bool,
    },
    Quit,
    /// Internal: how many clients are connected (never sent by clients).
    #[serde(skip)]
    Clients(usize),
}

/// The state the bar shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Status {
    /// off · starting · no-input · live · muted
    pub voice: &'static str,
    /// Apps recording the virtual microphone.
    pub listeners: u32,
    /// off · starting · failed · standby · ready · live
    pub camera: &'static str,
    /// e.g. "1920×1080 · 30 fps"
    pub format: String,
    pub look: String,
    pub bypass: bool,
    pub window: bool,
    /// Theme colours as `#rrggbb`, so the bar matches the app.
    pub theme: Theme,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Theme {
    pub fg: String,
    pub muted: String,
    pub accent: String,
    pub cyan: String,
    pub blue: String,
    pub magenta: String,
    pub red: String,
    pub orange: String,
    pub yellow: String,
    pub green: String,
}

impl Status {
    /// Whether levels and pictures flow: the voice is running unmuted, and
    /// the camera is capturing.
    fn feeds(&self) -> (bool, bool) {
        (
            self.voice == "live",
            self.camera == "live" || self.camera == "ready",
        )
    }
}

#[derive(Default)]
struct Inner {
    clients: Vec<(u64, UnixStream)>,
    /// Clients that asked for the camera picture.
    watching: HashSet<u64>,
    next_client: u64,
    status: Option<(Status, String)>,
}

/// Shared between the app, the accept thread and the ticker.
pub struct Hub {
    inner: Mutex<Inner>,
    wake: Condvar,
    meter: Option<Arc<Meter>>,
    thumb: Option<Arc<Thumb>>,
    /// Where pictures are written.
    picture: PathBuf,
}

impl Hub {
    /// Publish a new status; clients hear about it only if it changed.
    pub fn publish(&self, status: Status) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.status.as_ref().is_some_and(|(old, _)| *old == status) {
            return;
        }
        let line = message("status", &status);
        inner.status = Some((status, line.clone()));
        broadcast(&mut inner.clients, &line);
        self.wake.notify_all();
    }

    /// What to send now: levels, and pictures for clients that watch.
    fn feeds(inner: &Inner) -> (bool, bool) {
        let (voice, camera) = inner
            .status
            .as_ref()
            .map_or((false, false), |(status, _)| status.feeds());
        (voice, camera && !inner.watching.is_empty())
    }

    /// A client starts or stops watching; the camera thread only samples
    /// pictures while someone does.
    fn watch(&self, client: u64, on: bool) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if on {
            inner.watching.insert(client);
        } else {
            inner.watching.remove(&client);
        }
        if let Some(thumb) = &self.thumb {
            thumb.want(!inner.watching.is_empty());
        }
        self.wake.notify_all();
    }

    #[cfg(test)]
    fn clients(&self) -> usize {
        self.inner.lock().map_or(0, |inner| inner.clients.len())
    }
}

fn message(kind: &str, value: &impl Serialize) -> String {
    let mut value = serde_json::to_value(value).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.insert("type".into(), kind.into());
    }
    format!("{value}\n")
}

/// Write to every client, dropping any that cannot keep up or went away.
fn broadcast(clients: &mut Vec<(u64, UnixStream)>, line: &str) {
    clients.retain_mut(|(_, client)| client.write_all(line.as_bytes()).is_ok());
}

/// The running socket; removes its files when dropped.
pub struct Server {
    path: PathBuf,
    pub hub: Arc<Hub>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(&self.hub.picture);
    }
}

impl Server {
    /// Listen at `path`. Call only while holding the instance lock: an
    /// existing socket file there is left over from a crash and replaced.
    pub fn start(
        path: &Path,
        commands: async_channel::Sender<Command>,
        meter: Option<Arc<Meter>>,
        thumb: Option<Arc<Thumb>>,
    ) -> Result<Self> {
        let _ = std::fs::remove_file(path);
        let listener =
            UnixListener::bind(path).with_context(|| format!("Listening on {}", path.display()))?;
        let hub = Arc::new(Hub {
            inner: Mutex::default(),
            wake: Condvar::new(),
            meter,
            thumb,
            picture: path.with_extension("ppm"),
        });
        let accept_hub = hub.clone();
        std::thread::Builder::new()
            .name("omalive-control".into())
            .spawn(move || accept(listener, &accept_hub, commands))?;
        let tick_hub = hub.clone();
        std::thread::Builder::new()
            .name("omalive-levels".into())
            .spawn(move || tick(&tick_hub))?;
        let picture_hub = hub.clone();
        std::thread::Builder::new()
            .name("omalive-pictures".into())
            .spawn(move || pictures(&picture_hub))?;
        Ok(Self {
            path: path.to_path_buf(),
            hub,
        })
    }
}

fn accept(listener: UnixListener, hub: &Arc<Hub>, commands: async_channel::Sender<Command>) {
    for stream in listener.incoming().flatten() {
        // A stalled client must never block the app: drop it instead.
        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
        let Ok(reader) = stream.try_clone() else {
            continue;
        };
        // A one-shot sender (`omalive` opening the window) may hang up
        // before reading anything: still read its command.
        let (id, count) = {
            let Ok(mut inner) = hub.inner.lock() else {
                return;
            };
            let id = inner.next_client;
            inner.next_client += 1;
            let mut stream = stream;
            let listening = inner
                .status
                .as_ref()
                .is_none_or(|(_, line)| stream.write_all(line.as_bytes()).is_ok());
            if listening {
                inner.clients.push((id, stream));
            }
            (id, listening.then_some(inner.clients.len()))
        };
        if let Some(count) = count {
            hub.wake.notify_all();
            let _ = commands.send_blocking(Command::Clients(count));
        }
        let (hub, commands) = (hub.clone(), commands.clone());
        let _ = std::thread::Builder::new()
            .name("omalive-client".into())
            .spawn(move || read_commands(id, reader, &hub, &commands));
    }
}

fn read_commands(
    id: u64,
    stream: UnixStream,
    hub: &Hub,
    commands: &async_channel::Sender<Command>,
) {
    for line in BufReader::new(&stream).lines() {
        let Ok(line) = line else {
            break;
        };
        match serde_json::from_str::<Command>(&line) {
            Ok(Command::Clients(_)) | Err(_) => {}
            Ok(Command::Picture { on }) => hub.watch(id, on),
            Ok(command) => {
                let _ = commands.send_blocking(command);
            }
        }
    }
    // The client hung up: forget it, so its slot and the bar feed go.
    hub.watch(id, false);
    if let Ok(mut inner) = hub.inner.lock() {
        let before = inner.clients.len();
        inner.clients.retain(|(client, _)| *client != id);
        if inner.clients.len() != before {
            let _ = commands.send_blocking(Command::Clients(inner.clients.len()));
        }
    }
}

/// Levels, only while someone is connected and the voice is live.
fn tick(hub: &Hub) {
    loop {
        {
            let Ok(mut inner) = hub.inner.lock() else {
                return;
            };
            while inner.clients.is_empty() || !Hub::feeds(&inner).0 {
                inner = match hub.wake.wait(inner) {
                    Ok(inner) => inner,
                    Err(_) => return,
                };
            }
        }
        std::thread::sleep(Duration::from_secs(1) / LEVEL_RATE);
        let Some(meter) = &hub.meter else {
            return;
        };
        let line = message(
            "level",
            &Level {
                peak: meter.take_peak(),
                rms: meter.levels().rms,
            },
        );
        // Never send a level after the status that ended it.
        if let Ok(mut inner) = hub.inner.lock()
            && Hub::feeds(&inner).0
        {
            broadcast(&mut inner.clients, &line);
        }
    }
}

/// Camera pictures for the clients watching, each as soon as it lands.
fn pictures(hub: &Hub) {
    let Some(thumb) = &hub.thumb else {
        return;
    };
    let mut seen = 0;
    let mut ppm = Vec::new();
    loop {
        {
            let Ok(mut inner) = hub.inner.lock() else {
                return;
            };
            while !Hub::feeds(&inner).1 {
                inner = match hub.wake.wait(inner) {
                    Ok(inner) => inner,
                    Err(_) => return,
                };
            }
        }
        // The timeout notices the feed stopping while no picture comes.
        let Some((sequence, rgba)) = thumb.next(seen, Duration::from_millis(250)) else {
            continue;
        };
        seen = sequence;
        if write_picture(&hub.picture, &rgba, &mut ppm).is_err() {
            continue;
        }
        let line = message(
            "picture",
            &Picture {
                path: hub.picture.display().to_string(),
                sequence,
            },
        );
        // Never send a picture after the status that ended it.
        if let Ok(mut inner) = hub.inner.lock()
            && Hub::feeds(&inner).1
        {
            let Inner {
                clients, watching, ..
            } = &mut *inner;
            clients.retain_mut(|(id, client)| {
                !watching.contains(id) || client.write_all(line.as_bytes()).is_ok()
            });
        }
    }
}

/// A binary PPM from R G B A, renamed into place so a reader never sees
/// half of one. `ppm` is reused between pictures.
fn write_picture(path: &Path, rgba: &[u8], ppm: &mut Vec<u8>) -> std::io::Result<()> {
    ppm.clear();
    ppm.extend_from_slice(format!("P6\n{} {}\n255\n", thumb::WIDTH, thumb::HEIGHT).as_bytes());
    for pixel in rgba.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    let partial = path.with_extension("ppm.part");
    std::fs::write(&partial, &ppm)?;
    std::fs::rename(partial, path)
}

#[derive(Serialize)]
struct Level {
    peak: f32,
    rms: f32,
}

#[derive(Serialize)]
struct Picture {
    path: String,
    sequence: u64,
}

/// Send one command to a running omalive. Errors when none is running.
pub fn send(command: &str) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path()).context("omalive is not running")?;
    stream.write_all(format!("{{\"cmd\":\"{command}\"}}\n").as_bytes())?;
    Ok(())
}

/// `omalive status --follow`: bridge the socket to stdout and stdin for
/// the bar, reconnecting while omalive is not running. Prints
/// `{"type":"offline"}` whenever it is unreachable.
pub fn follow() -> Result<()> {
    let path = socket_path();
    let (commands, pending) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if commands.send(line).is_err() {
                break;
            }
        }
        // Stdin closed: the bar went away. Dropping the sender ends the
        // bridge once pending commands are delivered.
    });
    let stdout = std::io::stdout();
    let mut offline_said = false;
    loop {
        let Ok(stream) = UnixStream::connect(&path) else {
            if !offline_said {
                writeln!(stdout.lock(), "{{\"type\":\"offline\"}}")?;
                offline_said = true;
            }
            // Commands sent while offline have nothing to reach.
            loop {
                match pending.try_recv() {
                    Ok(_) => {}
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        offline_said = false;
        let mut writer = stream.try_clone()?;
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let forward_done = done.clone();
        let reader = std::thread::spawn(move || -> Result<()> {
            for line in BufReader::new(stream).lines() {
                let line = line?;
                if !line.is_empty() {
                    writeln!(std::io::stdout().lock(), "{line}")?;
                }
            }
            forward_done.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        });
        while !done.load(std::sync::atomic::Ordering::Acquire) {
            match pending.recv_timeout(Duration::from_millis(250)) {
                Ok(line) => {
                    if writer.write_all(format!("{line}\n").as_bytes()).is_err() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Ok(()),
            }
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
        reader.join().ok().transpose()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(voice: &'static str, listeners: u32) -> Status {
        Status {
            voice,
            listeners,
            camera: "off",
            ..Default::default()
        }
    }

    fn read_line(reader: &mut BufReader<UnixStream>) -> serde_json::Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn commands_parse_from_json_lines() {
        let parse = |s: &str| serde_json::from_str::<Command>(s).ok();
        assert_eq!(parse(r#"{"cmd":"toggle-mute"}"#), Some(Command::ToggleMute));
        assert_eq!(
            parse(r#"{"cmd":"look","file":"Warm.cube"}"#),
            Some(Command::Look {
                file: "Warm.cube".into()
            })
        );
        assert_eq!(parse(r#"{"cmd":"clients"}"#), None);
        assert_eq!(parse(r#"{"cmd":"rm -rf"}"#), None);
    }

    #[test]
    fn clients_get_status_levels_and_send_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("omalive.sock");
        let (sender, commands) = async_channel::unbounded();
        let meter = Arc::new(Meter::default());
        let server = Server::start(&path, sender, Some(meter.clone()), None).unwrap();
        server.hub.publish(status("off", 0));

        let stream = UnixStream::connect(&path).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let first = read_line(&mut reader);
        assert_eq!(first["type"], "status");
        assert_eq!(first["voice"], "off");
        assert_eq!(commands.recv_blocking().unwrap(), Command::Clients(1));

        // Unchanged status is not resent; a change is.
        server.hub.publish(status("off", 0));
        server.hub.publish(status("live", 1));
        let second = read_line(&mut reader);
        assert_eq!(
            (second["voice"].as_str(), second["listeners"].as_u64()),
            (Some("live"), Some(1))
        );

        // On air: levels flow, carrying the loudest peak since the last one.
        let loud: Vec<u8> = 0.5f32.to_le_bytes().repeat(4);
        meter.analyse(&loud);
        let level = read_line(&mut reader);
        assert_eq!(level["type"], "level");
        assert!((level["peak"].as_f64().unwrap() - -6.02).abs() < 0.01);

        (&stream)
            .write_all(b"{\"cmd\":\"toggle-mute\"}\nnonsense\n")
            .unwrap();
        assert_eq!(commands.recv_blocking().unwrap(), Command::ToggleMute);

        drop(reader);
        drop(stream);
        assert_eq!(commands.recv_blocking().unwrap(), Command::Clients(0));
        assert_eq!(server.hub.clients(), 0);
        drop(server);
        assert!(!path.exists(), "the socket file goes with the server");
    }

    #[test]
    fn pictures_go_only_to_clients_that_watch() {
        use omalive_media::camera::thumb::Sampler;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("omalive.sock");
        let (sender, commands) = async_channel::unbounded();
        let picture = Arc::new(Thumb::default());
        let server = Server::start(&path, sender, None, Some(picture.clone())).unwrap();
        server.hub.publish(Status {
            camera: "live",
            ..status("off", 0)
        });
        let stream = UnixStream::connect(&path).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        assert_eq!(read_line(&mut reader)["camera"], "live");
        assert_eq!(commands.recv_blocking().unwrap(), Command::Clients(1));
        assert!(!picture.wanted(), "nobody watches yet");

        (&stream)
            .write_all(b"{\"cmd\":\"picture\",\"on\":true}\n")
            .unwrap();
        while !picture.wanted() {
            std::thread::sleep(Duration::from_millis(5));
        }
        // The camera thread renders one: orange everywhere.
        let frame: Vec<u8> = [255u8, 128, 0, 255].repeat(thumb::WIDTH * thumb::HEIGHT);
        Sampler::default().publish(&picture, frame);
        let message = read_line(&mut reader);
        assert_eq!(message["type"], "picture");
        let ppm = std::fs::read(message["path"].as_str().unwrap()).unwrap();
        let header = format!("P6\n{} {}\n255\n", thumb::WIDTH, thumb::HEIGHT);
        assert!(ppm.starts_with(header.as_bytes()));
        assert_eq!(&ppm[header.len()..header.len() + 3], [255, 128, 0]);
        assert_eq!(ppm.len(), header.len() + thumb::WIDTH * thumb::HEIGHT * 3);

        drop(reader);
        drop(stream);
        assert_eq!(commands.recv_blocking().unwrap(), Command::Clients(0));
        assert!(!picture.wanted(), "a client that hangs up stops watching");
        drop(server);
        assert!(!path.with_extension("ppm").exists());
    }
}
