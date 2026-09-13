//! Recorder module — captures and normalizes user input into Macro Actions.

pub mod normalizer;
pub mod raw_event;
pub mod recorder;

pub use recorder::{RecorderHandle, RecordingMode};
