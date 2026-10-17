//! The app outlives its window: processing keeps running in the background
//! and the bar shows it. One view entity holds all state; the window is
//! opened onto it on demand and only borrows it while open.
use crate::{
    control::{self, Command},
    ui::Omalive,
};
use gpui_kit::{
    App, Bounds, Entity, Global, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions, point,
    px, size,
};

struct Session {
    view: Entity<Omalive>,
    window: Option<WindowHandle<Omalive>>,
    demo: bool,
    /// Kept alive for the life of the app; its socket file goes with it.
    _server: Option<control::Server>,
}

impl Global for Session {}

pub fn install(view: Entity<Omalive>, demo: bool, server: Option<control::Server>, cx: &mut App) {
    cx.set_global(Session {
        view,
        window: None,
        demo,
        _server: server,
    });
    // Take the socket down with the app, so the bar sees it go at once.
    cx.on_app_quit(|cx| {
        if cx.has_global::<Session>() {
            cx.global_mut::<Session>()._server = None;
        }
        async {}
    })
    .detach();
    cx.on_window_closed(|cx, closed| {
        let Some(session) = cx.try_global::<Session>() else {
            return;
        };
        if session.window.is_none_or(|w| w.window_id() != closed) {
            return;
        }
        // The demo is a window and nothing else.
        if session.demo {
            cx.quit();
            return;
        }
        let view = session.view.clone();
        cx.global_mut::<Session>().window = None;
        view.update(cx, |app, cx| app.hidden(cx));
        explain_background(&view, cx);
    })
    .detach();
}

/// Open the window, or bring the open one forward.
pub fn show(cx: &mut App) {
    let Some((view, window)) = cx
        .try_global::<Session>()
        .map(|session| (session.view.clone(), session.window))
    else {
        return;
    };
    if let Some(window) = window
        && window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(80.), px(80.)),
                size(px(1180.), px(820.)),
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("omalive".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(480.), px(480.))),
            app_id: Some("omalive".into()),
            ..Default::default()
        },
        move |_, _| view,
    );
    match opened {
        Ok(window) => {
            cx.global_mut::<Session>().window = Some(window);
            cx.activate(true);
        }
        Err(error) => eprintln!("omalive could not open its window: {error:#}"),
    }
}

pub fn command(command: Command, cx: &mut App) {
    match command {
        Command::Open => show(cx),
        Command::Quit => cx.quit(),
        command => {
            if let Some(session) = cx.try_global::<Session>() {
                let view = session.view.clone();
                view.update(cx, |app, cx| app.command(command, cx));
            }
        }
    }
}

/// The first time the window closes, say that omalive is still running.
fn explain_background(view: &Entity<Omalive>, cx: &mut App) {
    let explained = view.update(cx, |app, cx| app.background_explained(cx));
    if !explained {
        let _ = std::process::Command::new("notify-send")
            .args([
                "--app-name=omalive",
                "--icon=omalive",
                "omalive keeps running",
                "Your mic and camera stay processed in the background. Open it from the bar; ctrl+q in the window quits.",
            ])
            .spawn();
    }
}
