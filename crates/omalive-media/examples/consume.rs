//! Acts as a call app: connects to "Omalive Camera" over PipeWire and
//! reports the negotiated format, frame rate and frame contents.
//! `cargo run --release -p omalive-media --example consume -- [seconds]`
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;
use std::{cell::RefCell, rc::Rc, time::Instant};

#[derive(Default)]
struct Seen {
    format: spa::param::video::VideoInfoRaw,
    frames: u32,
    bytes: usize,
    first: Option<Instant>,
    mean_luma: f32,
}

fn main() -> Result<(), pw::Error> {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "omalive-consumer-test",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Camera",
            *pw::keys::TARGET_OBJECT => "omalive_camera",
        },
    )?;
    let seen = Rc::new(RefCell::new(Seen::default()));
    let _listener = stream
        .add_local_listener_with_user_data(seen.clone())
        .state_changed(|_, _, old, new| println!("state {old:?} -> {new:?}"))
        .param_changed(|_, seen, id, param| {
            if let Some(param) = param
                && id == spa::param::ParamType::Format.as_raw()
            {
                let mut seen = seen.borrow_mut();
                let _ = seen.format.parse(param);
                println!(
                    "negotiated {:?} {}x{} @ {}/{}",
                    seen.format.format(),
                    seen.format.size().width,
                    seen.format.size().height,
                    seen.format.framerate().num,
                    seen.format.framerate().denom
                );
            }
        })
        .process(|stream, seen| {
            if let Some(mut buffer) = stream.dequeue_buffer()
                && let Some(data) = buffer.datas_mut().first_mut()
            {
                let size = data.chunk().size() as usize;
                let mut seen = seen.borrow_mut();
                seen.first.get_or_insert_with(Instant::now);
                seen.frames += 1;
                seen.bytes = size;
                if let Some(bytes) = data.data() {
                    let luma = &bytes[..size.min(bytes.len()).min(4096)];
                    seen.mean_luma =
                        luma.iter().map(|b| *b as f32).sum::<f32>() / luma.len().max(1) as f32;
                }
            }
        })
        .register()?;
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::NV12,
            spa::param::video::VideoFormat::NV12,
            spa::param::video::VideoFormat::YUY2
        ),
    );
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .unwrap()
    .0
    .into_inner();
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut [Pod::from_bytes(&bytes).unwrap()],
    )?;
    let weak = main_loop.downgrade();
    let timer = main_loop.loop_().add_timer(move |_| {
        if let Some(main_loop) = weak.upgrade() {
            main_loop.quit();
        }
    });
    timer.update_timer(Some(std::time::Duration::from_secs(seconds)), None);
    main_loop.run();
    let seen = seen.borrow();
    let elapsed = seen.first.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.);
    println!(
        "received {} frames ({:.1} fps), {} bytes each, mean luma of first 4 KiB {:.0}",
        seen.frames,
        seen.frames as f32 / elapsed.max(0.001),
        seen.bytes,
        seen.mean_luma
    );
    Ok(())
}
