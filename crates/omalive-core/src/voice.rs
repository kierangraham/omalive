//! PipeWire filter-chain description for the voice processor.
//!
//! Signal path: selected channel → high-pass → optional RNNoise → three-band EQ
//! → optional SC4 compression → output trim → −1 dBFS clamp → virtual source.
use crate::settings::VoiceSettings;
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const CAPTURE_NODE: &str = "omalive_voice_capture";
pub const SOURCE_NODE: &str = "omalive_voice";
pub const SOURCE_DESCRIPTION: &str = "Omalive Mic";
/// The graph quantum the chain asks for. RNNoise works on 480-sample (10 ms)
/// frames internally, so with suppression on a smaller quantum adds wakeups
/// (measured: 187/s instead of 100/s) without lowering latency. Without it,
/// 256 samples (5.3 ms) keeps the chain responsive.
pub fn latency(settings: &VoiceSettings) -> &'static str {
    if settings.suppression {
        "480/48000"
    } else {
        "256/48000"
    }
}
pub const GUARD: f32 = 0.891_251; // −1 dBFS

pub const RNNOISE: &str = "librnnoise_ladspa.so";
pub const SC4: &str = "sc4m_1916.so";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plugins {
    pub noise: Option<PathBuf>,
    pub compressor: Option<PathBuf>,
}

impl Plugins {
    pub fn discover() -> Self {
        Self {
            noise: find_plugin(RNNOISE),
            compressor: find_plugin(SC4),
        }
    }

    /// Check the effects the settings ask for are installed.
    pub fn require(&self, settings: &VoiceSettings) -> Result<()> {
        if settings.suppression && self.noise.is_none() {
            anyhow::bail!(
                "Noise suppression needs the RNNoise plugin (noise-suppression-for-voice). Install it or switch suppression off."
            );
        }
        if settings.compression && self.compressor.is_none() {
            anyhow::bail!(
                "The compressor needs the SC4 plugin (swh-plugins). Install it or switch compression off."
            );
        }
        Ok(())
    }
}

/// Search LADSPA_PATH, then per-user and system plugin directories.
pub fn find_plugin(file: &str) -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var("LADSPA_PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect();
    if let Some(home) = std::env::home_dir() {
        directories.push(home.join(".ladspa"));
        directories.push(home.join(".local/lib/ladspa"));
    }
    directories.extend(
        [
            "/usr/local/lib/ladspa",
            "/usr/lib/ladspa",
            "/usr/lib64/ladspa",
            "/usr/lib",
        ]
        .map(PathBuf::from),
    );
    directories
        .into_iter()
        .map(|dir| dir.join(file))
        .find(|path| path.is_file())
}

struct Stage {
    name: &'static str,
    input: &'static str,
    output: &'static str,
}

/// The filter-chain module configuration for `pipewire -c`.
pub fn configuration(settings: &VoiceSettings, plugins: &Plugins) -> Result<Value> {
    settings.validate(true)?;
    plugins.require(settings)?;
    let mut nodes = Vec::new();
    let mut stages = Vec::new();
    let mut add = |name: &'static str, label: &str, control: Value, library: Option<&Path>| {
        let mut node = json!({
            "type": if library.is_some() { "ladspa" } else { "builtin" },
            "name": name,
            "label": label,
            "control": control,
        });
        if let Some(library) = library {
            node["plugin"] = json!(library.to_string_lossy());
        }
        nodes.push(node);
        let ladspa = library.is_some();
        stages.push(Stage {
            name,
            input: if ladspa { "Input" } else { "In" },
            output: if ladspa { "Output" } else { "Out" },
        });
    };
    add(
        "rumble",
        "bq_highpass",
        json!({"Freq": settings.highpass, "Q": 0.707, "Gain": 0}),
        None,
    );
    if settings.suppression {
        add(
            "denoise",
            "noise_suppressor_mono",
            json!({"VAD Threshold (%)": settings.vad, "VAD Grace Period (ms)": 200, "Retroactive VAD Grace (ms)": 0}),
            plugins.noise.as_deref(),
        );
    }
    for band in BANDS {
        add(
            band.key,
            band.kind.label(),
            json!({"Freq": band.frequency, "Q": band.q, "Gain": settings.get(band.key)}),
            None,
        );
    }
    if settings.compression {
        add(
            "compress",
            "sc4m",
            json!({"RMS/peak": 0, "Attack time (ms)": settings.attack,
                "Release time (ms)": settings.release, "Threshold level (dB)": settings.threshold,
                "Ratio (1:n)": settings.ratio, "Knee radius (dB)": 3, "Makeup gain (dB)": settings.makeup}),
            plugins.compressor.as_deref(),
        );
    }
    add(
        "gain",
        "linear",
        json!({"Mult": gain(settings.output), "Add": 0}),
        None,
    );
    add("guard", "clamp", json!({"Min": -GUARD, "Max": GUARD}), None);
    let links: Vec<Value> = stages
        .windows(2)
        .map(|pair| {
            json!({
                "output": format!("{}:{}", pair[0].name, pair[0].output),
                "input": format!("{}:{}", pair[1].name, pair[1].input),
            })
        })
        .collect();
    let mono = json!({"audio.rate": 48000, "audio.channels": 1, "audio.position": ["MONO"]});
    let mut capture = json!({
        "node.name": CAPTURE_NODE,
        "node.autoconnect": false,
        "node.passive": true,
        "node.dont-reconnect": true,
        "node.latency": latency(settings),
        "stream.dont-remix": true,
    });
    let mut playback = json!({
        "node.name": SOURCE_NODE,
        "node.description": SOURCE_DESCRIPTION,
        "media.class": "Audio/Source",
        "node.virtual": true,
        "node.latency": latency(settings),
        "device.icon-name": "audio-input-microphone",
    });
    merge(&mut capture, &mono);
    merge(&mut playback, &mono);
    let mut args = json!({
        "node.description": SOURCE_DESCRIPTION,
        "media.name": SOURCE_DESCRIPTION,
        "filter.graph": {"nodes": nodes, "links": links, "inputs": ["rumble:In"], "outputs": ["guard:Out"]},
        "capture.props": capture,
        "playback.props": playback,
    });
    merge(&mut args, &mono);
    Ok(json!({
        "context.properties": {"core.daemon": false, "log.level": 2},
        "context.spa-libs": {"audio.convert.*": "audioconvert/libspa-audioconvert", "support.*": "support/libspa-support"},
        "context.modules": [
            {"name": "libpipewire-module-rt", "flags": ["ifexists", "nofail"]},
            {"name": "libpipewire-module-protocol-native"},
            {"name": "libpipewire-module-client-node"},
            {"name": "libpipewire-module-adapter"},
            {"name": "libpipewire-module-filter-chain", "args": args},
        ],
    }))
}

/// Render the configuration file. PipeWire's SPA-JSON reader rejects long
/// single-line module arrays ("malformed object array") while staying alive,
/// so the file must be indented.
pub fn render(configuration: &Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(configuration)?)
}

fn merge(target: &mut Value, extra: &Value) {
    if let (Some(target), Some(extra)) = (target.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
}

pub fn gain(db: f32) -> f32 {
    10f32.powf(db / 20.)
}

/// Filter controls that can be changed on a running chain, as
/// `node:Control` names accepted by the filter-chain `Props` params.
pub fn live_controls(settings: &VoiceSettings) -> Vec<(String, f32)> {
    let mut controls = vec![("rumble:Freq".to_string(), settings.highpass)];
    controls.extend(
        BANDS
            .iter()
            .map(|band| (format!("{}:Gain", band.key), settings.get(band.key))),
    );
    controls.push(("gain:Mult".to_string(), gain(settings.output)));
    if settings.suppression {
        controls.push(("denoise:VAD Threshold (%)".into(), settings.vad));
    }
    if settings.compression {
        controls.extend([
            ("compress:Threshold level (dB)".into(), settings.threshold),
            ("compress:Ratio (1:n)".into(), settings.ratio),
            ("compress:Attack time (ms)".into(), settings.attack),
            ("compress:Release time (ms)".into(), settings.release),
            ("compress:Makeup gain (dB)".into(), settings.makeup),
        ]);
    }
    controls
}

/// Controls whose values differ between two settings with the same structure.
pub fn changed_controls(before: &VoiceSettings, after: &VoiceSettings) -> Vec<(String, f32)> {
    let previous = live_controls(before);
    live_controls(after)
        .into_iter()
        .filter(|(name, value)| {
            previous
                .iter()
                .find(|(other, _)| other == name)
                .is_none_or(|(_, old)| (old - value).abs() > f32::EPSILON)
        })
        .collect()
}

/// RBJ biquad magnitude response of the EQ section in dB, for the response graph.
pub fn eq_response(settings: &VoiceSettings, frequency: f32) -> f32 {
    let filters: Vec<_> =
        std::iter::once(coefficients(Kind::HighPass, settings.highpass, 0., 0.707))
            .chain(BANDS.iter().map(|band| {
                coefficients(band.kind, band.frequency, settings.get(band.key), band.q)
            }))
            .collect();
    let w = std::f64::consts::TAU * frequency as f64 / 48000.;
    let energy = |c: [f64; 3]| {
        let re = c[0] + c[1] * w.cos() + c[2] * (2. * w).cos();
        let im = -c[1] * w.sin() - c[2] * (2. * w).sin();
        (re * re + im * im).max(1e-24)
    };
    filters
        .iter()
        .map(|(b, a)| 10. * (energy(*b) / energy(*a)).log10())
        .sum::<f64>() as f32
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    HighPass,
    LowShelf,
    Peaking,
    HighShelf,
}

impl Kind {
    /// The filter-chain builtin for this shape.
    fn label(self) -> &'static str {
        match self {
            Kind::HighPass => "bq_highpass",
            Kind::LowShelf => "bq_lowshelf",
            Kind::Peaking => "bq_peaking",
            Kind::HighShelf => "bq_highshelf",
        }
    }
}

/// One EQ band: its settings key, shape, centre and Q.
#[derive(Clone, Copy, Debug)]
pub struct Band {
    pub key: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    pub frequency: f32,
    pub q: f64,
}

/// The voice EQ, placed where spoken voice needs it: body from the low
/// shelf, a cut for mud and boxiness, presence for intelligibility, and
/// air above a dynamic mic's roll-off.
pub const BANDS: [Band; 4] = [
    Band {
        key: "low",
        name: "Body",
        kind: Kind::LowShelf,
        frequency: 150.,
        q: 0.707,
    },
    Band {
        key: "mud",
        name: "Mud",
        kind: Kind::Peaking,
        frequency: 300.,
        q: 1.2,
    },
    Band {
        key: "mid",
        name: "Presence",
        kind: Kind::Peaking,
        frequency: 3000.,
        q: 0.8,
    },
    Band {
        key: "high",
        name: "Air",
        kind: Kind::HighShelf,
        frequency: 10000.,
        q: 0.707,
    },
];

fn coefficients(kind: Kind, frequency: f32, gain: f32, q: f64) -> ([f64; 3], [f64; 3]) {
    let w = std::f64::consts::TAU * frequency as f64 / 48000.;
    let (c, alpha) = (w.cos(), w.sin() / (2. * q));
    let a = 10f64.powf(gain as f64 / 40.);
    let beta = 2. * a.sqrt() * alpha;
    match kind {
        Kind::HighPass => (
            [(1. + c) / 2., -(1. + c), (1. + c) / 2.],
            [1. + alpha, -2. * c, 1. - alpha],
        ),
        Kind::LowShelf => (
            [
                a * ((a + 1.) - (a - 1.) * c + beta),
                2. * a * ((a - 1.) - (a + 1.) * c),
                a * ((a + 1.) - (a - 1.) * c - beta),
            ],
            [
                (a + 1.) + (a - 1.) * c + beta,
                -2. * ((a - 1.) + (a + 1.) * c),
                (a + 1.) + (a - 1.) * c - beta,
            ],
        ),
        Kind::Peaking => (
            [1. + alpha * a, -2. * c, 1. - alpha * a],
            [1. + alpha / a, -2. * c, 1. - alpha / a],
        ),
        Kind::HighShelf => (
            [
                a * ((a + 1.) + (a - 1.) * c + beta),
                -2. * a * ((a - 1.) + (a + 1.) * c),
                a * ((a + 1.) + (a - 1.) * c - beta),
            ],
            [
                (a + 1.) - (a - 1.) * c + beta,
                2. * ((a - 1.) - (a + 1.) * c),
                (a + 1.) - (a - 1.) * c - beta,
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Natural preset: a flat EQ and 2:1 compression.
    fn settings() -> VoiceSettings {
        let mut voice = VoiceSettings {
            microphone: "interface".into(),
            port: "capture_FL".into(),
            ..Default::default()
        };
        voice.apply_preset(crate::Preset::Natural);
        voice
    }

    fn plugins() -> Plugins {
        Plugins {
            noise: Some("/tmp/a\"b\\c.so".into()),
            compressor: Some("/tmp/compressor.so".into()),
        }
    }

    fn graph(config: &Value) -> &Value {
        &config["context.modules"][4]["args"]["filter.graph"]
    }

    #[test]
    fn builtin_chain_and_gain() {
        let voice = VoiceSettings {
            suppression: false,
            compression: false,
            output: 6.,
            ..settings()
        };
        let config = configuration(&voice, &Plugins::default()).unwrap();
        let chain = graph(&config);
        let names: Vec<_> = chain["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["rumble", "low", "mud", "mid", "high", "gain", "guard"]
        );
        assert_eq!(chain["links"].as_array().unwrap().len(), 6);
        let mult = chain["nodes"][5]["control"]["Mult"].as_f64().unwrap();
        assert!((mult - 10f64.powf(0.3)).abs() < 1e-5);
        let args = &config["context.modules"][4]["args"];
        assert_eq!(args["capture.props"]["node.autoconnect"], false);
        assert_eq!(args["playback.props"]["media.class"], "Audio/Source");
        assert_eq!(args["playback.props"]["node.latency"], "256/48000");
        assert_eq!(args["audio.channels"], 1);
    }

    #[test]
    fn plugin_ports_and_json_paths() {
        let config = configuration(&settings(), &plugins()).unwrap();
        assert_eq!(
            config["context.modules"][4]["args"]["capture.props"]["node.latency"],
            "480/48000"
        );
        let rendered = render(&config).unwrap();
        // Indented output is what PipeWire accepts.
        assert!(rendered.contains('\n'));
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        let chain = graph(&parsed);
        assert_eq!(chain["nodes"][1]["plugin"], "/tmp/a\"b\\c.so");
        assert_eq!(chain["links"][0]["input"], "denoise:Input");
        assert_eq!(chain["links"][1]["output"], "denoise:Output");
        assert_eq!(chain["nodes"][6]["control"]["Ratio (1:n)"], 2.0);
    }

    #[test]
    fn missing_plugins_reported() {
        let none = Plugins::default();
        let error = configuration(&settings(), &none).unwrap_err().to_string();
        assert!(error.contains("RNNoise"));
        let voice = VoiceSettings {
            suppression: false,
            ..settings()
        };
        assert!(
            configuration(&voice, &none)
                .unwrap_err()
                .to_string()
                .contains("swh-plugins")
        );
    }

    #[test]
    fn live_controls_follow_structure() {
        let mut after = settings();
        after.set("low", 4.5);
        after.set("output", -6.);
        let changed = changed_controls(&settings(), &after);
        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0], ("low:Gain".into(), 4.5));
        assert!((changed[1].1 - 0.501).abs() < 0.01);
        let bare = VoiceSettings {
            suppression: false,
            compression: false,
            ..settings()
        };
        assert!(
            live_controls(&bare)
                .iter()
                .all(|(n, _)| !n.starts_with("compress"))
        );
    }

    #[test]
    fn eq_response_matches_band_gains() {
        let mut voice = settings();
        voice.set("mid", 6.);
        voice.set("highpass", 20.);
        assert!((eq_response(&voice, 3000.) - 6.).abs() < 0.3);
        assert!(eq_response(&voice, 15000.).abs() < 0.5);
        voice.set("highpass", 200.);
        assert!(eq_response(&voice, 30.) < -24.);
    }
}
