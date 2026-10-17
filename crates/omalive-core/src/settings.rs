//! Persisted settings, validation and voice presets.
use crate::grade::Grade;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub voice: VoiceSettings,
    pub camera: CameraSettings,
    pub app: AppSettings,
}

/// How omalive runs in the background.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Start in the background when you log in (an XDG autostart entry).
    pub start_at_login: bool,
    /// Restart voice and camera at launch if they were on.
    pub resume: bool,
    pub voice_on: bool,
    pub camera_on: bool,
    /// The one-time note that closing the window keeps omalive running.
    pub background_explained: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            start_at_login: false,
            resume: true,
            voice_on: false,
            camera_on: false,
            background_explained: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// PipeWire node name of the physical input.
    pub microphone: String,
    /// Output port on that node carrying the microphone.
    pub port: String,
    /// The preset these values came from, or empty once edited. A file
    /// without one is treated as custom, whatever the default preset is.
    #[serde(default)]
    pub preset: String,
    pub suppression: bool,
    pub vad: f32,
    pub highpass: f32,
    /// EQ gains in dB, by band (see `voice::BANDS`).
    pub low: f32,
    pub mud: f32,
    pub mid: f32,
    pub high: f32,
    pub compression: bool,
    pub threshold: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub makeup: f32,
    pub output: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        let mut settings = Self {
            microphone: String::new(),
            port: String::new(),
            preset: String::new(),
            suppression: true,
            vad: 50.,
            highpass: 80.,
            low: 0.,
            mud: 0.,
            mid: 0.,
            high: 0.,
            compression: true,
            threshold: -18.,
            ratio: 3.,
            attack: 10.,
            release: 150.,
            makeup: 3.,
            output: 0.,
        };
        settings.apply_preset(Preset::Podcast);
        settings
    }
}

/// A numeric voice control: its key, range and display unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

pub const VOICE_RANGES: [(&str, Range); 12] = [
    (
        "vad",
        Range {
            min: 0.,
            max: 99.,
            step: 1.,
        },
    ),
    (
        "highpass",
        Range {
            min: 20.,
            max: 200.,
            step: 1.,
        },
    ),
    (
        "low",
        Range {
            min: -12.,
            max: 12.,
            step: 0.5,
        },
    ),
    (
        "mud",
        Range {
            min: -12.,
            max: 12.,
            step: 0.5,
        },
    ),
    (
        "mid",
        Range {
            min: -12.,
            max: 12.,
            step: 0.5,
        },
    ),
    (
        "high",
        Range {
            min: -12.,
            max: 12.,
            step: 0.5,
        },
    ),
    (
        "threshold",
        Range {
            min: -30.,
            max: 0.,
            step: 0.5,
        },
    ),
    (
        "ratio",
        Range {
            min: 1.,
            max: 20.,
            step: 0.1,
        },
    ),
    (
        "attack",
        Range {
            min: 1.5,
            max: 400.,
            step: 0.5,
        },
    ),
    (
        "release",
        Range {
            min: 2.,
            max: 800.,
            step: 1.,
        },
    ),
    (
        "makeup",
        Range {
            min: 0.,
            max: 24.,
            step: 0.5,
        },
    ),
    (
        "output",
        Range {
            min: -24.,
            max: 6.,
            step: 0.5,
        },
    ),
];

pub fn voice_range(key: &str) -> Range {
    VOICE_RANGES
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, range)| *range)
        .unwrap_or(Range {
            min: 0.,
            max: 1.,
            step: 0.01,
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Natural,
    Podcast,
    Calls,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Natural, Preset::Podcast, Preset::Calls];
    pub fn name(self) -> &'static str {
        match self {
            Preset::Natural => "Natural",
            Preset::Podcast => "Podcast",
            Preset::Calls => "Calls",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.name() == name)
    }
}

impl VoiceSettings {
    pub fn get(&self, key: &str) -> f32 {
        match key {
            "vad" => self.vad,
            "highpass" => self.highpass,
            "low" => self.low,
            "mud" => self.mud,
            "mid" => self.mid,
            "high" => self.high,
            "threshold" => self.threshold,
            "ratio" => self.ratio,
            "attack" => self.attack,
            "release" => self.release,
            "makeup" => self.makeup,
            "output" => self.output,
            _ => 0.,
        }
    }

    /// Set a numeric control, clamped and snapped to its step. Editing any
    /// control makes the preset custom.
    pub fn set(&mut self, key: &str, value: f32) {
        let range = voice_range(key);
        let value = snap(value, range);
        let slot = match key {
            "vad" => &mut self.vad,
            "highpass" => &mut self.highpass,
            "low" => &mut self.low,
            "mud" => &mut self.mud,
            "mid" => &mut self.mid,
            "high" => &mut self.high,
            "threshold" => &mut self.threshold,
            "ratio" => &mut self.ratio,
            "attack" => &mut self.attack,
            "release" => &mut self.release,
            "makeup" => &mut self.makeup,
            "output" => &mut self.output,
            _ => return,
        };
        if *slot != value {
            *slot = value;
            self.preset.clear();
        }
    }

    /// Presets follow common podcast practice for spoken voice: an 80 Hz
    /// high-pass, a few dB cut around 300 Hz, a gentle presence lift, 3:1
    /// compression doing 3–6 dB of work, and enough makeup that the result
    /// lands near −16 LUFS. Podcast is tuned for dynamic mics like the SM58.
    pub fn apply_preset(&mut self, preset: Preset) {
        self.highpass = 80.;
        self.low = 0.;
        self.mud = 0.;
        self.mid = 0.;
        self.high = 0.;
        self.threshold = -18.;
        self.ratio = 2.;
        self.attack = 10.;
        self.release = 150.;
        self.makeup = 2.;
        self.output = 0.;
        self.vad = 70.;
        match preset {
            Preset::Natural => {}
            Preset::Podcast => {
                self.mud = -3.;
                self.mid = 1.5;
                self.high = 1.;
                self.ratio = 3.;
                self.attack = 15.;
                self.makeup = 5.;
                self.vad = 85.;
            }
            Preset::Calls => {
                self.highpass = 100.;
                self.low = -1.5;
                self.mud = -2.;
                self.mid = 2.;
                self.high = 1.;
                self.threshold = -20.;
                self.ratio = 3.;
                self.attack = 5.;
                self.release = 80.;
                self.makeup = 4.;
                self.vad = 90.;
            }
        }
        self.preset = preset.name().into();
    }

    pub fn preset(&self) -> Option<Preset> {
        Preset::from_name(&self.preset)
    }

    pub fn validate(&self, require_input: bool) -> Result<()> {
        if require_input && (self.microphone.is_empty() || self.port.is_empty()) {
            bail!("Choose your microphone and input channel");
        }
        for (key, range) in VOICE_RANGES {
            let value = self.get(key);
            if !value.is_finite() || value < range.min || value > range.max {
                bail!("{key} must be between {} and {}", range.min, range.max);
            }
        }
        Ok(())
    }

    /// Settings that change the filter graph's shape or wiring. Anything else
    /// can be applied to a running chain without interrupting audio.
    pub fn structure(&self) -> (&str, &str, bool, bool) {
        (
            &self.microphone,
            &self.port,
            self.suppression,
            self.compression,
        )
    }
}

fn snap(value: f32, range: Range) -> f32 {
    if !value.is_finite() {
        return range.min;
    }
    let stepped = range.min + ((value - range.min) / range.step).round() * range.step;
    // Remove float noise so values print and compare cleanly.
    ((stepped * 1000.).round() / 1000.).clamp(range.min, range.max)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    /// Stable identity of the source camera (V4L2 bus info), empty for the first camera.
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub grade: Grade,
    /// File name inside the LUT library, empty for none.
    pub lut: String,
    pub lut_strength: f32,
    pub bypass: bool,
    /// Capture only while an app is using Omalive Camera or the preview is shown.
    pub on_demand: bool,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            device: String::new(),
            width: 1280,
            height: 720,
            grade: Grade::default(),
            lut: String::new(),
            lut_strength: 1.,
            bypass: false,
            on_demand: true,
        }
    }
}

impl CameraSettings {
    pub fn validate(&self) -> Result<()> {
        if self.width < 160 || self.height < 120 || self.width > 4096 || self.height > 4096 {
            bail!("Unsupported camera size {}×{}", self.width, self.height);
        }
        if !(0. ..=1.).contains(&self.lut_strength) {
            bail!("LUT strength must be between 0 and 100%");
        }
        if self.lut.contains('/') || self.lut.starts_with('.') && !self.lut.is_empty() {
            bail!("LUT must be a file in the LUT library");
        }
        self.grade.validate()
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("omalive")
}

impl Settings {
    /// Read settings, migrating the original voice-only file when needed.
    /// Invalid values fall back to defaults field by field rather than failing.
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        let path = dir.join("settings.json");
        let legacy = dir.join("voice.json");
        let read = |path: &Path| -> Result<serde_json::Value> {
            let text = std::fs::read_to_string(path)?;
            Ok(serde_json::from_str(&text)?)
        };
        // Only a preset the file names counts: a missing field takes the
        // default preset's name without its values having been saved.
        let mut named_preset = None;
        let result = if path.exists() {
            read(&path).and_then(|value| {
                named_preset = value["voice"]["preset"]
                    .as_str()
                    .and_then(Preset::from_name);
                Ok(serde_json::from_value::<Settings>(value)?)
            })
        } else if legacy.exists() {
            read(&legacy).and_then(|value| {
                Ok(Settings {
                    voice: serde_json::from_value(value)?,
                    ..Default::default()
                })
            })
        } else {
            return (Self::default(), None);
        };
        match result {
            Ok(mut settings) => {
                // An unedited preset follows its current tuning, so improved
                // presets reach people who chose them.
                if let Some(preset) = named_preset {
                    settings.voice.apply_preset(preset);
                }
                let mut warning = None;
                if let Err(error) = settings.voice.validate(false) {
                    warning = Some(format!("Voice settings were reset: {error}"));
                    settings.voice = VoiceSettings {
                        microphone: settings.voice.microphone,
                        port: settings.voice.port,
                        ..Default::default()
                    };
                }
                if let Err(error) = settings.camera.validate() {
                    warning = Some(format!("Camera settings were reset: {error}"));
                    settings.camera = CameraSettings::default();
                }
                (settings, warning)
            }
            Err(error) => (
                Self::default(),
                Some(format!("Saved settings could not be read: {error}")),
            ),
        }
    }

    /// Atomically replace the settings file.
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).with_context(|| format!("Creating {}", dir.display()))?;
        let temporary = dir.join(format!(".settings-{}.json", std::process::id()));
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, dir.join("settings.json")).inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_snaps_clamps_and_clears_preset() {
        let mut voice = VoiceSettings::default();
        assert_eq!(voice.preset(), Some(Preset::Podcast), "the default");
        voice.set("low", 3.26);
        assert_eq!(voice.low, 3.5);
        assert!(voice.preset.is_empty());
        voice.set("low", 99.);
        assert_eq!(voice.low, 12.);
        voice.set("ratio", 2.94);
        assert_eq!(voice.ratio, 2.9);
        voice.set("low", f32::NAN);
        assert_eq!(voice.low, -12.);
    }

    #[test]
    fn unchanged_value_keeps_preset() {
        let mut voice = VoiceSettings::default();
        voice.apply_preset(Preset::Podcast);
        voice.set("mud", -3.);
        assert_eq!(voice.preset(), Some(Preset::Podcast));
    }

    #[test]
    fn input_required_only_for_start() {
        let voice = VoiceSettings::default();
        assert!(voice.validate(false).is_ok());
        assert!(voice.validate(true).is_err());
    }

    #[test]
    fn invalid_values_rejected() {
        for value in [f32::NAN, f32::INFINITY, 13., -13.] {
            let voice = VoiceSettings {
                low: value,
                ..Default::default()
            };
            assert!(voice.validate(false).is_err(), "{value}");
        }
    }

    #[test]
    fn round_trip_and_legacy_migration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("voice.json"),
            r#"{"microphone":"interface","port":"capture_FL","low":2,"compression":false,"unknown":1}"#,
        )
        .unwrap();
        let (settings, warning) = Settings::load(dir.path());
        assert!(warning.is_none());
        assert_eq!(settings.voice.microphone, "interface");
        assert_eq!(settings.voice.low, 2.);
        assert!(!settings.voice.compression);
        settings.save(dir.path()).unwrap();
        let (again, _) = Settings::load(dir.path());
        assert_eq!(again, settings);
    }

    #[test]
    fn an_unedited_preset_takes_its_current_tuning() {
        let dir = tempfile::tempdir().unwrap();
        let old =
            r#"{"voice":{"preset":"Podcast","low":2,"mid":-1,"high":1.5,"suppression":false}}"#;
        std::fs::write(dir.path().join("settings.json"), old).unwrap();
        let (settings, _) = Settings::load(dir.path());
        let mut podcast = VoiceSettings::default();
        podcast.apply_preset(Preset::Podcast);
        assert_eq!(
            (settings.voice.mud, settings.voice.mid),
            (podcast.mud, podcast.mid)
        );
        assert!(
            !settings.voice.suppression,
            "switches are the user's, not the preset's"
        );
        // Custom settings are left alone.
        let custom = r#"{"voice":{"preset":"","mid":-1}}"#;
        std::fs::write(dir.path().join("settings.json"), custom).unwrap();
        assert_eq!(Settings::load(dir.path()).0.voice.mid, -1.);
    }

    #[test]
    fn invalid_file_falls_back_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"voice":{"microphone":"m","low":50}}"#,
        )
        .unwrap();
        let (settings, warning) = Settings::load(dir.path());
        assert!(warning.unwrap().contains("Voice settings were reset"));
        assert_eq!(settings.voice.microphone, "m");
        assert_eq!(settings.voice.low, 0.);
    }

    #[test]
    fn lut_names_cannot_escape_library() {
        let camera = CameraSettings {
            lut: "../x.cube".into(),
            ..Default::default()
        };
        assert!(camera.validate().is_err());
    }
}
