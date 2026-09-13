//! # Recorder — captures raw input events and produces a clean `Vec<Action>`
//! via the Smart Normalizer.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §3.
//!
//! Architecture:
//! - The frontend (or a hotkey callback) calls `Recorder::start(mode)`.
//! - That spawns two background threads:
//!     * Capture thread — calls a platform-supplied closure that pushes
//!       `RawEvent`s into the channel (Windows hooks go here).
//!     * Consumer thread — receives events, buffers them, and on stop
//!       hands the buffer to the Normalizer.
//! - `RecorderHandle::stop()` blocks briefly, then returns the final
//!   `Vec<Action>`.
//!
//! The Recorder itself is platform-agnostic — only the `event_source`
//! closure varies (Windows hooks, Linux evdev, etc.).

// Submodules are declared in `recorder/mod.rs` to avoid duplicates here.

use crate::core::Action;
use crate::recorder::normalizer;
use crate::recorder::raw_event::RawEvent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Recording mode — `Smart` (default) applies the 5-phase Normalizer,
/// `Precise` keeps every event as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingMode {
    Smart,
    Precise,
}

impl Default for RecordingMode {
    fn default() -> Self {
        RecordingMode::Smart
    }
}

/// Threadsafe handle for a recording session.
#[derive(Clone)]
pub struct RecorderHandle {
    shared: Arc<Shared>,
}

struct Shared {
    is_running: AtomicBool,
    mode: Mutex<RecordingMode>,
    final_actions: Mutex<Option<Vec<Action>>>,
}

impl RecorderHandle {
    /// Start a new recording session. The caller supplies a closure that
    /// pushes `RawEvent`s — this is the platform-specific entry point
    /// (Windows hooks or cursor polling).
    ///
    /// The closure runs on a dedicated thread. When the recording is
    /// stopped, the consumer thread joins naturally.
    #[allow(dead_code)]
    pub fn start<F>(mode: RecordingMode, event_source: F) -> Self
    where
        F: FnOnce(Sender<RawEvent>) + Send + 'static,
    {
        let (tx, rx): (Sender<RawEvent>, Receiver<RawEvent>) = mpsc::channel();
        let shared = Arc::new(Shared {
            is_running: AtomicBool::new(true),
            mode: Mutex::new(mode),
            final_actions: Mutex::new(None),
        });

        // ── Capture thread: runs platform-specific event_source. ──
        let tx_for_capture = tx.clone();
        thread::spawn(move || {
            event_source(tx_for_capture);
        });

        let handle = RecorderHandle {
            shared: shared.clone(),
        };
        spawn_consumer_thread(shared, rx, Some(tx));
        handle
    }

    /// Variant used by the Tauri command layer: builds a recorder and
    /// returns the input `Sender` so the platform hook thread can push
    /// events into the same channel without going through a closure.
    pub fn start_with_external_sender(mode: RecordingMode) -> (Self, Sender<RawEvent>) {
        let (tx, rx): (Sender<RawEvent>, Receiver<RawEvent>) = mpsc::channel();
        let shared = Arc::new(Shared {
            is_running: AtomicBool::new(true),
            mode: Mutex::new(mode),
            final_actions: Mutex::new(None),
        });
        // No capture thread — the platform hook pushes directly into `tx`.
        let handle = RecorderHandle { shared };
        // Keep `tx` alive inside the consumer until is_running flips; we
        // do that by passing a clone of `tx` for `drop` later.
        let _keepalive = tx.clone();
        spawn_consumer_thread(handle.shared.clone(), rx, Some(_keepalive));
        (handle, tx)
    }

    /// Stop recording and return the resulting list of actions.
    pub fn stop(&self) -> Vec<Action> {
        self.shared.is_running.store(false, Ordering::Relaxed);
        // Poll briefly for the consumer thread to finalize.
        for _ in 0..120 {
            if self.shared.final_actions.lock().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.shared
            .final_actions
            .lock()
            .unwrap()
            .take()
            .unwrap_or_default()
    }

    /// Force-cancel (e.g. on app shutdown).
    pub fn cancel(&self) {
        self.shared.is_running.store(false, Ordering::Relaxed);
    }

    /// Whether recording is still active.
    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.shared.is_running.load(Ordering::Relaxed)
    }
}

/// Spawn the consumer thread that buffers events until is_running flips.
fn spawn_consumer_thread(
    shared: Arc<Shared>,
    rx: Receiver<RawEvent>,
    tx_keepalive: Option<Sender<RawEvent>>,
) {
    thread::spawn(move || {
        let mut buffer: Vec<RawEvent> = Vec::new();
        while shared.is_running.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(ev) => buffer.push(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // Drain anything still in the channel.
        while let Ok(ev) = rx.try_recv() {
            buffer.push(ev);
        }
        let mode = *shared.mode.lock().unwrap();
        let actions: Vec<Action> = if mode == RecordingMode::Precise {
            buffer.into_iter().map(|e| e.to_action()).collect()
        } else {
            normalizer::normalize(buffer)
        };
        *shared.final_actions.lock().unwrap() = Some(actions);
        drop(tx_keepalive); // release the sender so the channel disconnects
    });
}

/// Test-only: build a handle with a synthetic event source. Used by the
/// Normalizer unit tests.
#[cfg(test)]
#[allow(dead_code)]
pub fn start_with_sender<F>(mode: RecordingMode, source: F) -> (RecorderHandle, Sender<RawEvent>)
where
    F: FnOnce(Sender<RawEvent>) + Send + 'static,
{
    let (tx, _rx) = mpsc::channel();
    let handle = RecorderHandle::start(mode, source);
    (handle, tx)
}
