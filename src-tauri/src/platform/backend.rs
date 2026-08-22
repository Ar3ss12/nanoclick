//! Platform contracts shared by the executor, recorder and hotkey layer.
//! Implementations are platform-specific; core code depends only on these
//! contracts and never on VK/X11/AppKit handles.

#![allow(dead_code)]

use crate::core::action::{KeyCode, Modifiers, MouseButton};
use crate::recorder::raw_event::RawEvent;
use std::sync::mpsc::Sender;

pub trait InputBackend: Send + Sync {
    fn mouse_click(&self, button: MouseButton);
    fn mouse_down(&self, button: MouseButton);
    fn mouse_up(&self, button: MouseButton);
    fn key_press(&self, key: KeyCode, modifiers: Modifiers);
    fn key_down(&self, key: KeyCode, modifiers: Modifiers);
    fn key_up(&self, key: KeyCode, modifiers: Modifiers);
    fn cursor_position(&self) -> (i32, i32);
}

pub trait HotkeyBackend: Send + Sync {
    type Binding: Send + Sync + Clone + 'static;

    fn register(&self, binding: Self::Binding) -> Result<(), String>;
    fn unregister(&self, binding: &Self::Binding) -> Result<(), String>;
    fn shutdown(&self);
}

pub trait RecorderBackend: Send + Sync {
    fn start(&self, sender: Sender<RawEvent>, ignored_hotkey: String) -> Result<(), String>;
    fn stop(&self);
}
