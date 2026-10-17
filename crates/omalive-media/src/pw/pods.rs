//! SPA parameter objects, built as plain bytes so they can be unit tested
//! without a PipeWire connection.
use pipewire::spa::{
    self,
    param::{ParamType, format::FormatProperties, video::VideoFormat},
    pod::{ChoiceValue, Object, Property, Value, serialize::PodSerializer},
    utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Rectangle, SpaTypes},
};

pub fn bytes(value: &Value) -> Vec<u8> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), value)
        .map(|(cursor, _)| cursor.into_inner())
        .unwrap_or_default()
}

fn props(properties: Vec<Property>) -> Vec<u8> {
    bytes(&Value::Object(Object {
        type_: SpaTypes::ObjectParamProps.as_raw(),
        id: ParamType::Props.as_raw(),
        properties,
    }))
}

/// filter-chain controls: `Props { params = [ "node:Control" value … ] }`.
pub fn controls(controls: &[(String, f32)]) -> Vec<u8> {
    let values = controls
        .iter()
        .flat_map(|(name, value)| [Value::String(name.clone()), Value::Float(*value)])
        .collect();
    props(vec![Property::new(
        spa::sys::SPA_PROP_params,
        Value::Struct(values),
    )])
}

pub fn mute(muted: bool) -> Vec<u8> {
    props(vec![Property::new(
        spa::sys::SPA_PROP_mute,
        Value::Bool(muted),
    )])
}

/// F32 mono at 48 kHz, for the level meter.
pub fn meter_format() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_channels(1);
    info.set_rate(48000);
    bytes(&Value::Object(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    }))
}

/// Buffer requirements for one negotiated video frame. Few buffers keep
/// latency down; PipeWire allocates and maps them.
pub fn buffers(size: usize, stride: u32) -> Vec<u8> {
    bytes(&Value::Object(Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: 3,
                        min: 2,
                        max: 8,
                    },
                ))),
            ),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_size, Value::Int(size as i32)),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_stride,
                Value::Int(stride as i32),
            ),
        ],
    }))
}

/// The camera's offer: NV12 (preferred) or YUY2 at the capture size and rate.
pub fn video_format(width: u32, height: u32, fps: u32) -> Vec<u8> {
    let object = spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        spa::pod::property!(
            FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::NV12,
            VideoFormat::NV12,
            VideoFormat::YUY2
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Rectangle,
            Rectangle { width, height }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Fraction,
            Fraction { num: fps, denom: 1 }
        ),
    );
    bytes(&Value::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spa::pod::deserialize::PodDeserializer;

    fn object(bytes: &[u8]) -> Object {
        match PodDeserializer::deserialize_any_from(bytes).unwrap().1 {
            Value::Object(object) => object,
            other => panic!("not an object: {other:?}"),
        }
    }

    fn property(object: &Object, key: u32) -> &Value {
        &object
            .properties
            .iter()
            .find(|p| p.key == key)
            .unwrap()
            .value
    }

    #[test]
    fn controls_are_name_value_pairs() {
        let pod = object(&controls(&[
            ("low:Gain".into(), 4.5),
            ("gain:Mult".into(), 0.5),
        ]));
        assert_eq!(pod.id, ParamType::Props.as_raw());
        assert_eq!(
            property(&pod, spa::sys::SPA_PROP_params),
            &Value::Struct(vec![
                Value::String("low:Gain".into()),
                Value::Float(4.5),
                Value::String("gain:Mult".into()),
                Value::Float(0.5),
            ])
        );
        assert_eq!(
            property(&object(&mute(true)), spa::sys::SPA_PROP_mute),
            &Value::Bool(true)
        );
    }

    #[test]
    fn buffers_carry_size_and_stride() {
        let pod = object(&buffers(1_382_400, 1280));
        assert_eq!(
            property(&pod, spa::sys::SPA_PARAM_BUFFERS_size),
            &Value::Int(1_382_400)
        );
        assert_eq!(
            property(&pod, spa::sys::SPA_PARAM_BUFFERS_stride),
            &Value::Int(1280)
        );
    }

    #[test]
    fn video_offer_is_fixed_size_and_rate() {
        let pod = object(&video_format(1280, 720, 30));
        assert_eq!(
            property(&pod, FormatProperties::VideoSize.as_raw()),
            &Value::Rectangle(Rectangle {
                width: 1280,
                height: 720
            })
        );
        assert_eq!(
            property(&pod, FormatProperties::VideoFramerate.as_raw()),
            &Value::Fraction(Fraction { num: 30, denom: 1 })
        );
        assert!(!meter_format().is_empty());
    }
}
