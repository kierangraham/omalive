//! The PipeWire thread.
//!
//! One `MainLoop` owns every PipeWire object: the registry view of the graph,
//! the voice filter-chain (a `pipewire -c` child), its live controls, the
//! level meter stream and the camera source stream. Other threads talk to it
//! only through [`Command`]s; nothing here polls the graph.
mod chain;
mod pods;
mod registry;
mod streams;

use crate::{
    Emit, Event, VoiceState,
    frames::{PixelFormat, Slot},
    meter::{Meter, Taps},
};
use anyhow::Result;
use chain::Chain;
use omalive_core::{Plugins, VoiceSettings, graph::Graph, voice};
use pipewire as pw;
use pw::{
    properties::properties,
    registry::GlobalObject,
    spa::{self, param::ParamType, pod::Pod},
    stream::StreamState,
    types::ObjectType,
};
use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub enum Command {
    VoiceStart(VoiceSettings, Plugins),
    VoiceStop,
    /// `node:Control` values for the running chain.
    VoiceControls(Vec<(String, f32)>),
    VoiceMute(bool),
    /// Run the level meter and taps (only while they are visible).
    Meter(bool),
    /// Run the level meter for the bar while the voice chain runs. It keeps
    /// the chain processing (about 1% of a core) so the bar always shows
    /// your level, not only during calls.
    BarMeter(bool),
    VideoStart {
        width: u32,
        height: u32,
        fps: u32,
    },
    VideoStop,
    /// A new frame is in the video slot.
    VideoFrame,
    Quit,
}

pub struct PipeWire {
    sender: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl PipeWire {
    pub fn start(emit: Emit, meter: Arc<Meter>, taps: Arc<Taps>, video: Arc<Slot>) -> Self {
        let (sender, receiver) = pw::channel::channel();
        let thread = std::thread::Builder::new()
            .name("omalive-pipewire".into())
            .spawn(move || {
                if let Err(error) = run(receiver, emit.clone(), meter, taps, video) {
                    emit(Event::Connected(false));
                    emit(Event::Error(format!(
                        "Cannot reach the PipeWire audio session: {error:#}"
                    )));
                }
            })
            .ok();
        Self { sender, thread }
    }

    pub fn sender(&self) -> pw::channel::Sender<Command> {
        self.sender.clone()
    }

    pub fn send(&self, command: Command) {
        let _ = self.sender.send(command);
    }
}

impl Drop for PipeWire {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    receiver: pw::channel::Receiver<Command>,
    emit: Emit,
    meter: Arc<Meter>,
    taps: Arc<Taps>,
    video: Arc<Slot>,
) -> Result<()> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let state = Rc::new(RefCell::new(State {
        core: core.clone(),
        registry: registry.clone(),
        graph: Graph::default(),
        globals: HashMap::new(),
        synced: false,
        inputs: Vec::new(),
        voice: None,
        pending_start: None,
        want_meter: false,
        want_bar: false,
        listeners: 0,
        meter_stream: None,
        meter,
        taps,
        tap_stream: None,
        tap_link: None,
        retired: Vec::new(),
        expiring: Vec::new(),
        video_slot: video,
        video: None,
        emit: emit.clone(),
    }));

    let sync = core.sync(0)?;
    let _core_listener = core
        .add_listener_local()
        .done({
            let state = state.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == sync {
                    let mut state = state.borrow_mut();
                    state.synced = true;
                    state.emit_inputs();
                }
            }
        })
        .error({
            let emit = emit.clone();
            move |id, _seq, res, message| {
                // Most core errors answer one request; only a broken pipe
                // means the connection itself is gone.
                if id == pw::core::PW_ID_CORE && res == -rustix::io::Errno::PIPE.raw_os_error() {
                    emit(Event::Connected(false));
                }
                emit(Event::Warning(format!("PipeWire: {message}")));
            }
        })
        .register();
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let state = state.clone();
            move |global| state.borrow_mut().global(global)
        })
        .global_remove({
            let state = state.clone();
            move |id| state.borrow_mut().global_remove(id)
        })
        .register();
    // Only start deadlines, child exits and retired proxies need time;
    // everything else is driven by registry events. One wake-up per second is negligible.
    let timer = main_loop.loop_().add_timer({
        let state = state.clone();
        move |_| state.borrow_mut().tick()
    });
    timer.update_timer(Some(Duration::from_secs(1)), Some(Duration::from_secs(1)));
    let _receiver = receiver.attach(main_loop.loop_(), {
        let state = state.clone();
        let main_loop = main_loop.downgrade();
        move |command| {
            if matches!(command, Command::Quit) {
                state.borrow_mut().shutdown();
                if let Some(main_loop) = main_loop.upgrade() {
                    main_loop.quit();
                }
            } else {
                state.borrow_mut().command(command);
            }
        }
    });
    emit(Event::Connected(true));
    main_loop.run();
    Ok(())
}

enum Phase {
    Starting { deadline: Instant },
    WaitingForMicrophone,
    Running,
}

struct Voice {
    settings: VoiceSettings,
    chain: Chain,
    phase: Phase,
    /// The microphone link and whether the registry has confirmed it.
    link: Option<(pw::link::Link, bool)>,
    capture: Option<pw::node::Node>,
    source: Option<pw::node::Node>,
    muted: bool,
}

struct State {
    core: pw::core::CoreRc,
    registry: pw::registry::RegistryRc,
    graph: Graph,
    /// Registry globals for omalive's own nodes, kept to bind proxies.
    globals: HashMap<u32, GlobalObject<pw::properties::PropertiesBox>>,
    synced: bool,
    inputs: Vec<omalive_core::Input>,
    voice: Option<Voice>,
    pending_start: Option<(VoiceSettings, Plugins)>,
    want_meter: bool,
    want_bar: bool,
    /// Apps reading the virtual microphone.
    listeners: u32,
    meter_stream: Option<streams::MeterStream>,
    meter: Arc<Meter>,
    taps: Arc<Taps>,
    /// Raw-microphone capture for the visualisations, linked by hand.
    tap_stream: Option<streams::TapStream>,
    /// The link and whether the registry has confirmed it.
    tap_link: Option<(pw::link::Link, bool)>,
    /// Link and node proxies let go of. The server may already have removed
    /// the object, and destroying a proxy before its remove_id arrives earns
    /// a core error, so each waits out a full tick before it is dropped.
    retired: Vec<Box<dyn Any>>,
    expiring: Vec<Box<dyn Any>>,
    video_slot: Arc<Slot>,
    video: Option<streams::VideoOut>,
    emit: Emit,
}

impl State {
    fn command(&mut self, command: Command) {
        match command {
            Command::VoiceStart(settings, plugins) => self.start_voice(settings, plugins),
            Command::VoiceStop => {
                self.pending_start = None;
                self.stop_voice();
                (self.emit)(Event::Voice(VoiceState::Off));
            }
            Command::VoiceControls(controls) => self.set_controls(&controls),
            Command::VoiceMute(muted) => self.mute(muted),
            Command::Meter(on) => {
                self.want_meter = on;
                self.sync_meter();
            }
            Command::BarMeter(on) => {
                self.want_bar = on;
                self.sync_meter();
            }
            Command::VideoStart { width, height, fps } => {
                self.video = None;
                match streams::video(
                    &self.core,
                    self.video_slot.clone(),
                    self.emit.clone(),
                    width,
                    height,
                    fps,
                ) {
                    Ok(video) => self.video = Some(video),
                    Err(error) => (self.emit)(Event::Warning(format!(
                        "PipeWire camera unavailable: {error:#}"
                    ))),
                }
            }
            Command::VideoStop => {
                if let Some(video) = self.video.take() {
                    let _ = video.stream.disconnect();
                }
                self.video_slot.want(PixelFormat::None);
                (self.emit)(Event::CameraViewers(false));
            }
            Command::VideoFrame => {
                if let Some(video) = &self.video
                    && matches!(video.stream.state(), StreamState::Streaming)
                {
                    let _ = video.stream.trigger_process();
                }
            }
            Command::Quit => {}
        }
    }

    // ---------------------------------------------------------------- graph

    fn global(&mut self, global: &GlobalObject<&spa::utils::dict::DictRef>) {
        let Some(props) = global.props else {
            return;
        };
        let get = |key: &str| props.get(key);
        match global.type_ {
            ObjectType::Node => {
                let node = registry::node(get);
                if node.name.starts_with("omalive_") {
                    self.globals.insert(global.id, global.to_owned());
                }
                self.graph.nodes.insert(global.id, node);
            }
            ObjectType::Port => match registry::port(global.id, get) {
                Some(port) => {
                    self.graph.ports.insert(global.id, port);
                }
                None => return,
            },
            ObjectType::Link => match registry::link(global.id, get) {
                Some(link) => {
                    self.graph.links.insert(global.id, link);
                }
                None => return,
            },
            _ => return,
        }
        self.changed();
    }

    fn global_remove(&mut self, id: u32) {
        self.globals.remove(&id);
        if !self.graph.remove(id) {
            return;
        }
        // Our own nodes vanishing means the voice child died.
        let lost = self.voice.as_ref().is_some_and(|voice| {
            !matches!(voice.phase, Phase::Starting { .. })
                && (self.graph.node_id(voice::CAPTURE_NODE).is_none()
                    || self.graph.node_id(voice::SOURCE_NODE).is_none())
        });
        if lost {
            self.fail_voice("The voice engine stopped unexpectedly. Start it again.");
        }
        self.changed();
    }

    fn changed(&mut self) {
        self.emit_inputs();
        self.advance_voice();
        let listeners = self.graph.listeners(voice::SOURCE_NODE);
        if listeners != self.listeners {
            self.listeners = listeners;
            (self.emit)(Event::VoiceListeners(listeners));
            self.sync_meter();
        }
        self.link_tap();
        if let Some((settings, plugins)) = self.pending_start.take() {
            if self.graph.node_id(voice::SOURCE_NODE).is_none() {
                self.launch(settings, plugins);
            } else {
                self.pending_start = Some((settings, plugins));
            }
        }
    }

    fn emit_inputs(&mut self) {
        if !self.synced {
            return;
        }
        let inputs = self.graph.inputs();
        if inputs != self.inputs {
            self.inputs = inputs.clone();
            (self.emit)(Event::Inputs(inputs));
        }
    }

    fn tick(&mut self) {
        self.expiring = std::mem::take(&mut self.retired);
        let Some(voice) = &mut self.voice else {
            return;
        };
        if let Ok(Some(status)) = voice.chain.child.try_wait() {
            self.fail_voice(&format!("The voice engine exited ({status})."));
        } else if let Phase::Starting { deadline } = voice.phase
            && Instant::now() > deadline
        {
            self.fail_voice("The virtual microphone did not appear within eight seconds.");
        }
    }

    // ---------------------------------------------------------------- voice

    fn fail_voice(&mut self, message: &str) {
        let detail = self
            .voice
            .as_mut()
            .map(|v| v.chain.error())
            .unwrap_or_default();
        self.stop_voice();
        (self.emit)(Event::Voice(VoiceState::Off));
        (self.emit)(Event::Error(format!("{message}{detail}")));
    }

    fn start_voice(&mut self, settings: VoiceSettings, plugins: Plugins) {
        if let Err(error) = voice::configuration(&settings, &plugins) {
            (self.emit)(Event::Error(format!("{error:#}")));
            if self.voice.is_none() {
                (self.emit)(Event::Voice(VoiceState::Off));
            }
            return;
        }
        if self.voice.is_some() {
            // Restart: wait for the old nodes to leave before the new chain
            // claims the same names.
            self.stop_voice();
            self.pending_start = Some((settings, plugins));
            (self.emit)(Event::Voice(VoiceState::Starting));
            self.changed();
        } else if self.graph.node_id(voice::SOURCE_NODE).is_some() {
            (self.emit)(Event::Error(
                "Omalive Mic is already running in another omalive instance.".into(),
            ));
            (self.emit)(Event::Voice(VoiceState::Off));
        } else {
            self.launch(settings, plugins);
        }
    }

    fn launch(&mut self, settings: VoiceSettings, plugins: Plugins) {
        match Chain::spawn(&settings, &plugins) {
            Ok(chain) => {
                self.voice = Some(Voice {
                    settings,
                    chain,
                    phase: Phase::Starting {
                        deadline: Instant::now() + Duration::from_secs(8),
                    },
                    link: None,
                    capture: None,
                    source: None,
                    muted: false,
                });
                (self.emit)(Event::Voice(VoiceState::Starting));
                self.advance_voice();
            }
            Err(error) => {
                (self.emit)(Event::Voice(VoiceState::Off));
                (self.emit)(Event::Error(format!("{error:#}")));
            }
        }
    }

    /// Link the microphone once both ends exist; relink after hotplug. The
    /// virtual microphone stays available to call apps meanwhile.
    fn advance_voice(&mut self) {
        let Some(voice) = &mut self.voice else {
            return;
        };
        let graph = &self.graph;
        let capture_input = graph
            .inputs_of(voice::CAPTURE_NODE)
            .first()
            .map(|p| (p.id, p.node));
        let source_ready = graph
            .node_id(voice::SOURCE_NODE)
            .is_some_and(|node| graph.ports.values().any(|p| p.node == node && p.output));
        let (Some((input_port, input_node)), true) = (capture_input, source_ready) else {
            return;
        };
        let microphone = graph
            .port(&voice.settings.microphone, &voice.settings.port, true)
            .map(|p| (p.id, p.node));
        let linked = microphone.is_some_and(|(port, _)| graph.linked(port, input_port));
        let Some((output_port, output_node)) = microphone else {
            if let Some((link, _)) = voice.link.take() {
                self.retired.push(Box::new(link));
            }
            if !matches!(voice.phase, Phase::WaitingForMicrophone) {
                voice.phase = Phase::WaitingForMicrophone;
                (self.emit)(Event::Voice(VoiceState::WaitingForMicrophone));
            }
            return;
        };
        match &mut voice.link {
            Some((_, seen)) if linked => *seen = true,
            // Created, not yet announced by the registry.
            Some((_, false)) => {}
            // None yet, or the server removed a confirmed link (hotplug).
            _ => {
                if let Some((link, _)) = voice.link.take() {
                    self.retired.push(Box::new(link));
                }
                let link = self.core.create_object::<pw::link::Link>(
                    "link-factory",
                    &properties! {
                        "link.output.node" => output_node.to_string(),
                        "link.output.port" => output_port.to_string(),
                        "link.input.node" => input_node.to_string(),
                        "link.input.port" => input_port.to_string(),
                    },
                );
                match link {
                    Ok(link) => voice.link = Some((link, false)),
                    Err(error) => {
                        (self.emit)(Event::Error(format!(
                            "Cannot connect the microphone: {error}"
                        )));
                        return;
                    }
                }
            }
        }
        if voice.capture.is_none() || voice.source.is_none() {
            for (id, global) in &self.globals {
                let name = self.graph.nodes.get(id).map(|n| n.name.as_str());
                if name == Some(voice::CAPTURE_NODE) && voice.capture.is_none() {
                    voice.capture = self.registry.bind(global).ok();
                } else if name == Some(voice::SOURCE_NODE) && voice.source.is_none() {
                    voice.source = self.registry.bind(global).ok();
                }
            }
        }
        if !matches!(voice.phase, Phase::Running) {
            voice.phase = Phase::Running;
            let muted = voice.muted;
            (self.emit)(Event::Voice(VoiceState::Running { muted }));
            self.sync_meter();
        }
    }

    fn stop_voice(&mut self) {
        self.retire_tap_link();
        self.meter_stream = None;
        self.tap_stream = None;
        self.meter.reset();
        if let Some(voice) = self.voice.take() {
            // The chain's nodes may already be gone if it died.
            self.retired
                .push(Box::new((voice.link, voice.capture, voice.source)));
            // Dropping the chain kills the child and removes its files.
            drop(voice.chain);
        }
    }

    fn retire_tap_link(&mut self) {
        if let Some((link, _)) = self.tap_link.take() {
            self.retired.push(Box::new(link));
        }
    }

    fn set_controls(&mut self, controls: &[(String, f32)]) {
        let Some(voice) = &mut self.voice else {
            return;
        };
        if let Some(capture) = &voice.capture {
            let bytes = pods::controls(controls);
            if let Some(pod) = Pod::from_bytes(&bytes) {
                capture.set_param(ParamType::Props, 0, pod);
            }
        }
        for (name, value) in controls {
            chain::apply_control(&mut voice.settings, name, *value);
        }
    }

    fn mute(&mut self, muted: bool) {
        let Some(voice) = &mut self.voice else {
            return;
        };
        voice.muted = muted;
        if let Some(source) = &voice.source {
            let bytes = pods::mute(muted);
            if let Some(pod) = Pod::from_bytes(&bytes) {
                source.set_param(ParamType::Props, 0, pod);
            }
        }
        if matches!(voice.phase, Phase::Running) {
            (self.emit)(Event::Voice(VoiceState::Running { muted }));
        }
    }

    /// The meter and taps keep the chain processing, so they only run while
    /// something shows them: the window (meter and taps) or the bar (meter).
    fn sync_meter(&mut self) {
        let running = self
            .voice
            .as_ref()
            .is_some_and(|voice| matches!(voice.phase, Phase::Running));
        let window = self.want_meter && running;
        let bar = self.want_bar && running;
        if !window {
            self.retire_tap_link();
            self.tap_stream = None;
        }
        if !(window || bar) {
            self.meter_stream = None;
            self.meter.reset();
            return;
        }
        if self.meter_stream.is_none() {
            match streams::meter(&self.core, self.meter.clone(), self.taps.output.clone()) {
                Ok(stream) => self.meter_stream = Some(stream),
                Err(error) => (self.emit)(Event::Warning(format!(
                    "Level meter unavailable: {error:#}"
                ))),
            }
        }
        if !window {
            return;
        }
        if self.tap_stream.is_none() {
            match streams::tap(&self.core, self.taps.input.clone()) {
                Ok(stream) => self.tap_stream = Some(stream),
                Err(error) => (self.emit)(Event::Warning(format!(
                    "Input analysis unavailable: {error:#}"
                ))),
            }
        }
        self.link_tap();
    }

    /// Link the selected microphone port to the raw-input tap.
    fn link_tap(&mut self) {
        let (Some(voice), Some(_)) = (&self.voice, &self.tap_stream) else {
            return;
        };
        let graph = &self.graph;
        let microphone = graph
            .port(&voice.settings.microphone, &voice.settings.port, true)
            .map(|p| (p.id, p.node));
        let tap = graph
            .inputs_of(streams::TAP_NODE)
            .first()
            .map(|p| (p.id, p.node));
        let (Some((output_port, output_node)), Some((tap_port, tap_node))) = (microphone, tap)
        else {
            return;
        };
        let linked = graph.linked(output_port, tap_port);
        match &mut self.tap_link {
            Some((_, seen)) if linked => {
                *seen = true;
                return;
            }
            // The server removed a confirmed link (hotplug): relink.
            Some((_, true)) => self.retire_tap_link(),
            // Created, not yet announced by the registry.
            Some(_) => return,
            None if linked => return,
            None => {}
        }
        if let Ok(link) = self.core.create_object::<pw::link::Link>(
            "link-factory",
            &properties! {
                "link.output.node" => output_node.to_string(),
                "link.output.port" => output_port.to_string(),
                "link.input.node" => tap_node.to_string(),
                "link.input.port" => tap_port.to_string(),
            },
        ) {
            self.tap_link = Some((link, false));
        }
    }

    fn shutdown(&mut self) {
        self.pending_start = None;
        self.stop_voice();
        if let Some(video) = self.video.take() {
            let _ = video.stream.disconnect();
        }
    }
}
