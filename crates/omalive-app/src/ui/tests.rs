//! Interaction tests: real views and key bindings, a recording engine.
use super::*;
use crate::{engine::Recorder, model::Effect};
use gpui_kit::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, size};
use omalive_core::{Channel, Control, Input, Settings};
use omalive_media::{CameraState, VoiceState};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Fixture<'a> {
    view: Entity<Omalive>,
    cx: &'a mut VisualTestContext,
    effects: Rc<RefCell<Vec<Effect>>>,
    meter: Rc<Cell<bool>>,
    bar: Rc<Cell<bool>>,
    preview: Rc<Cell<Option<(u32, u32)>>>,
    swatches: Rc<RefCell<Vec<bool>>>,
}

fn fixture(cx: &mut TestAppContext) -> Fixture<'_> {
    cx.update(|cx| {
        gpui_omarchy::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        bind_keys(cx);
    });
    let effects = Rc::new(RefCell::new(Vec::new()));
    let meter = Rc::new(Cell::new(false));
    let bar = Rc::new(Cell::new(false));
    let preview = Rc::new(Cell::new(None));
    let swatches = Rc::new(RefCell::new(Vec::new()));
    let recorder = Recorder {
        effects: effects.clone(),
        meter: meter.clone(),
        bar: bar.clone(),
        preview: preview.clone(),
        swatches: swatches.clone(),
    };
    let (_events, receiver) = async_channel::unbounded();
    let lut_dir = std::env::temp_dir().join("omalive-ui-test-luts");
    let (view, cx) = cx.add_window_view(move |_, cx| {
        let plugins = recorder.plugins();
        let model = Model::new(Settings::default(), plugins, lut_dir);
        let mut app = Omalive::new(model, Box::new(recorder), receiver, false, None, cx);
        app.model.event(Event::Connected(true));
        app.model.event(Event::Inputs(vec![
            input("interface", "Scarlett 2i2", &["capture_FL", "capture_FR"]),
            input("laptop", "MacBook Pro Microphone", &["capture_MONO"]),
        ]));
        app
    });
    cx.simulate_resize(size(px(1180.), px(820.)));
    draw(cx);
    Fixture {
        view,
        cx,
        effects,
        meter,
        bar,
        preview,
        swatches,
    }
}

fn input(name: &str, description: &str, ports: &[&str]) -> Input {
    Input {
        name: name.into(),
        description: description.into(),
        ports: ports
            .iter()
            .map(|p| Channel {
                name: (*p).into(),
                label: (*p).into(),
            })
            .collect(),
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"))
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let point = bounds(cx, selector).center();
    cx.simulate_click(point, Modifiers::default());
    draw(cx);
}

impl Fixture<'_> {
    fn take(&self) -> Vec<Effect> {
        std::mem::take(&mut self.effects.borrow_mut())
    }

    fn event(&mut self, event: Event) {
        self.view.update(self.cx, |app, cx| app.event(event, cx));
        draw(self.cx);
    }

    fn read<T>(&mut self, f: impl FnOnce(&Omalive) -> T) -> T {
        self.view.read_with(self.cx, |app, _| f(app))
    }

    fn start_voice(&mut self) {
        click(self.cx, "input-interface");
        self.take();
        click(self.cx, "power");
        assert!(matches!(self.take()[..], [Effect::StartVoice(..)]));
        self.event(Event::Voice(VoiceState::Running { muted: false }));
    }
}

#[gpui_kit::test]
fn number_keys_switch_pages_and_only_visible_work_runs(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.start_voice();
    assert!(
        f.meter.get(),
        "meter runs while the voice page shows a running chain"
    );
    f.cx.simulate_keystrokes("2");
    draw(f.cx);
    assert_eq!(f.read(|app| app.model.page), Page::Camera);
    assert!(!f.meter.get(), "meter stops when hidden");
    assert!(f.cx.debug_bounds("preview").is_some());
    f.cx.simulate_keystrokes("1");
    draw(f.cx);
    assert!(f.meter.get());
    assert!(f.cx.debug_bounds("spectrum").is_some());
}

#[gpui_kit::test]
fn power_is_blocked_until_a_microphone_is_chosen(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    click(f.cx, "power");
    assert!(f.take().is_empty(), "nothing starts without a microphone");
    assert!(f.read(|app| {
        app.model
            .message
            .as_ref()
            .is_some_and(|m| m.tone == Tone::Warning)
    }));
    f.start_voice();
    assert_eq!(f.read(|app| app.model.voice_status().0), "Live");
}

#[gpui_kit::test]
fn slider_keys_change_a_running_chain_live(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.start_voice();
    // Module controls sit below the fold at the default size.
    f.cx.simulate_resize(size(px(1180.), px(1200.)));
    click(f.cx, "module-eq");
    click(f.cx, "control-low");
    f.take();
    f.cx.simulate_keystrokes("home");
    draw(f.cx);
    match &f.take()[..] {
        [.., Effect::Controls(controls)] => assert_eq!(controls, &[("low:Gain".to_string(), -12.)]),
        other => panic!("{other:?}"),
    }
    f.cx.simulate_keystrokes("right");
    draw(f.cx);
    assert_eq!(f.read(|app| app.model.settings.voice.low), -11.5);
    assert!(
        f.read(|app| app.model.settings.voice.preset.is_empty()),
        "an edit leaves the preset"
    );
    assert!(f.cx.debug_bounds("preset-custom").is_some());
}

#[gpui_kit::test]
fn dragging_an_eq_node_sets_its_gain(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.start_voice();
    let graph = bounds(f.cx, "spectrum");
    // The presence node sits at 3 kHz, +1.5 dB in the default Podcast preset
    // (the graph spans ±15 dB).
    let x = graph.origin.x + graph.size.width * ((3000f32 / 20.).ln() / 1000f32.ln());
    let start = point(x, graph.origin.y + graph.size.height * 0.45);
    f.cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    f.cx.simulate_mouse_move(
        point(x, graph.origin.y + graph.size.height * 0.25),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    f.cx.simulate_mouse_up(
        point(x, graph.origin.y + graph.size.height * 0.25),
        MouseButton::Left,
        Modifiers::default(),
    );
    draw(f.cx);
    let mid = f.read(|app| app.model.settings.voice.mid);
    assert!(
        (mid - 7.5).abs() < 0.6,
        "a quarter of the way up is +7.5 dB, got {mid}"
    );
    assert_eq!(
        f.read(|app| app.module),
        Module::Eq,
        "dragging a node opens its module"
    );
}

#[gpui_kit::test]
fn effect_switches_ask_before_restarting(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.start_voice();
    click(f.cx, "module-denoise");
    assert!(f.cx.debug_bounds("apply").is_none());
    click(f.cx, "toggle-suppression");
    assert!(f.take().is_empty(), "no restart without asking");
    assert!(f.cx.debug_bounds("apply").is_some());
    f.cx.simulate_keystrokes("a");
    draw(f.cx);
    match &f.take()[..] {
        [Effect::StartVoice(voice, _)] => assert!(!voice.suppression),
        other => panic!("{other:?}"),
    }
}

#[gpui_kit::test]
fn missing_compressor_is_explained_not_offered(cx: &mut TestAppContext) {
    let f = fixture(cx);
    // The recorder reports SC4 as missing.
    click(f.cx, "module-comp");
    assert!(f.cx.debug_bounds("control-threshold").is_none());
    assert!(f.cx.debug_bounds("toggle-compression").is_none());
    assert!(f.cx.debug_bounds("module-panel").is_some());
}

#[gpui_kit::test]
fn bracket_keys_walk_the_chain(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    assert_eq!(f.read(|app| app.module), Module::Input);
    f.cx.simulate_keystrokes("]");
    f.cx.simulate_keystrokes("]");
    draw(f.cx);
    assert_eq!(f.read(|app| app.module), Module::Denoise);
    f.cx.simulate_keystrokes("[ [ [");
    draw(f.cx);
    assert_eq!(
        f.read(|app| app.module),
        Module::Output,
        "walking wraps around"
    );
    assert!(f.cx.debug_bounds("control-output").is_some());
}

#[gpui_kit::test]
fn mute_acts_only_while_running(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.cx.simulate_keystrokes("m");
    assert!(f.take().is_empty());
    f.start_voice();
    f.cx.simulate_keystrokes("m");
    assert!(matches!(f.take()[..], [Effect::Mute(true)]));
    f.event(Event::Voice(VoiceState::Running { muted: true }));
    click(f.cx, "mute");
    assert!(matches!(f.take()[..], [Effect::Mute(false)]));
}

#[gpui_kit::test]
fn camera_start_grade_balance_and_compare(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.cx.simulate_keystrokes("2");
    draw(f.cx);
    click(f.cx, "power");
    match &f.take()[..] {
        [
            Effect::StartCamera {
                source, on_demand, ..
            },
        ] => {
            assert!(*on_demand);
            assert!(matches!(
                source,
                omalive_media::SourceSpec::Device(_, (1280, 720))
            ));
        }
        other => panic!("{other:?}"),
    }
    f.event(Event::Camera(CameraState::Running {
        device: "FaceTime HD Camera".into(),
        width: 1280,
        height: 720,
        fps: 30,
        grader: "GPU".into(),
        loopback: None,
    }));
    assert_eq!(
        f.preview.get(),
        Some(PREVIEW),
        "preview requested while shown"
    );
    click(f.cx, "control-exposure");
    f.take();
    f.cx.simulate_keystrokes("end");
    draw(f.cx);
    match &f.take()[..] {
        [.., Effect::Look(look)] => assert_eq!(look.grade.exposure, Control::Exposure.range().1),
        other => panic!("{other:?}"),
    }
    // The right edge of the balance pad, half way down: full warmth, no tint.
    let pad = bounds(f.cx, "balance");
    f.cx.simulate_click(
        point(pad.right() - px(1.), pad.center().y),
        Modifiers::default(),
    );
    draw(f.cx);
    let grade = f.read(|app| app.model.settings.camera.grade);
    assert!(grade.warmth > 0.95 && grade.tint.abs() < 0.05, "{grade:?}");
    f.cx.update(|window, cx| f.view.read(cx).focus.clone().focus(window, cx));
    f.take();
    f.cx.simulate_keystrokes("b");
    draw(f.cx);
    assert!(matches!(&f.take()[..], [Effect::Look(look)] if look.bypass));
    f.cx.simulate_keystrokes("1");
    draw(f.cx);
    assert_eq!(f.preview.get(), None, "preview stops when hidden");
}

#[gpui_kit::test]
fn the_looks_carousel_renders_only_what_is_in_view(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    let dir = tempfile::tempdir().unwrap();
    omalive_core::looks::install_starter_looks(dir.path()).unwrap();
    let path = dir.path().to_path_buf();
    f.view.update(f.cx, |app, _| {
        app.model.lut_dir = path;
        app.model.refresh_luts();
    });
    f.cx.simulate_keystrokes("2");
    draw(f.cx);
    click(f.cx, "power");
    f.event(Event::Camera(CameraState::Running {
        device: "FaceTime HD Camera".into(),
        width: 1280,
        height: 720,
        fps: 30,
        grader: "GPU".into(),
        loopback: None,
    }));
    // Original and the first four of nine looks.
    assert_eq!(*f.swatches.borrow(), [false, true, true, true, true]);
    assert!(f.cx.debug_bounds("look-none").is_some());
    assert!(f.cx.debug_bounds("look-Mono.cube").is_none());
    click(f.cx, "looks-forward");
    assert_eq!(f.swatches.borrow().len(), 5);
    assert!(f.swatches.borrow().iter().all(|lut| *lut), "only looks now");
    assert!(f.cx.debug_bounds("look-none").is_none());
    // `]` past the end wraps to Original and brings it back into view.
    f.cx.update(|window, cx| f.view.read(cx).focus.clone().focus(window, cx));
    f.cx.simulate_keystrokes("[");
    draw(f.cx);
    let last = f.read(|app| app.model.settings.camera.lut.clone());
    assert_eq!(
        last, "Warm_film.cube",
        "[ from Original wraps to the last look"
    );
    assert!(f.cx.debug_bounds("look-Warm_film.cube").is_some());
    f.cx.simulate_keystrokes("]");
    draw(f.cx);
    assert_eq!(f.read(|app| app.model.settings.camera.lut.clone()), "");
    assert!(f.cx.debug_bounds("look-none").is_some());
    assert!(!f.swatches.borrow()[0], "Original has no LUT");
    f.cx.simulate_keystrokes("1");
    draw(f.cx);
    assert!(
        f.swatches.borrow().is_empty(),
        "no swatches without the preview"
    );
}

#[gpui_kit::test]
fn narrow_zoomed_light_window_stays_in_bounds(cx: &mut TestAppContext) {
    let f = fixture(cx);
    f.cx.update(|_, cx| gpui_omarchy::Theme::flexoki_light().apply(cx));
    f.view.update(f.cx, |app, cx| {
        app.zoom = 1.8;
        cx.notify();
    });
    f.cx.simulate_resize(size(px(480.), px(480.)));
    draw(f.cx);
    let right = px(480.) + px(0.5);
    for selector in ["power", "tab-voice", "tab-camera", "module-output"] {
        let b = bounds(f.cx, selector);
        assert!(b.right() <= right, "{selector} overflows: {b:?}");
    }
    f.cx.simulate_keystrokes("2");
    draw(f.cx);
    let preview = bounds(f.cx, "preview");
    assert!(preview.right() <= right, "preview overflows: {preview:?}");
}

#[gpui_kit::test]
fn theme_slots_colour_the_palette(cx: &mut TestAppContext) {
    let f = fixture(cx);
    let magenta: Hsla = gpui_kit::rgb(0xbb9af7).into();
    f.cx.update(|_, cx| {
        assert_ne!(palette(cx).mag, magenta, "derived without slots");
        cx.set_global(kit::ThemeSlots {
            magenta: Some(magenta),
            ..Default::default()
        });
        assert_eq!(palette(cx).mag, magenta);
        assert_eq!(palette(cx).signal()[2], magenta);
        assert!(palette(cx).spectrum().contains(&magenta));
    });
}

#[gpui_kit::test]
fn a_closed_window_stops_screen_work_but_not_processing(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.start_voice();
    assert!(f.meter.get());
    f.view.update(f.cx, |app, cx| app.hidden(cx));
    assert!(!f.meter.get(), "the meter only feeds the window");
    assert!(f.take().is_empty(), "voice keeps running");
    assert!(f.read(|app| app.model.voice_running()));
    // The camera page's preview also stops while hidden, and both return
    // with the window.
    f.cx.simulate_keystrokes("2");
    f.view.update(f.cx, |app, cx| {
        let effects = app.model.toggle_camera(false);
        app.run(effects, cx);
    });
    f.event(Event::Camera(CameraState::Running {
        device: "FaceTime HD Camera".into(),
        width: 1280,
        height: 720,
        fps: 30,
        grader: "GPU".into(),
        loopback: None,
    }));
    assert!(f.preview.get().is_some());
    f.view.update(f.cx, |app, cx| app.hidden(cx));
    assert_eq!(f.preview.get(), None);
    draw(f.cx);
    assert!(
        f.preview.get().is_some(),
        "drawing again means the window is back"
    );
}

#[gpui_kit::test]
fn bar_commands_drive_the_model(cx: &mut TestAppContext) {
    use crate::control::Command;
    let mut f = fixture(cx);
    f.start_voice();
    f.view
        .update(f.cx, |app, cx| app.command(Command::ToggleMute, cx));
    assert!(matches!(f.take()[..], [Effect::Mute(true)]));
    f.view
        .update(f.cx, |app, cx| app.command(Command::Clients(1), cx));
    assert!(f.bar.get(), "a connected bar asks for levels");
    f.view
        .update(f.cx, |app, cx| app.command(Command::Clients(0), cx));
    assert!(!f.bar.get());
    // Unknown looks are ignored rather than trusted as paths.
    f.view.update(f.cx, |app, cx| {
        app.command(
            Command::Look {
                file: "../../etc/passwd".into(),
            },
            cx,
        )
    });
    assert!(f.take().is_empty());
    f.view
        .update(f.cx, |app, cx| app.command(Command::ToggleVoice, cx));
    assert!(matches!(f.take()[..], [Effect::StopVoice]));
    assert!(
        !f.read(|app| app.model.settings.app.voice_on),
        "remembered for resume"
    );
}

#[gpui_kit::test]
fn the_voice_test_needs_a_live_mic_and_stops_with_it(cx: &mut TestAppContext) {
    let mut f = fixture(cx);
    f.cx.simulate_keystrokes("t");
    draw(f.cx);
    assert!(f.read(|app| matches!(app.test, test::VoiceTest::Idle)));
    assert!(f.read(|app| {
        app.model
            .message
            .as_ref()
            .unwrap()
            .text
            .contains("Start the mic")
    }));
    f.start_voice();
    click(f.cx, "test");
    assert!(f.read(|app| matches!(app.test, test::VoiceTest::Recording(_))));
    assert!(
        f.cx.debug_bounds("test").is_none(),
        "the button gives way to progress"
    );
    // Leaving the voice page stops the meter, and with it the test.
    f.cx.simulate_keystrokes("2");
    draw(f.cx);
    assert!(f.read(|app| matches!(app.test, test::VoiceTest::Idle)));
    assert!(f.read(|app| app.visuals.recording.is_none()));
}
