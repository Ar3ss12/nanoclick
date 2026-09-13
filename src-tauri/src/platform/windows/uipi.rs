//! Windows UIPI (User Interface Privilege Isolation), UAC elevation, and DPI awareness.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT,
};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
use winreg::RegKey;

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: isize,
        lpoperation: *const u16,
        lpfile: *const u16,
        lpparameters: *const u16,
        lpdirectory: *const u16,
        nshowcmd: i32,
    ) -> isize;
}

#[link(name = "user32")]
extern "system" {
    fn SetProcessDpiAwarenessContext(value: isize) -> i32;
    fn MessageBeep(uType: u32) -> i32;
}

const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
const MB_ICONWARNING: u32 = 0x00000030;
const SW_SHOW: i32 = 5;

const APPCOMPAT_LAYERS_PATH: &str =
    r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

/// Check if the current NanoClick process is running with elevated (Administrator) privileges.
pub fn is_current_process_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION {
            TokenIsElevated: 0,
        };
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size,
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Check if a given window handle belongs to an elevated (Administrator) process.
pub fn is_window_elevated(hwnd: HWND) -> bool {
    if hwnd.0 == 0 {
        return false;
    }
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return false;
        }

        let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => h,
            Err(_) => {
                // If OpenProcess fails with ACCESS_DENIED from a non-elevated process,
                // it is almost certainly a High/System integrity or protected process!
                return true;
            }
        };

        let mut token = HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(process);
            // If opening token fails from standard user, it is elevated or protected.
            return true;
        }

        let mut elevation = TOKEN_ELEVATION {
            TokenIsElevated: 0,
        };
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size,
            &mut size,
        )
        .is_ok();

        let _ = CloseHandle(token);
        let _ = CloseHandle(process);

        ok && elevation.TokenIsElevated != 0
    }
}

/// Check if the target screen point (x, y) resides over a window of an elevated process.
pub fn is_target_point_elevated(x: i32, y: i32) -> bool {
    unsafe {
        let pt = POINT { x, y };
        let hwnd = WindowFromPoint(pt);
        if hwnd.0 == 0 {
            return false;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let target = if root.0 != 0 { root } else { hwnd };
        is_window_elevated(target)
    }
}

/// Restart the application with elevated administrator rights via UAC prompt ("runas").
pub fn restart_as_admin() -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("failed to get current exe path: {e}"))?;
    let exe_path_str = current_exe
        .to_str()
        .ok_or_else(|| "invalid exe path string".to_string())?;
    let exe_wide: Vec<u16> = exe_path_str
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let runas_wide: Vec<u16> = "runas\0".encode_utf16().collect();

    unsafe {
        let hinstance = ShellExecuteW(
            0,
            runas_wide.as_ptr(),
            exe_wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOW,
        );
        // ShellExecute returns HINSTANCE > 32 on success
        if hinstance > 32 {
            std::process::exit(0);
        } else {
            Err(format!(
                "ShellExecute runas failed with code: {}",
                hinstance
            ))
        }
    }
}

/// Configure Windows AppCompat registry to always run this executable as administrator.
pub fn set_always_run_as_admin(enabled: bool) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("failed to get current exe path: {e}"))?;
    let exe_str = current_exe.to_string_lossy().to_string();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (layers_key, _) = hkcu
        .create_subkey(APPCOMPAT_LAYERS_PATH)
        .map_err(|e| format!("failed to open AppCompatFlags\\Layers: {e}"))?;

    if enabled {
        layers_key
            .set_value(&exe_str, &"~ RUNASADMIN")
            .map_err(|e| format!("failed to write registry value: {e}"))?;
    } else {
        let _ = layers_key.delete_value(&exe_str);
    }
    Ok(())
}

/// Check if Windows AppCompat registry is set to always launch this executable as administrator.
pub fn is_always_run_as_admin() -> bool {
    if let Ok(current_exe) = std::env::current_exe() {
        let exe_str = current_exe.to_string_lossy().to_string();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(layers_key) = hkcu.open_subkey_with_flags(APPCOMPAT_LAYERS_PATH, KEY_READ) {
            if let Ok(val) = layers_key.get_value::<String, _>(&exe_str) {
                return val.contains("RUNASADMIN");
            }
        }
    }
    false
}

/// Play a standard Windows alert/warning chime when UIPI prevents action.
pub fn play_warning_sound() {
    unsafe {
        let _ = MessageBeep(MB_ICONWARNING);
    }
}

/// Initialize Per-Monitor V2 DPI awareness programmatically at startup.
pub fn init_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
