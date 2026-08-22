//! Platform layer — re-exports Windows / Linux / macOS-specific code.
//!
//! The executor and recorder use these functions to dispatch input.
//! Reference: `docs/MACRO_ARCHITECTURE.md` §11.

pub mod backend;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
