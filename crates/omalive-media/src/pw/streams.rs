//! Streams omalive owns on the PipeWire thread: the level meter and the
//! "Omalive Camera" video source.
use super::pods;
use crate::{
    Emit, Event,
    frames::{Frame, PixelFormat, Slot},
    meter::{Meter, Tap},
};
use anyhow::{Context, Result};
use omalive_core::voice;
use pipewire as pw;
use pw::{
    properties::properties,
    spa::{
        param::{ParamType, video::VideoFormat},
        pod::Pod,
        utils::Direction,
    },
    stream::{StreamFlags, StreamListener, StreamRc, StreamState},
};
use std::sync::Arc;

pub struct MeterStream {
    _stream: StreamRc,
    _listener: StreamListener<(Arc<Meter>, Arc<Tap>)>,
}

/// Capture the virtual microphone for metering and the visualisations. The
/// realtime callback only touches atomics.
pub fn meter(core: &pw::core::CoreRc, meter: Arc<Meter>, tap: Arc<Tap>) -> Result<MeterStream> {
    let stream = StreamRc::new(
        core.clone(),
        "omalive-meter",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Production",
            *pw::keys::NODE_NAME => "omalive_meter",
            *pw::keys::TARGET_OBJECT => voice::SOURCE_NODE,
            "node.dont-reconnect" => "true",
            // The meter must never lower the graph quantum.
            "node.latency" => "1024/48000",
            "stream.dont-remix" => "true",
        },
    )?;
    let listener = stream
        .add_local_listener_with_user_data((meter, tap))
        .process(|stream, (meter, tap)| {
            if let Some(mut buffer) = stream.dequeue_buffer()
                && let Some(first) = buffer.datas_mut().first_mut()
            {
                let size = first.chunk().size() as usize;
                if let Some(bytes) = first.data() {
                    let bytes = &bytes[..size.min(bytes.len())];
                    meter.analyse(bytes);
                    tap.push_bytes(bytes);
                }
            }
        })
        .register()?;
    let bytes = pods::meter_format();
    let pod = Pod::from_bytes(&bytes).context("Meter format")?;
    stream.connect(
        Direction::Input,
        None,
        StreamFlags::AUTOCONNECT
            | StreamFlags::MAP_BUFFERS
            | StreamFlags::RT_PROCESS
            | StreamFlags::DONT_RECONNECT,
        &mut [pod],
    )?;
    Ok(MeterStream {
        _stream: stream,
        _listener: listener,
    })
}

pub struct TapStream {
    _stream: StreamRc,
    _listener: StreamListener<Arc<Tap>>,
}

pub const TAP_NODE: &str = "omalive_tap";

/// An unconnected capture stream; the PipeWire thread links the selected
/// microphone port to it so the spectrum can show the raw input.
pub fn tap(core: &pw::core::CoreRc, tap: Arc<Tap>) -> Result<TapStream> {
    let stream = StreamRc::new(
        core.clone(),
        "omalive-tap",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Production",
            *pw::keys::NODE_NAME => TAP_NODE,
            "node.dont-reconnect" => "true",
            "node.latency" => "1024/48000",
            "stream.dont-remix" => "true",
        },
    )?;
    let listener = stream
        .add_local_listener_with_user_data(tap)
        .process(|stream, tap| {
            if let Some(mut buffer) = stream.dequeue_buffer()
                && let Some(first) = buffer.datas_mut().first_mut()
            {
                let size = first.chunk().size() as usize;
                if let Some(bytes) = first.data() {
                    tap.push_bytes(&bytes[..size.min(bytes.len())]);
                }
            }
        })
        .register()?;
    let bytes = pods::meter_format();
    let pod = Pod::from_bytes(&bytes).context("Tap format")?;
    stream.connect(
        Direction::Input,
        None,
        StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS | StreamFlags::DONT_RECONNECT,
        &mut [pod],
    )?;
    Ok(TapStream {
        _stream: stream,
        _listener: listener,
    })
}

pub struct VideoOut {
    pub stream: StreamRc,
    _listener: StreamListener<VideoData>,
}

pub struct VideoData {
    slot: Arc<Slot>,
    emit: Emit,
    format: pw::spa::param::video::VideoInfoRaw,
    seen: u64,
}

fn pixel_format(format: VideoFormat) -> PixelFormat {
    match format {
        VideoFormat::YUY2 => PixelFormat::Yuyv,
        _ => PixelFormat::Nv12,
    }
}

/// "Omalive Camera": a driver-mode source. The camera thread triggers one
/// process cycle per graded frame, so frames leave as soon as they exist.
pub fn video(
    core: &pw::core::CoreRc,
    slot: Arc<Slot>,
    emit: Emit,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<VideoOut> {
    let stream = StreamRc::new(
        core.clone(),
        "omalive-camera",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Source",
            *pw::keys::MEDIA_ROLE => "Camera",
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::NODE_NAME => "omalive_camera",
            *pw::keys::NODE_DESCRIPTION => "Omalive Camera",
            "node.nick" => "Omalive Camera",
            "device.icon-name" => "camera-web",
        },
    )?;
    let listener = stream
        .add_local_listener_with_user_data(VideoData {
            slot,
            emit,
            format: Default::default(),
            seen: 0,
        })
        .state_changed(|_, data, _, state| {
            let streaming = matches!(state, StreamState::Streaming);
            data.slot.want(if streaming {
                pixel_format(data.format.format())
            } else {
                PixelFormat::None
            });
            (data.emit)(Event::CameraViewers(streaming));
        })
        .param_changed(|stream, data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != ParamType::Format.as_raw() || data.format.parse(param).is_err() {
                return;
            }
            let format = pixel_format(data.format.format());
            let size = data.format.size();
            let bytes = pods::buffers(
                format.frame_size(size.width, size.height),
                format.stride(size.width),
            );
            if let Some(pod) = Pod::from_bytes(&bytes) {
                let _ = stream.update_params(&mut [pod]);
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(target) = buffer.datas_mut().first_mut() else {
                return;
            };
            let size = data.format.size();
            let format = pixel_format(data.format.format());
            let mut written = 0;
            if let Some(destination) = target.data()
                && let Some((sequence, copied)) = data.slot.read(data.seen, |frame| {
                    (
                        frame.sequence,
                        copy_frame(frame, destination, format, size.width, size.height),
                    )
                })
            {
                data.seen = sequence;
                written = copied;
            }
            let chunk = target.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = format.stride(size.width) as i32;
            *chunk.size_mut() = written as u32;
        })
        .register()?;
    let bytes = pods::video_format(width, height, fps);
    let pod = Pod::from_bytes(&bytes).context("Camera format")?;
    stream.connect(
        Direction::Output,
        None,
        StreamFlags::DRIVER | StreamFlags::MAP_BUFFERS,
        &mut [pod],
    )?;
    Ok(VideoOut {
        stream,
        _listener: listener,
    })
}

/// Copy a graded frame into a consumer buffer if it matches the negotiated
/// format, returning the bytes written (0 for a mismatch, e.g. mid-resize).
fn copy_frame(
    frame: &Frame,
    destination: &mut [u8],
    format: PixelFormat,
    width: u32,
    height: u32,
) -> usize {
    let expected = format.frame_size(width, height);
    let fits = expected > 0
        && frame.data.len() == expected
        && frame.width == width
        && frame.height == height
        && destination.len() >= expected;
    if fits {
        destination[..expected].copy_from_slice(&frame.data);
        expected
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_copy_only_when_they_match() {
        let frame = Frame {
            data: vec![7; 8 * 4 * 3 / 2],
            width: 8,
            height: 4,
            ..Default::default()
        };
        let mut destination = vec![0; 64];
        assert_eq!(
            copy_frame(&frame, &mut destination, PixelFormat::Nv12, 8, 4),
            48
        );
        assert!(destination[..48].iter().all(|b| *b == 7));
        assert_eq!(
            copy_frame(&frame, &mut destination, PixelFormat::Yuyv, 8, 4),
            0,
            "format changed"
        );
        assert_eq!(
            copy_frame(&frame, &mut destination, PixelFormat::Nv12, 16, 4),
            0,
            "size changed"
        );
        assert_eq!(
            copy_frame(&frame, &mut [0; 8], PixelFormat::Nv12, 8, 4),
            0,
            "buffer too small"
        );
    }
}
