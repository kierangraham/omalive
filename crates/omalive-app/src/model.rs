//! The application's state and decisions, free of GPUI.
//!
//! Views read a [`Model`] and call its methods for user actions. Each method
//! updates state and returns [`Effect`]s for the engine to carry out, so
//! every behaviour here is testable without a window or any devices.
use omalive_core::{
    Control, Cube, Input, LutEntry, Plugins, Preset, Settings, VoiceSettings, cube,
    looks::{self, GradePreset},
};
use omalive_media::{
    CameraDevice, CameraState, Event, Look, SourceSpec, Stats, VoiceState,
    camera::synthetic::Picture,
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Voice,
    Camera,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Good,
    Warning,
    Bad,
}

/// Work for the engine, as data.
#[derive(Clone, Debug)]
pub enum Effect {
    StartVoice(VoiceSettings, Plugins),
    StopVoice,
    Controls(Vec<(String, f32)>),
    Mute(bool),
    StartCamera {
        source: SourceSpec,
        look: Look,
        on_demand: bool,
    },
    StopCamera,
    Look(Look),
    OnDemand(bool),
    /// Persist settings (the UI debounces this).
    Save,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub text: String,
    pub tone: Tone,
}

pub struct Model {
    pub settings: Settings,
    pub plugins: Plugins,
    pub page: Page,
    /// `None` while connecting to PipeWire.
    pub connected: Option<bool>,
    pub inputs: Vec<Input>,
    pub voice: VoiceState,
    /// Apps recording Omalive Mic.
    pub listeners: u32,
    /// What the running chain was started or last updated with.
    active_voice: Option<VoiceSettings>,
    pub cameras: Vec<CameraDevice>,
    pub loopback: Option<String>,
    pub camera: CameraState,
    /// Capturing (false during on-demand standby).
    pub camera_active: bool,
    /// A PipeWire app is streaming Omalive Camera.
    pub viewers: bool,
    pub stats: Option<Stats>,
    pub luts: Vec<LutEntry>,
    pub lut_dir: PathBuf,
    cube: Option<Arc<Cube>>,
    /// Parsed LUTs by file, for thumbnails.
    cubes: HashMap<String, Option<Arc<Cube>>>,
    pub message: Option<Message>,
    /// What the demo camera pans across instead of colour bars.
    pub demo_picture: Option<Arc<Picture>>,
}

impl Model {
    pub fn new(settings: Settings, plugins: Plugins, lut_dir: PathBuf) -> Self {
        let mut model = Self {
            settings,
            plugins,
            page: Page::Voice,
            connected: None,
            inputs: Vec::new(),
            voice: VoiceState::Off,
            listeners: 0,
            active_voice: None,
            cameras: Vec::new(),
            loopback: None,
            camera: CameraState::Off,
            camera_active: false,
            viewers: false,
            stats: None,
            luts: Vec::new(),
            lut_dir,
            cube: None,
            cubes: HashMap::new(),
            message: None,
            demo_picture: None,
        };
        model.refresh_luts();
        model.load_cube();
        model
    }

    pub fn say(&mut self, text: impl Into<String>, tone: Tone) {
        self.message = Some(Message {
            text: text.into(),
            tone,
        });
    }

    // ------------------------------------------------------------ events

    pub fn event(&mut self, event: Event) {
        match event {
            Event::Connected(connected) => self.connected = Some(connected),
            Event::Inputs(inputs) => {
                self.inputs = inputs;
                // A single input needs no decision.
                if self.settings.voice.microphone.is_empty() && self.inputs.len() == 1 {
                    let name = self.inputs[0].name.clone();
                    self.select_input(&name);
                }
            }
            Event::Voice(state) => {
                match &state {
                    VoiceState::Off => self.active_voice = None,
                    VoiceState::Running { muted: false } if self.voice == VoiceState::Starting => {
                        self.say("Omalive Mic is live. Choose it as the microphone in your call app.", Tone::Good);
                    }
                    VoiceState::WaitingForMicrophone => self.say(
                        "Waiting for your microphone. Omalive Mic stays available and reconnects when it returns.",
                        Tone::Warning,
                    ),
                    _ => {}
                }
                self.voice = state;
            }
            Event::Camera(state) => {
                match &state {
                    CameraState::Failed(error) => self.say(error.clone(), Tone::Bad),
                    CameraState::Running { loopback, .. } => self.loopback = loopback.clone(),
                    CameraState::Off => {
                        self.stats = None;
                        self.camera_active = false;
                        self.viewers = false;
                    }
                    CameraState::Starting => {}
                }
                self.camera = state;
            }
            Event::CameraActive(active) => {
                self.camera_active = active;
                if !active {
                    self.stats = None;
                }
            }
            Event::CameraViewers(viewers) => self.viewers = viewers,
            Event::VoiceListeners(listeners) => self.listeners = listeners,
            Event::CameraStats(stats) => self.stats = Some(stats),
            Event::Preview => {}
            Event::Error(text) => self.say(text, Tone::Bad),
            Event::Warning(text) => self.say(text, Tone::Warning),
        }
    }

    // ------------------------------------------------------------ voice

    pub fn voice_running(&self) -> bool {
        matches!(
            self.voice,
            VoiceState::Running { .. } | VoiceState::WaitingForMicrophone
        )
    }

    pub fn voice_busy(&self) -> bool {
        self.voice == VoiceState::Starting
    }

    pub fn muted(&self) -> bool {
        matches!(self.voice, VoiceState::Running { muted: true })
    }

    pub fn selected_input(&self) -> Option<&Input> {
        self.inputs
            .iter()
            .find(|input| input.name == self.settings.voice.microphone)
    }

    /// The settings the chain should run with: effects whose plugin is
    /// missing are off rather than failing the start.
    pub fn effective_voice(&self) -> VoiceSettings {
        let mut voice = self.settings.voice.clone();
        voice.suppression &= self.plugins.noise.is_some();
        voice.compression &= self.plugins.compressor.is_some();
        voice
    }

    /// Why the voice chain cannot start, if it cannot.
    pub fn voice_blocker(&self) -> Option<&'static str> {
        if self.connected == Some(false) {
            Some("PipeWire is unavailable")
        } else if self.settings.voice.microphone.is_empty() {
            Some("Choose a microphone first")
        } else if self.selected_input().is_none() && self.connected == Some(true) {
            Some("That microphone is not connected")
        } else {
            None
        }
    }

    /// The chain is running with a different input or set of effects than
    /// the settings describe; applying needs a brief restart.
    pub fn restart_needed(&self) -> bool {
        self.voice_running()
            && self
                .active_voice
                .as_ref()
                .is_some_and(|active| active.structure() != self.effective_voice().structure())
    }

    pub fn toggle_voice(&mut self) -> Vec<Effect> {
        if self.voice_running() || self.voice_busy() {
            self.say("Voice processing stopped.", Tone::Neutral);
            vec![Effect::StopVoice]
        } else {
            self.start_voice()
        }
    }

    pub fn start_voice(&mut self) -> Vec<Effect> {
        if let Some(reason) = self.voice_blocker() {
            self.say(reason, Tone::Warning);
            return Vec::new();
        }
        let voice = self.effective_voice();
        self.active_voice = Some(voice.clone());
        self.voice = VoiceState::Starting;
        vec![Effect::StartVoice(voice, self.plugins.clone())]
    }

    pub fn toggle_mute(&mut self) -> Vec<Effect> {
        match self.voice {
            VoiceState::Running { muted } => vec![Effect::Mute(!muted)],
            _ => Vec::new(),
        }
    }

    /// A numeric control changed. Running chains update live.
    pub fn set_voice(&mut self, key: &str, value: f32) -> Vec<Effect> {
        let before = self.settings.voice.get(key);
        self.settings.voice.set(key, value);
        if self.settings.voice.get(key) == before {
            return Vec::new();
        }
        let mut effects = self.live_voice_update();
        effects.push(Effect::Save);
        effects
    }

    pub fn apply_preset(&mut self, preset: Preset) -> Vec<Effect> {
        self.settings.voice.apply_preset(preset);
        let mut effects = self.live_voice_update();
        effects.push(Effect::Save);
        effects
    }

    fn live_voice_update(&mut self) -> Vec<Effect> {
        let desired = self.effective_voice();
        let running = self.voice_running();
        match &mut self.active_voice {
            Some(active) if running && active.structure() == desired.structure() => {
                let controls = omalive_core::voice::changed_controls(active, &desired);
                *active = desired;
                if controls.is_empty() {
                    Vec::new()
                } else {
                    vec![Effect::Controls(controls)]
                }
            }
            _ => Vec::new(),
        }
    }

    pub fn set_suppression(&mut self, on: bool) -> Vec<Effect> {
        self.settings.voice.suppression = on;
        vec![Effect::Save]
    }

    pub fn set_compression(&mut self, on: bool) -> Vec<Effect> {
        self.settings.voice.compression = on;
        vec![Effect::Save]
    }

    pub fn select_input(&mut self, name: &str) -> Vec<Effect> {
        if self.settings.voice.microphone == name {
            return Vec::new();
        }
        self.settings.voice.microphone = name.into();
        self.settings.voice.port = self
            .selected_input()
            .and_then(|input| input.ports.first())
            .map(|port| port.name.clone())
            .unwrap_or_default();
        vec![Effect::Save]
    }

    pub fn select_port(&mut self, port: &str) -> Vec<Effect> {
        if self.settings.voice.port == port {
            return Vec::new();
        }
        self.settings.voice.port = port.into();
        vec![Effect::Save]
    }

    pub fn voice_status(&self) -> (&'static str, Tone) {
        match self.voice {
            VoiceState::Off => ("Off", Tone::Neutral),
            VoiceState::Starting => ("Starting", Tone::Neutral),
            VoiceState::WaitingForMicrophone => ("No input", Tone::Warning),
            VoiceState::Running { muted: true } => ("Muted", Tone::Bad),
            VoiceState::Running { muted: false } => ("Live", Tone::Good),
        }
    }

    // ------------------------------------------------------------ camera

    pub fn camera_running(&self) -> bool {
        matches!(
            self.camera,
            CameraState::Running { .. } | CameraState::Starting
        )
    }

    pub fn selected_camera(&self) -> Option<&CameraDevice> {
        self.cameras
            .iter()
            .find(|camera| camera.id == self.settings.camera.device)
            .or_else(|| self.cameras.first())
    }

    /// The configured size if the camera offers it, else the closest to 720p.
    pub fn camera_size(&self) -> Option<(u32, u32)> {
        let camera = self.selected_camera()?;
        let wanted = (self.settings.camera.width, self.settings.camera.height);
        if camera.sizes.contains(&wanted) {
            return Some(wanted);
        }
        camera
            .sizes
            .iter()
            .copied()
            .min_by_key(|(w, h)| (*w as i64 * *h as i64 - 1280 * 720).abs())
    }

    pub fn look(&self) -> Look {
        let camera = &self.settings.camera;
        Look {
            grade: camera.grade,
            cube: self.cube.clone(),
            strength: camera.lut_strength,
            bypass: camera.bypass,
        }
    }

    fn look_changed(&self) -> Vec<Effect> {
        let mut effects = vec![Effect::Save];
        if self.camera_running() {
            effects.insert(0, Effect::Look(self.look()));
        }
        effects
    }

    pub fn toggle_camera(&mut self, demo: bool) -> Vec<Effect> {
        if self.camera_running() {
            return vec![Effect::StopCamera];
        }
        let source = if demo {
            SourceSpec::Synthetic {
                width: 1280,
                height: 720,
                fps: 30,
                picture: self.demo_picture.clone(),
            }
        } else {
            match (self.selected_camera(), self.camera_size()) {
                (Some(camera), Some(size)) => SourceSpec::Device(camera.clone(), size),
                _ => {
                    self.say(
                        "No camera with a supported format was found.",
                        Tone::Warning,
                    );
                    return Vec::new();
                }
            }
        };
        self.camera = CameraState::Starting;
        vec![Effect::StartCamera {
            source,
            look: self.look(),
            on_demand: self.settings.camera.on_demand,
        }]
    }

    pub fn set_grade(&mut self, control: Control, value: f32) -> Vec<Effect> {
        let before = self.settings.camera.grade.get(control);
        self.settings.camera.grade.set(control, value);
        if self.settings.camera.grade.get(control) == before {
            return Vec::new();
        }
        self.look_changed()
    }

    pub fn apply_grade_preset(&mut self, preset: &GradePreset) -> Vec<Effect> {
        if self.settings.camera.grade == preset.grade {
            return Vec::new();
        }
        self.settings.camera.grade = preset.grade;
        self.look_changed()
    }

    /// Whether some of omalive's own starter looks are not in the library.
    pub fn starter_looks_missing(&self) -> bool {
        looks::STARTER_LOOKS.iter().any(|look| {
            !self
                .luts
                .iter()
                .any(|lut| lut.file == format!("{}.cube", look.file))
        })
    }

    pub fn install_starter_looks(&mut self) {
        match looks::install_starter_looks(&self.lut_dir) {
            Ok(added) => {
                self.refresh_luts();
                self.say(
                    format!(
                        "Added {added} looks to your library. Pick one, then set its strength."
                    ),
                    Tone::Good,
                );
            }
            Err(error) => self.say(format!("{error:#}"), Tone::Bad),
        }
    }

    pub fn reset_grade(&mut self) -> Vec<Effect> {
        self.settings.camera.grade = Default::default();
        self.look_changed()
    }

    pub fn toggle_compare(&mut self) -> Vec<Effect> {
        self.settings.camera.bypass = !self.settings.camera.bypass;
        self.look_changed()
    }

    pub fn set_strength(&mut self, strength: f32) -> Vec<Effect> {
        let strength = (strength.clamp(0., 1.) * 100.).round() / 100.;
        if strength == self.settings.camera.lut_strength {
            return Vec::new();
        }
        self.settings.camera.lut_strength = strength;
        self.look_changed()
    }

    /// Choose a LUT from the library, or none with an empty name.
    pub fn select_lut(&mut self, file: &str) -> Vec<Effect> {
        if self.settings.camera.lut == file {
            return Vec::new();
        }
        self.settings.camera.lut = file.into();
        self.load_cube();
        self.look_changed()
    }

    pub fn set_on_demand(&mut self, on: bool) -> Vec<Effect> {
        self.settings.camera.on_demand = on;
        let mut effects = vec![Effect::Save];
        if self.camera_running() {
            effects.insert(0, Effect::OnDemand(on));
        }
        effects
    }

    /// Changing the source or size restarts a running camera.
    pub fn select_camera(&mut self, id: &str, demo: bool) -> Vec<Effect> {
        if self.settings.camera.device == id {
            return Vec::new();
        }
        self.settings.camera.device = id.into();
        self.restart_camera(demo)
    }

    pub fn select_size(&mut self, size: (u32, u32), demo: bool) -> Vec<Effect> {
        if (self.settings.camera.width, self.settings.camera.height) == size {
            return Vec::new();
        }
        (self.settings.camera.width, self.settings.camera.height) = size;
        self.restart_camera(demo)
    }

    fn restart_camera(&mut self, demo: bool) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.camera_running() {
            effects.push(Effect::StopCamera);
            self.camera = CameraState::Off;
            effects.extend(self.toggle_camera(demo));
        }
        effects.push(Effect::Save);
        effects
    }

    pub fn refresh_luts(&mut self) {
        self.luts = cube::library(&self.lut_dir);
        self.cubes.clear();
    }

    /// A library LUT, parsed once and cached (unreadable files are skipped).
    pub fn cube_for(&mut self, file: &str) -> Option<Arc<Cube>> {
        let dir = &self.lut_dir;
        self.cubes
            .entry(file.to_string())
            .or_insert_with(|| Cube::read(&dir.join(file)).ok().map(Arc::new))
            .clone()
    }

    fn load_cube(&mut self) {
        self.cube = None;
        let file = self.settings.camera.lut.clone();
        if file.is_empty() {
            return;
        }
        match Cube::read(&self.lut_dir.join(&file)) {
            Ok(cube) => self.cube = Some(Arc::new(cube)),
            Err(error) => {
                self.settings.camera.lut.clear();
                self.say(format!("{error:#}"), Tone::Bad);
            }
        }
    }

    /// Import a LUT file into the library and select it.
    /// Import `.cube` files and Hald CLUT images (PNG, as film-emulation
    /// collections ship them); the last good one is selected.
    pub fn import_luts(&mut self, paths: &[std::path::PathBuf]) -> Vec<Effect> {
        let mut imported = Vec::new();
        let mut failed = Vec::new();
        for path in paths {
            match import_one(path, &self.lut_dir) {
                Ok(file) => imported.push(file),
                Err(error) => failed.push(format!("{error:#}")),
            }
        }
        self.refresh_luts();
        let mut effects = Vec::new();
        if let Some(file) = imported.last() {
            self.settings.camera.lut.clear();
            effects = self.select_lut(file);
        }
        match (imported.len(), failed.first()) {
            (0, Some(error)) => self.say(error.clone(), Tone::Bad),
            (1, None) => self.say(format!("Imported {}.", imported[0]), Tone::Good),
            (n, None) => self.say(format!("Imported {n} looks."), Tone::Good),
            (n, Some(error)) => self.say(
                format!("Imported {n} looks; {} failed: {error}", failed.len()),
                Tone::Warning,
            ),
        }
        effects
    }

    pub fn camera_status(&self) -> (&'static str, Tone) {
        match &self.camera {
            CameraState::Off => ("Off", Tone::Neutral),
            CameraState::Starting => ("Starting", Tone::Neutral),
            CameraState::Failed(_) => ("Failed", Tone::Bad),
            CameraState::Running { .. } if self.viewers => ("In use", Tone::Good),
            CameraState::Running { .. } if !self.camera_active => ("Standby", Tone::Neutral),
            CameraState::Running { .. } => ("Ready", Tone::Good),
        }
    }
}

/// Import one LUT file into the library, converting Hald CLUT images
/// (resampled to 33³).
pub fn import_one(path: &std::path::Path, dir: &std::path::Path) -> anyhow::Result<String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if extension != "png" {
        return cube::import(path, dir);
    }
    let image = image::open(path)
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?
        .into_rgb8();
    let mut cube = Cube::from_hald(image.width(), image.height(), image.as_raw())?;
    if cube.size > omalive_core::grade::BAKE_SIZE {
        cube = cube.resample(omalive_core::grade::BAKE_SIZE);
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.starts_with('.'))
        .ok_or_else(|| anyhow::anyhow!("{} has no usable name", path.display()))?;
    cube.title = stem.replace('_', " ");
    std::fs::create_dir_all(dir)?;
    let file = format!("{stem}.cube");
    std::fs::write(dir.join(&file), cube.to_text())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use omalive_core::Channel;

    fn input(name: &str) -> Input {
        Input {
            name: name.into(),
            description: format!("{name} description"),
            ports: vec![
                Channel {
                    name: "capture_FL".into(),
                    label: "FL".into(),
                },
                Channel {
                    name: "capture_FR".into(),
                    label: "FR".into(),
                },
            ],
        }
    }

    fn model() -> Model {
        let dir = std::env::temp_dir().join(format!("omalive-model-{}", std::process::id()));
        let mut model = Model::new(
            Settings::default(),
            Plugins {
                noise: Some("/x/rnnoise.so".into()),
                compressor: Some("/x/sc4.so".into()),
            },
            dir,
        );
        model.event(Event::Connected(true));
        model.event(Event::Inputs(vec![input("interface"), input("laptop")]));
        model
    }

    fn running(model: &mut Model) {
        model.select_input("interface");
        assert!(matches!(model.start_voice()[..], [Effect::StartVoice(..)]));
        model.event(Event::Voice(VoiceState::Running { muted: false }));
    }

    #[test]
    fn start_requires_a_microphone_and_says_why() {
        let mut model = model();
        assert_eq!(model.voice_blocker(), Some("Choose a microphone first"));
        assert!(model.start_voice().is_empty());
        assert_eq!(model.message.as_ref().unwrap().tone, Tone::Warning);
        model.select_input("interface");
        assert_eq!(
            model.settings.voice.port, "capture_FL",
            "first channel chosen"
        );
        assert!(model.voice_blocker().is_none());
    }

    #[test]
    fn single_input_is_selected_automatically() {
        let mut model = model();
        model.event(Event::Inputs(vec![input("only")]));
        assert_eq!(model.settings.voice.microphone, "only");
    }

    #[test]
    fn live_edits_send_only_changed_controls() {
        let mut model = model();
        running(&mut model);
        let effects = model.set_voice("low", 4.5);
        match &effects[..] {
            [Effect::Controls(controls), Effect::Save] => {
                assert_eq!(controls, &[("low:Gain".to_string(), 4.5)])
            }
            other => panic!("{other:?}"),
        }
        assert!(
            model.set_voice("low", 4.5).is_empty(),
            "no-op edits are free"
        );
        assert!(!model.restart_needed());
    }

    #[test]
    fn structural_changes_wait_for_apply() {
        let mut model = model();
        running(&mut model);
        assert!(matches!(model.set_suppression(false)[..], [Effect::Save]));
        assert!(model.restart_needed());
        // Live controls are not sent across a pending structure change.
        assert!(matches!(model.set_voice("mid", 2.)[..], [Effect::Save]));
        match &model.start_voice()[..] {
            [Effect::StartVoice(voice, _)] => assert!(!voice.suppression),
            other => panic!("{other:?}"),
        }
        model.event(Event::Voice(VoiceState::Running { muted: false }));
        assert!(!model.restart_needed());
    }

    #[test]
    fn missing_plugins_turn_effects_off_instead_of_failing() {
        let mut model = model();
        model.plugins = Plugins::default();
        model.select_input("interface");
        match &model.start_voice()[..] {
            [Effect::StartVoice(voice, _)] => assert!(!voice.suppression && !voice.compression),
            other => panic!("{other:?}"),
        }
        assert!(
            model.settings.voice.suppression,
            "the user's choice is kept"
        );
    }

    #[test]
    fn hald_clut_images_import_as_cubes() {
        let mut model = model();
        let dir = tempfile::tempdir().unwrap();
        model.lut_dir = dir.path().join("luts");
        // A level-2 identity Hald CLUT: 8×8 pixels, a 4³ cube.
        let mut image = image::RgbImage::new(8, 8);
        for i in 0..64u32 {
            let (r, g, b) = (i % 4, i / 4 % 4, i / 16);
            image.put_pixel(i % 8, i / 8, image::Rgb([r, g, b].map(|v| (v * 85) as u8)));
        }
        let png = dir.path().join("Kodak_Portra_160.png");
        image.save(&png).unwrap();
        let bad = dir.path().join("photo.png");
        image::RgbImage::new(10, 10).save(&bad).unwrap();
        model.import_luts(&[png, bad]);
        assert_eq!(model.settings.camera.lut, "Kodak_Portra_160.cube");
        assert_eq!(model.luts.len(), 1);
        assert!(model.message.as_ref().unwrap().text.contains("1 failed"));
        assert_eq!(model.cube_for("Kodak_Portra_160.cube").unwrap().size, 4);
    }

    #[test]
    fn grade_presets_and_starter_looks() {
        let mut model = model();
        let bright = &looks::GRADE_PRESETS[1];
        assert!(matches!(
            model.apply_grade_preset(bright)[..],
            [Effect::Save]
        ));
        assert_eq!(model.settings.camera.grade, bright.grade);
        assert!(
            model.apply_grade_preset(bright).is_empty(),
            "already applied"
        );

        let dir = tempfile::tempdir().unwrap();
        model.lut_dir = dir.path().to_path_buf();
        model.refresh_luts();
        assert!(model.starter_looks_missing());
        model.install_starter_looks();
        assert_eq!(model.luts.len(), looks::STARTER_LOOKS.len());
        assert!(!model.starter_looks_missing());
    }

    #[test]
    fn presets_update_a_running_chain_live() {
        let mut model = model();
        running(&mut model);
        let effects = model.apply_preset(Preset::Calls);
        assert!(matches!(effects[..], [Effect::Controls(_), Effect::Save]));
        assert_eq!(model.settings.voice.preset(), Some(Preset::Calls));
    }

    #[test]
    fn mute_toggles_and_status_follows() {
        let mut model = model();
        assert!(model.toggle_mute().is_empty());
        running(&mut model);
        assert!(matches!(model.toggle_mute()[..], [Effect::Mute(true)]));
        model.event(Event::Voice(VoiceState::Running { muted: true }));
        assert_eq!(model.voice_status(), ("Muted", Tone::Bad));
        assert!(matches!(model.toggle_voice()[..], [Effect::StopVoice]));
    }

    #[test]
    fn grade_edits_reach_a_running_camera_only() {
        let mut model = model();
        assert!(matches!(
            model.set_grade(Control::Exposure, 0.5)[..],
            [Effect::Save]
        ));
        let effects = model.toggle_camera(true);
        assert!(matches!(
            effects[..],
            [Effect::StartCamera {
                on_demand: true,
                ..
            }]
        ));
        match &model.set_grade(Control::Warmth, 0.3)[..] {
            [Effect::Look(look), Effect::Save] => {
                assert_eq!(look.grade.exposure, 0.5);
                assert_eq!(look.grade.warmth, 0.3);
            }
            other => panic!("{other:?}"),
        }
        match &model.toggle_compare()[..] {
            [Effect::Look(look), Effect::Save] => assert!(look.bypass),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn camera_size_prefers_setting_then_720p() {
        let mut model = model();
        model.cameras = vec![CameraDevice {
            id: "isp".into(),
            path: "/dev/video0".into(),
            name: "FaceTime HD Camera".into(),
            sizes: vec![(1920, 1080), (1280, 960), (640, 480)],
        }];
        assert_eq!(model.camera_size(), Some((1280, 960)));
        model.settings.camera.width = 1920;
        model.settings.camera.height = 1080;
        assert_eq!(model.camera_size(), Some((1920, 1080)));
    }

    #[test]
    fn camera_status_reflects_standby_and_viewers() {
        let mut model = model();
        model.event(Event::Camera(CameraState::Running {
            device: "Test".into(),
            width: 1280,
            height: 720,
            fps: 30,
            grader: "GPU".into(),
            loopback: None,
        }));
        assert_eq!(model.camera_status().0, "Standby");
        model.event(Event::CameraActive(true));
        assert_eq!(model.camera_status().0, "Ready");
        model.event(Event::CameraViewers(true));
        assert_eq!(model.camera_status().0, "In use");
    }

    #[test]
    fn luts_import_select_and_survive_bad_files() {
        let mut model = model();
        let source = model.lut_dir.with_extension("source.cube");
        std::fs::write(
            &source,
            "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap();
        model.import_luts(std::slice::from_ref(&source));
        assert_eq!(
            model.settings.camera.lut,
            source.file_name().unwrap().to_str().unwrap()
        );
        assert!(model.look().cube.is_some());
        assert!(
            model
                .luts
                .iter()
                .any(|lut| lut.file == model.settings.camera.lut)
        );
        let effects = model.select_lut("");
        assert!(matches!(effects[..], [Effect::Save]));
        assert!(model.look().cube.is_none());
        let _ = std::fs::remove_dir_all(&model.lut_dir);
        let _ = std::fs::remove_file(source);
    }
}
