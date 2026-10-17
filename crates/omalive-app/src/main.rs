mod assets;
mod autostart;
mod control;
mod engine;
mod model;
mod session;
mod ui;
mod visuals;

use anyhow::{Context, Result};
use gpui_kit::{AppContext, QuitMode};
use omalive_core::{Plugins, Settings, cube, settings::config_dir};
use std::sync::Arc;

const HELP: &str = "omalive — voice and camera processing for calls on Omarchy

Usage: omalive [--background | --demo]
       omalive status --follow
       omalive import FILE...

--background  Start without a window; omalive keeps processing and shows
              its state in the Omarchy bar
--demo        Simulated microphone and a test-pattern camera; touches no
              devices and saves nothing (OMALIVE_DEMO_PICTURE=image.jpg
              pans the camera across a picture instead)
status --follow
              Stream omalive's state as JSON lines and pass commands from
              stdin (the bar widget's bridge)
import FILE...
              Add .cube LUTs or Hald CLUT PNGs to the looks library

Closing the window leaves omalive running; ctrl+q quits.

1 / 2   Voice / Camera page        s   start or stop the current page
m       mute Omalive Mic           b   compare with the original image
a       apply input or effect changes (restarts the chain)
ctrl+= ctrl+- ctrl+0   zoom        ctrl+q   quit";

fn main() -> Result<()> {
    let mut demo = false;
    let mut background = false;
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("status") {
        return match args.get(1).map(String::as_str) {
            Some("--follow") => control::follow(),
            _ => anyhow::bail!("Usage: omalive status --follow"),
        };
    }
    if args.first().map(String::as_str) == Some("import") {
        return import(&args[1..]);
    }
    for arg in &args {
        match arg.as_str() {
            "--demo" => demo = true,
            "--background" => background = true,
            "--version" => {
                println!("omalive {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(());
            }
            _ => anyhow::bail!("Unknown argument {arg}. Use --help"),
        }
    }
    // One instance owns the devices; launching again opens its window.
    let _lock = if demo {
        None
    } else {
        match single_instance()? {
            Some(lock) => Some(lock),
            None => {
                if !background {
                    control::send("open")?;
                }
                return Ok(());
            }
        }
    };
    // The demo only serves a socket when asked to (for testing the bar),
    // so it never takes over the real instance's.
    let serve = !demo || std::env::var_os("OMALIVE_SOCKET").is_some();
    let config = config_dir();
    let (settings, warning) = if demo {
        (Settings::default(), None)
    } else {
        Settings::load(&config)
    };
    gpui_kit::application()
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_omarchy::init(cx);
            ui::init_theme(cx);
            ui::bind_keys(cx);
            let (sender, events) = async_channel::unbounded();
            let emit: omalive_media::Emit = Arc::new(move |event| {
                let _ = sender.try_send(event);
            });
            let engine: Box<dyn engine::Engine> = if demo {
                Box::new(engine::Demo::start(emit))
            } else {
                Box::new(engine::Live::start(emit))
            };
            let plugins: Plugins = engine.plugins();
            let (meter, thumb) = engine.bar_sources();
            let lut_dir = if demo {
                demo_looks(&std::env::temp_dir().join("omalive-demo-looks"))
            } else {
                cube::library_dir()
            };
            let mut model = model::Model::new(settings.clone(), plugins, lut_dir);
            if demo && let Some(path) = std::env::var_os("OMALIVE_DEMO_PICTURE") {
                model.demo_picture = demo_picture(std::path::Path::new(&path));
            }
            if let Some(warning) = warning.clone() {
                model.say(warning, model::Tone::Warning);
            }
            let view = cx.new(|cx| {
                let mut app =
                    ui::Omalive::new(model, engine, events, demo, (!demo).then_some(config), cx);
                if std::env::var("OMALIVE_PAGE").as_deref() == Ok("camera") {
                    app.model.page = model::Page::Camera;
                }
                // The demo starts live: a simulated talker and a
                // test-pattern camera, so every view is populated.
                if demo {
                    app.model.settings.voice.microphone = "demo.interface".into();
                    app.model.settings.voice.port = "capture_FL".into();
                    let mut effects = app.model.start_voice();
                    effects.extend(app.model.select_lut("Warm_film.cube"));
                    effects.extend(app.model.toggle_camera(true));
                    app.run(effects, cx);
                } else {
                    app.resume(cx);
                }
                app
            });
            let (commands, received) = async_channel::unbounded();
            let server = if serve {
                match control::Server::start(&control::socket_path(), commands, meter, thumb) {
                    Ok(server) => {
                        let hub = server.hub.clone();
                        view.update(cx, |app, cx| app.set_hub(hub, cx));
                        Some(server)
                    }
                    Err(error) => {
                        eprintln!("omalive: the bar cannot reach this instance: {error:#}");
                        None
                    }
                }
            } else {
                None
            };
            session::install(view, demo, server, cx);
            cx.spawn(async move |cx| {
                while let Ok(command) = received.recv().await {
                    cx.update(|cx| session::command(command, cx));
                }
            })
            .detach();
            if !background {
                session::show(cx);
            }
        });
    Ok(())
}

/// A picture for the demo camera to pan across, scaled to cover a little
/// more than its 1280×720 frame.
fn demo_picture(path: &std::path::Path) -> Option<Arc<omalive_media::camera::synthetic::Picture>> {
    let image = image::open(path)
        .inspect_err(|error| eprintln!("omalive: demo picture {}: {error}", path.display()))
        .ok()?
        .into_rgb8();
    let scale = (1472. / image.width() as f32).max(828. / image.height() as f32);
    let (width, height) = (
        (image.width() as f32 * scale).ceil() as u32,
        (image.height() as f32 * scale).ceil() as u32,
    );
    let image =
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
    Some(Arc::new(omalive_media::camera::synthetic::Picture {
        width,
        height,
        rgb: image.into_raw(),
    }))
}

/// `omalive import FILE...`: add looks to the library from a script.
fn import(files: &[String]) -> Result<()> {
    if files.is_empty() {
        anyhow::bail!("Usage: omalive import FILE...");
    }
    let dir = cube::library_dir();
    let mut failed = 0;
    for file in files {
        match model::import_one(std::path::Path::new(file), &dir) {
            Ok(name) => println!("{name}"),
            Err(error) => {
                eprintln!("{file}: {error:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} of {} could not be imported", files.len());
    }
    Ok(())
}

/// The demo's looks: omalive's starter set, in a scratch library.
fn demo_looks(dir: &std::path::Path) -> std::path::PathBuf {
    let _ = omalive_core::looks::install_starter_looks(dir);
    dir.to_path_buf()
}

/// Hold an exclusive lock for this session's omalive, or `None` when
/// another instance already has it.
fn single_instance() -> Result<Option<std::fs::File>> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("omalive.lock"))
        .context("Creating the instance lock")?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error).context("Locking the instance lock"),
    }
}
