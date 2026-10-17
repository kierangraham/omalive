//! End-to-end check against the real desktop session:
//! `cargo run --release -p omalive-media --example probe -- [voice] [camera] [seconds]`
//!
//! Voice: starts the chain on the first input with every available effect,
//! changes controls live, mutes, reads the meter and stops.
//! Camera: grades the first camera for a few seconds and prints statistics.
use omalive_core::{Grade, Plugins, VoiceSettings};
use omalive_media::{Event, Look, Media, VoiceState, camera};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

fn wait<T>(
    events: &mpsc::Receiver<Event>,
    timeout: Duration,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(event) => {
                if !matches!(event, Event::Preview | Event::CameraStats(_)) {
                    println!("  event: {event:?}");
                }
                if let Some(value) = pick(&event) {
                    return Some(value);
                }
            }
            Err(_) => break,
        }
    }
    None
}

fn props(node: &str) -> String {
    let output = std::process::Command::new("pw-dump")
        .arg(node)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = args.iter().all(|a| a.parse::<u64>().is_ok());
    let seconds = args.iter().find_map(|a| a.parse().ok()).unwrap_or(5);
    let (sender, events) = mpsc::channel();
    let mut media = Media::start(Arc::new(move |event| {
        let _ = sender.send(event);
    }));

    if all || args.iter().any(|a| a == "voice") {
        println!("voice:");
        let inputs = wait(&events, Duration::from_secs(5), |e| match e {
            Event::Inputs(inputs) => Some(inputs.clone()),
            _ => None,
        })
        .expect("input list");
        let input = &inputs[0];
        let plugins = Plugins::discover();
        println!("  plugins: {plugins:?}");
        let mut settings = VoiceSettings {
            microphone: input.name.clone(),
            port: input.ports[0].name.clone(),
            suppression: plugins.noise.is_some() && std::env::var_os("PROBE_NO_RNNOISE").is_none(),
            compression: plugins.compressor.is_some(),
            ..Default::default()
        };
        let started = Instant::now();
        media.start_voice(settings.clone(), plugins);
        let running = wait(&events, Duration::from_secs(10), |e| match e {
            Event::Voice(VoiceState::Running { .. }) => Some(true),
            Event::Voice(VoiceState::Off) | Event::Error(_) => Some(false),
            _ => None,
        });
        println!("  running: {running:?} after {:?}", started.elapsed());
        assert_eq!(running, Some(true));
        let before = settings.clone();
        settings.set("low", 4.5);
        settings.set("threshold", -24.);
        media.voice_controls(omalive_core::voice::changed_controls(&before, &settings));
        media.meter(std::env::var_os("PROBE_NO_METER").is_none());
        std::thread::sleep(Duration::from_millis(600));
        let dump = props("omalive_voice_capture");
        let low = dump.contains("\"low:Gain\", 4.5") || dump.contains("\"low:Gain\",\n");
        println!(
            "  live low:Gain applied: {low} (threshold -24 present: {})",
            dump.contains("-24")
        );
        println!(
            "  meter buffers {} levels {:?}",
            media.meter.buffers(),
            media.meter.levels()
        );
        println!(
            "  taps: input {} samples, output {} samples",
            media.taps.input.written(),
            media.taps.output.written()
        );
        if let Ok(hold) = std::env::var("PROBE_HOLD") {
            println!("  holding for {hold}s");
            std::thread::sleep(Duration::from_secs(hold.parse().unwrap_or(5)));
        }
        media.mute(true);
        std::thread::sleep(Duration::from_millis(400));
        let muted = props("omalive_voice").contains("\"mute\": true");
        println!("  source muted in PipeWire: {muted}");
        media.mute(false);
        media.stop_voice();
        wait(&events, Duration::from_secs(3), |e| {
            matches!(e, Event::Voice(VoiceState::Off)).then_some(())
        });
        std::thread::sleep(Duration::from_millis(500));
        println!(
            "  leftover omalive_voice nodes: {}",
            props("omalive_voice").contains("omalive_voice")
        );
    }

    if all || args.iter().any(|a| a == "camera") {
        println!("camera:");
        let devices = camera::devices();
        println!(
            "  devices: {:?}",
            devices
                .iter()
                .map(|d| (&d.name, &d.path, &d.sizes))
                .collect::<Vec<_>>()
        );
        println!("  loopback: {:?}", camera::loopback());
        // PROBE_CAMERA=/dev/videoN and PROBE_SIZE=WxH pick another camera.
        let wanted = std::env::var_os("PROBE_CAMERA").map(std::path::PathBuf::from);
        let device = devices
            .into_iter()
            .find(|d| wanted.as_ref().is_none_or(|path| d.path == *path))
            .expect("a camera");
        let size = std::env::var("PROBE_SIZE")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((1280, 720));
        let grade = Grade {
            exposure: 0.3,
            warmth: 0.2,
            contrast: 0.15,
            vibrance: 0.2,
            ..Default::default()
        };
        let look = Look {
            grade,
            strength: 1.,
            ..Default::default()
        };
        let started = Instant::now();
        look.bake();
        println!("  bake took {:?}", started.elapsed());
        media.start_camera(
            omalive_media::SourceSpec::Device(device, size),
            look,
            Some((640, 360)),
            false,
        );
        let mut previews = 0;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < deadline {
            if let Ok(event) = events.recv_timeout(Duration::from_millis(100)) {
                match event {
                    Event::Preview => {
                        if let Some(frame) = media.preview.take(0) {
                            previews += 1;
                            if previews == 30 {
                                let path = "/tmp/omalive-preview.bgra";
                                std::fs::write(path, &frame.data).unwrap();
                                let differs = frame.extra.len() == frame.data.len()
                                    && frame.extra != frame.data;
                                println!(
                                    "  original preview present and different from graded: {differs}"
                                );
                                println!(
                                    "  preview {}x{} saved to {path}",
                                    frame.width, frame.height
                                );
                            }
                        }
                    }
                    Event::CameraStats(stats) => println!("  {stats:?}"),
                    other => println!("  event: {other:?}"),
                }
            }
        }
        println!("  previews drawn: {previews}");
        media.stop_camera();
    }
}
