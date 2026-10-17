//! A plain model of the PipeWire graph, maintained from registry events.
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub description: String,
    pub media_class: String,
    pub virtual_: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub id: u32,
    pub node: u32,
    pub name: String,
    pub channel: String,
    pub output: bool,
    pub audio: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub id: u32,
    pub output_port: u32,
    pub input_port: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: BTreeMap<u32, Node>,
    pub ports: BTreeMap<u32, Port>,
    pub links: BTreeMap<u32, Link>,
}

/// A physical microphone and the channels it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub description: String,
    pub ports: Vec<Channel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub label: String,
}

impl Graph {
    pub fn remove(&mut self, id: u32) -> bool {
        self.nodes.remove(&id).is_some()
            || self.ports.remove(&id).is_some()
            || self.links.remove(&id).is_some()
    }

    pub fn node_id(&self, name: &str) -> Option<u32> {
        self.nodes
            .iter()
            .find(|(_, node)| node.name == name)
            .map(|(id, _)| *id)
    }

    /// Audio sources with at least one output channel, hardware first.
    /// Processed virtual sources stay listed: on Apple silicon the built-in
    /// microphone is only usable through its DSP chain. omalive's own
    /// sources and inputs the system marks "do not use" are excluded.
    pub fn inputs(&self) -> Vec<Input> {
        let mut inputs: Vec<(bool, Input)> = self
            .nodes
            .iter()
            .filter(|(_, node)| {
                node.media_class == "Audio/Source"
                    && !node.name.starts_with("omalive_")
                    && !node.description.to_lowercase().contains("do not use")
            })
            .filter_map(|(id, node)| {
                let ports: Vec<Channel> = self
                    .ports
                    .values()
                    .filter(|p| p.node == *id && p.output && p.audio)
                    .map(|p| Channel {
                        name: p.name.clone(),
                        label: if p.channel.is_empty() {
                            p.name.clone()
                        } else {
                            format!("{} · {}", p.name, p.channel)
                        },
                    })
                    .collect();
                (!ports.is_empty()).then(|| {
                    (
                        node.virtual_,
                        Input {
                            name: node.name.clone(),
                            description: if node.description.is_empty() {
                                node.name.clone()
                            } else {
                                node.description.clone()
                            },
                            ports,
                        },
                    )
                })
            })
            .collect();
        inputs.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.description.cmp(&b.1.description))
        });
        inputs.into_iter().map(|(_, input)| input).collect()
    }

    /// The single port with this node name, port name and direction.
    pub fn port(&self, node: &str, port: &str, output: bool) -> Option<&Port> {
        let mut matches = self.ports.values().filter(|p| {
            p.name == port
                && p.output == output
                && self.nodes.get(&p.node).is_some_and(|n| n.name == node)
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    /// Input ports of a node, by node name.
    pub fn inputs_of(&self, node: &str) -> Vec<&Port> {
        self.ports
            .values()
            .filter(|p| !p.output && self.nodes.get(&p.node).is_some_and(|n| n.name == node))
            .collect()
    }

    /// How many other nodes are reading from `node`: call apps recording
    /// the virtual microphone. omalive's own meter and taps do not count.
    pub fn listeners(&self, node: &str) -> u32 {
        let Some(source) = self.node_id(node) else {
            return 0;
        };
        let mut readers: Vec<u32> = self
            .links
            .values()
            .filter(|l| {
                self.ports
                    .get(&l.output_port)
                    .is_some_and(|p| p.node == source)
            })
            .filter_map(|l| self.ports.get(&l.input_port).map(|p| p.node))
            .filter(|n| {
                self.nodes
                    .get(n)
                    .is_some_and(|n| !n.name.starts_with("omalive_"))
            })
            .collect();
        readers.sort_unstable();
        readers.dedup();
        readers.len() as u32
    }

    pub fn linked(&self, output_port: u32, input_port: u32) -> bool {
        self.links
            .values()
            .any(|l| l.output_port == output_port && l.input_port == input_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Graph {
        let mut graph = Graph::default();
        graph.nodes.insert(
            1,
            Node {
                name: "interface".into(),
                description: "Scarlett".into(),
                media_class: "Audio/Source".into(),
                virtual_: false,
            },
        );
        graph.nodes.insert(
            2,
            Node {
                name: "virtual".into(),
                media_class: "Audio/Source".into(),
                virtual_: true,
                ..Default::default()
            },
        );
        graph.nodes.insert(
            3,
            Node {
                name: "omalive_voice_capture".into(),
                media_class: "Stream/Input/Audio".into(),
                ..Default::default()
            },
        );
        for (id, node, name, channel, output) in [
            (10, 1, "capture_FL", "FL", true),
            (11, 1, "capture_FR", "FR", true),
            (20, 2, "out", "MONO", true),
            (30, 3, "input_MONO", "MONO", false),
        ] {
            graph.ports.insert(
                id,
                Port {
                    id,
                    node,
                    name: name.into(),
                    channel: channel.into(),
                    output,
                    audio: true,
                },
            );
        }
        graph
    }

    #[test]
    fn listeners_count_other_apps_once() {
        let mut graph = graph();
        for (id, name) in [(5, "omalive_voice"), (6, "Meet"), (7, "omalive_meter")] {
            graph.nodes.insert(
                id,
                Node {
                    name: name.into(),
                    ..Default::default()
                },
            );
        }
        for (id, node, output) in [
            (50, 5, true),
            (60, 6, false),
            (61, 6, false),
            (70, 7, false),
        ] {
            graph.ports.insert(
                id,
                Port {
                    id,
                    node,
                    name: format!("p{id}"),
                    channel: String::new(),
                    output,
                    audio: true,
                },
            );
        }
        assert_eq!(graph.listeners("omalive_voice"), 0);
        for (id, input_port) in [(100, 60), (101, 61), (102, 70)] {
            graph.links.insert(
                id,
                Link {
                    id,
                    output_port: 50,
                    input_port,
                },
            );
        }
        // Meet's two channels count once; omalive's own meter not at all.
        assert_eq!(graph.listeners("omalive_voice"), 1);
        assert_eq!(graph.listeners("missing"), 0);
    }

    #[test]
    fn hardware_first_own_and_unusable_excluded() {
        let mut graph = graph();
        graph.nodes.insert(
            4,
            Node {
                name: "raw".into(),
                description: "Raw Mic Device (do not use)".into(),
                media_class: "Audio/Source".into(),
                virtual_: false,
            },
        );
        graph.ports.insert(
            40,
            Port {
                id: 40,
                node: 4,
                ..graph.ports[&10].clone()
            },
        );
        graph.nodes.insert(
            5,
            Node {
                name: "omalive_voice".into(),
                media_class: "Audio/Source".into(),
                virtual_: true,
                ..Default::default()
            },
        );
        graph.ports.insert(
            50,
            Port {
                id: 50,
                node: 5,
                ..graph.ports[&10].clone()
            },
        );
        let inputs = graph.inputs();
        let names: Vec<_> = inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["interface", "virtual"]);
        assert_eq!(inputs[0].description, "Scarlett");
        assert_eq!(inputs[0].ports[1].label, "capture_FR · FR");
    }

    #[test]
    fn exact_channel_and_direction() {
        let mut graph = graph();
        assert_eq!(graph.port("interface", "capture_FL", true).unwrap().id, 10);
        assert!(graph.port("interface", "capture_FL", false).is_none());
        assert!(graph.port("interface", "missing", true).is_none());
        graph.ports.insert(
            12,
            Port {
                id: 12,
                ..graph.ports[&10].clone()
            },
        );
        assert!(
            graph.port("interface", "capture_FL", true).is_none(),
            "ambiguous"
        );
    }

    #[test]
    fn links_and_removal() {
        let mut graph = graph();
        graph.links.insert(
            40,
            Link {
                id: 40,
                output_port: 10,
                input_port: 30,
            },
        );
        assert!(graph.linked(10, 30));
        assert_eq!(graph.inputs_of("omalive_voice_capture")[0].id, 30);
        assert!(graph.remove(40));
        assert!(!graph.linked(10, 30));
        assert!(!graph.remove(40));
    }
}
