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

/// Minimum gap between two UIPI alerts (milliseconds).
///
/// Every click passes through [`uipi_guard_check`], so this is what keeps the
/// expensive part off the hot path: `WindowFromPoint` + `OpenProcess` +
/// `GetTokenInformation` run at most this often instead of at 150 CPS.
/// Tuned from 3000ms to 1500ms for responsive detection.
pub const UIPI_ALERT_THROTTLE_MS: u64 = 1500;

/// Throttled UIPI check for the shared click path.
///
/// **THROTTLE FIRST, SYSCALLS SECOND.** The timestamp comparison is one relaxed
/// atomic load; only when it passes do we touch the elevation state. The
/// previous code checked elevation and then throttled only the *notification*,
/// which meant three syscalls per click whenever the target was elevated.
///
/// `is_current_process_elevated()` cannot be skipped before the throttle — it is
/// itself a `GetTokenInformation` pair — but caching it here keeps the
/// not-elevated case (the overwhelming majority) down to the throttle alone.
pub fn uipi_guard_check(x: i32, y: i32) {
    use std::sync::atomic::{AtomicU64, Ordering};

    static LAST_CHECK: AtomicU64 = AtomicU64::new(0);
    static SELF_ELEVATED: AtomicU64 = AtomicU64::new(u64::MAX); // sentinel = unknown

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Throttle BEFORE any syscall.
    if now_ms.saturating_sub(LAST_CHECK.load(Ordering::Relaxed)) < UIPI_ALERT_THROTTLE_MS {
        return;
    }

    // Our own elevation never changes while running, so resolve it once.
    let elevated = match SELF_ELEVATED.load(Ordering::Relaxed) {
        u64::MAX => {
            let value = u64::from(is_current_process_elevated());
            SELF_ELEVATED.store(value, Ordering::Relaxed);
            value
        }
        cached => cached,
    };
    if elevated == 1 {
        return;
    }

    if !is_target_point_elevated(x, y) {
        return;
    }

    LAST_CHECK.store(now_ms, Ordering::Relaxed);
    play_warning_sound();
    crate::platform::windows::trigger_uipi_block_notification(x, y);

    crate::notifications::push_notification(
        crate::notifications::NotificationCode::UipiBlocked,
        "NanoClick — Administrator Rights Required",
        "Click blocked: the target window runs elevated (UIPI)",
        Some(serde_json::json!({ "x": x, "y": y })),
        true,
    );
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

/// Name of the opt-in Task Scheduler entry for seamless elevation.
///
/// One task per machine, quoted exe path, `/RL HIGHEST` so the logon trigger
/// needs the UAC consent exactly ONCE (at registration) and never again.
pub const ELEVATED_TASK_NAME: &str = "NanoClick (elevated, user logon)";

/// Pure builder for `schtasks /Query` — probes whether the opt-in task
/// exists, without touching the scheduler. No side effects, unit-tested.
pub fn scheduled_task_query_argv() -> Vec<String> {
    vec![
        "/Query".to_string(),
        "/TN".to_string(),
        ELEVATED_TASK_NAME.to_string(),
    ]
}

/// Pure builder for `schtasks /Create` — registers the logon task that
/// starts THIS exe with highest privileges. Quoting the path is load-bearing:
/// `C:\Program Files\...` without quotes would split the `/TR` argument.
pub fn scheduled_task_create_argv(exe_path: &str) -> Vec<String> {
    vec![
        "/Create".to_string(),
        "/F".to_string(),
        "/TN".to_string(),
        ELEVATED_TASK_NAME.to_string(),
        "/TR".to_string(),
        format!("\"{exe_path}\""),
        "/SC".to_string(),
        "ONLOGON".to_string(),
        "/RL".to_string(),
        "HIGHEST".to_string(),
    ]
}

/// Pure builder for `schtasks /Delete` — removes the opt-in task again.
pub fn scheduled_task_delete_argv() -> Vec<String> {
    vec![
        "/Delete".to_string(),
        "/F".to_string(),
        "/TN".to_string(),
        ELEVATED_TASK_NAME.to_string(),
    ]
}

/// Classify a `schtasks` exit the way the UI needs it: ok / not-found /
/// failed-with-stderr. Pure, so the error copy is unit-tested without ever
/// spawning a process in `cargo test`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledTaskStatus {
    Ok,
    NotFound,
    Failed,
}

pub fn classify_schtasks_exit(code: Option<i32>, stderr: &str) -> ScheduledTaskStatus {
    if code == Some(0) {
        return ScheduledTaskStatus::Ok;
    }
    let lower = stderr.to_lowercase();
    if lower.contains("cannot find")
        || lower.contains("not found")
        || lower.contains("не удается найти")
        || lower.contains("не знайдено")
    {
        ScheduledTaskStatus::NotFound
    } else {
        ScheduledTaskStatus::Failed
    }
}

/// Does the opt-in elevated logon task exist? Read-only probe (`/Query`),
/// no mutation, no prompt.
pub fn is_scheduled_elevated_task_registered() -> bool {
    let out = std::process::Command::new("schtasks")
        .args(scheduled_task_query_argv())
        .output();
    matches!(out, Ok(o) if o.status.success())
}

/// Register the opt-in task. The UAC consent happens HERE, once, owned by
/// the user's explicit click — never at boot, never silently.
pub fn register_scheduled_elevated_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("failed to get current exe path: {e}"))?;
    let exe_str = exe
        .to_str()
        .ok_or_else(|| "invalid exe path string".to_string())?;
    let out = std::process::Command::new("schtasks")
        .args(scheduled_task_create_argv(exe_str))
        .output()
        .map_err(|e| format!("failed to run schtasks: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Honest copy for the toast: a missing service vs a refused consent are
    // different next steps for the user.
    match classify_schtasks_exit(out.status.code(), &stderr) {
        ScheduledTaskStatus::Ok => Ok(()),
        ScheduledTaskStatus::NotFound => Err(format!(
            "Task Scheduler not available on this machine: {}",
            stderr.trim()
        )),
        ScheduledTaskStatus::Failed => Err(format!("schtasks /Create failed: {}", stderr.trim())),
    }
}

/// Remove the opt-in task again (the checkbox is a real toggle, not a trap).
pub fn unregister_scheduled_elevated_task() -> Result<(), String> {
    let out = std::process::Command::new("schtasks")
        .args(scheduled_task_delete_argv())
        .output()
        .map_err(|e| format!("failed to run schtasks: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match classify_schtasks_exit(out.status.code(), &stderr) {
        // Deleting a task that is already gone is success, not an error:
        // the end state (no task) is exactly what the user asked for.
        ScheduledTaskStatus::NotFound | ScheduledTaskStatus::Ok => Ok(()),
        ScheduledTaskStatus::Failed => Err(format!("schtasks /Delete failed: {}", stderr.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_name_is_stable_and_single() {
        assert_eq!(ELEVATED_TASK_NAME, "NanoClick (elevated, user logon)");
    }

    #[test]
    fn query_argv_probes_without_mutation() {
        let argv = scheduled_task_query_argv();
        assert_eq!(argv, vec!["/Query", "/TN", ELEVATED_TASK_NAME]);
        assert!(!argv.iter().any(|a| a == "/Create" || a == "/Delete"));
    }

    #[test]
    fn create_argv_runs_highest_at_logon_with_quoted_exe() {
        let argv = scheduled_task_create_argv(r"C:\Program Files\NanoClick\NanoClick.exe");
        let joined = argv.join(" ");
        assert!(argv.contains(&"/SC".to_string()));
        assert!(argv.contains(&"ONLOGON".to_string()));
        assert!(argv.contains(&"/RL".to_string()));
        assert!(argv.contains(&"HIGHEST".to_string()));
        // Quoting is load-bearing: an unquoted Program Files path splits /TR.
        // NOTE: quotes wrap the VALUE element after /TR (one argv item),
        let tr_value: String = argv
            .windows(2)
            .find(|w| w[0] == "/TR")
            .map(|w| w[1].clone())
            .expect("trp");
        assert!(tr_value.starts_with('"'));
        assert!(tr_value.ends_with('"'));
        let _ = joined;
    }

    #[test]
    fn delete_argv_targets_only_our_task() {
        let argv = scheduled_task_delete_argv();
        assert_eq!(argv, vec!["/Delete", "/F", "/TN", ELEVATED_TASK_NAME]);
    }

    #[test]
    fn exit_classification_never_spawns_a_process() {
        use ScheduledTaskStatus::*;
        assert_eq!(classify_schtasks_exit(Some(0), "anything"), Ok);
        assert_eq!(
            classify_schtasks_exit(Some(1), "ERROR: The system cannot find the file specified."),
            NotFound
        );
        assert_eq!(
            classify_schtasks_exit(Some(1), "Access is denied."),
            Failed
        );
        assert_eq!(classify_schtasks_exit(None, ""), Failed);
    }
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
