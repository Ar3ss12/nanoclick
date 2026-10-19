//! Recorder module — captures and normalizes user input into Macro Actions.

pub mod normalizer;
pub mod raw_event;
#[allow(clippy::module_inception)]
pub mod recorder;

pub use recorder::{RecorderHandle, RecordingMode};
