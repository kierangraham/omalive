//! Registry globals → [`Graph`] entries. Only the properties PipeWire puts
//! on globals are used, so no proxies are bound to track the graph.
use omalive_core::graph::{Link, Node, Port};

/// A node from its global properties. Hardware nodes carry a device id;
/// filter chains, loopbacks and omalive's own nodes do not.
pub fn node<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Node {
    Node {
        name: get("node.name").unwrap_or_default().into(),
        description: get("node.description").unwrap_or_default().into(),
        media_class: get("media.class").unwrap_or_default().into(),
        virtual_: get("device.id").is_none(),
    }
}

pub fn port<'a>(id: u32, get: impl Fn(&str) -> Option<&'a str>) -> Option<Port> {
    Some(Port {
        id,
        node: get("node.id")?.parse().ok()?,
        name: get("port.name").unwrap_or_default().into(),
        channel: get("audio.channel").unwrap_or_default().into(),
        output: get("port.direction")? == "out",
        audio: get("format.dsp").unwrap_or_default().contains("audio"),
    })
}

pub fn link<'a>(id: u32, get: impl Fn(&str) -> Option<&'a str>) -> Option<Link> {
    Some(Link {
        id,
        output_port: get("link.output.port")?.parse().ok()?,
        input_port: get("link.input.port")?.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn props(pairs: &[(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn hardware_and_virtual_nodes() {
        let hardware = props(&[
            ("node.name", "alsa_input.x"),
            ("media.class", "Audio/Source"),
            ("device.id", "41"),
        ]);
        let node = node(|k| hardware.get(k).copied());
        assert!(!node.virtual_);
        assert_eq!(node.media_class, "Audio/Source");
        let chain = props(&[
            ("node.name", "effect_output.mic"),
            ("node.description", "Mic"),
        ]);
        assert!(super::node(|k| chain.get(k).copied()).virtual_);
    }

    #[test]
    fn ports_need_node_and_direction() {
        let complete = props(&[
            ("node.id", "56"),
            ("port.name", "capture_AUX0"),
            ("port.direction", "out"),
            ("audio.channel", "AUX0"),
            ("format.dsp", "32 bit float mono audio"),
        ]);
        let port = port(64, |k| complete.get(k).copied()).unwrap();
        assert_eq!((port.node, port.output, port.audio), (56, true, true));
        let orphan = props(&[("port.name", "x"), ("port.direction", "in")]);
        assert!(super::port(1, |k| orphan.get(k).copied()).is_none());
    }

    #[test]
    fn links_parse_ports() {
        let pairs = props(&[("link.output.port", "10"), ("link.input.port", "30")]);
        assert_eq!(link(5, |k| pairs.get(k).copied()).unwrap().input_port, 30);
        assert!(link(5, |_| None).is_none());
    }
}
