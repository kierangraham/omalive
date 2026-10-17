//! omalive's device-independent model: settings, the voice filter graph,
//! colour grading and the PipeWire graph view. No threads or system I/O
//! beyond reading and writing settings and LUT files.
pub mod analysis;
pub mod color;
pub mod cube;
pub mod grade;
pub mod graph;
pub mod looks;
pub mod settings;
pub mod voice;
pub mod voice_test;

pub use cube::{Cube, LutEntry};
pub use grade::{Control, Grade, Lattice, bake};
pub use graph::{Channel, Graph, Input};
pub use settings::{AppSettings, CameraSettings, Preset, Settings, VoiceSettings};
pub use voice::Plugins;
