//! Windows keyboard & mouse input synthesis — used by the Macro Executor.
//!
//! Wraps Win32 `SendInput` for synthesizing input events.

use crate::core::action::MouseButton;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

/// Move the OS cursor to the absolute screen position.
pub fn set_cursor_pos(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
    }
}

/// Press the mouse button down (no release).
pub fn mouse_down(button: MouseButton) {
    let (flags, data) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, 0u32),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, 0u32),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, 0u32),
        MouseButton::X1 => (MOUSEEVENTF_XDOWN, 1u32),
        MouseButton::X2 => (MOUSEEVENTF_XDOWN, 2u32),
    };
    send_mouse_input(flags, data);
}

/// Release the mouse button.
pub fn mouse_up(button: MouseButton) {
    let (flags, data) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTUP, 0u32),
        MouseButton::Right => (MOUSEEVENTF_RIGHTUP, 0u32),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEUP, 0u32),
        MouseButton::X1 => (MOUSEEVENTF_XUP, 1u32),
        MouseButton::X2 => (MOUSEEVENTF_XUP, 2u32),
    };
    send_mouse_input(flags, data);
}

/// Single click (press + release) of a button.
pub fn mouse_click(button: MouseButton) {
    mouse_down(button);
    std::thread::sleep(std::time::Duration::from_millis(1));
    mouse_up(button);
}

/// Scroll wheel delta. Positive `delta_y` = down (toward user).
pub fn scroll_wheel(delta_x: i32, delta_y: i32) {
    if delta_y != 0 {
        send_mouse_input(MOUSEEVENTF_WHEEL, delta_y as u32);
    }
    if delta_x != 0 {
        send_mouse_input(MOUSEEVENTF_HWHEEL, delta_x as u32);
    }
}

/// Send a synthesized keyboard event.
/// `vk` is the Win32 Virtual-Key code. `is_up = true` → release, else press.
#[allow(clippy::too_many_arguments)]
pub fn send_key(vk: u16, ctrl: bool, alt: bool, shift: bool, win: bool, is_up: bool) {
    let mods: [(bool, u16); 4] = [
        (ctrl, 0xA2),  // VK_LCONTROL
        (shift, 0xA0), // VK_LSHIFT
        (alt, 0xA4),   // VK_LMENU
        (win, 0x5B),   // VK_LWIN
    ];
    for (pressed, vk_mod) in mods.iter() {
        if !*pressed {
            continue;
        }
        send_keyboard_input(*vk_mod, false);
    }
    send_keyboard_input(vk, is_up);
    for (pressed, vk_mod) in mods.iter().rev() {
        if !*pressed {
            continue;
        }
        send_keyboard_input(*vk_mod, true);
    }
}

fn send_mouse_input(
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    data: u32,
) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SendInput(
            &[input],
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

fn send_keyboard_input(vk: u16, is_up: bool) {
    let flags: KEYBD_EVENT_FLAGS = if is_up {
        KEYEVENTF_KEYUP
    } else {
        KEYEVENTF_EXTENDEDKEY
    };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: windows::Win32::UI::Input::KeyboardAndMouse::KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SendInput(
            &[input],
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}
