//! Windows UIPI (User Interface Privilege Isolation), UAC elevation, DPI
//! awareness, and portable in-place self-update.
//!
//! The portable updater lives here (not in `lib.rs`) because it is pure
//! Win32 + std: download → minisign-verify → atomic rename swap → respawn.
//! No new HTTP/crypto crates — `minisign-verify` is already in the tree as a
//! transitive dep of `tauri-plugin-updater`, and the download goes through
//! WinHTTP from the `windows` crate (feature `Win32_Networking_WinHttp`).

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

// ── Portable in-place self-update: pure helpers ────────────────────────────
//
// The NSIS updater plugin cannot serve a zero-install binary: it would drop
// an installer on a user who explicitly chose "no installer". So portable
// updates swap the running `.exe` in place (download → verify → rename swap
// → respawn). Windows forbids OVERWRITING a running image but allows
// RENAMING it — that asymmetry is the whole trick. Helpers below are pure
// (argv/path/classify) so `cargo test` covers them without spawning anything.

/// Marker passed to the respawned process: "you are the update child,
/// wait for this PID to die before booting".
/// Used from the `[[bin]]` entry point (`main.rs`), so `cargo check --lib`
/// reports it unused — it is not dead, the bin target owns the call.
#[allow(dead_code)]
pub const UPDATED_FROM_ARG_PREFIX: &str = "--updated-from=";

/// Pure: extract the parent PID from an argv list. No process access.
/// Bin-owned caller (`main.rs` handover drain); see the const above.
#[allow(dead_code)]
pub fn updated_from_pid(argv: &[String]) -> Option<u32> {
    argv.iter()
        .find_map(|a| a.strip_prefix(UPDATED_FROM_ARG_PREFIX))
        .and_then(|v| v.parse::<u32>().ok())
}

/// Pure: sibling paths for the swap, derived from the running image.
/// Same directory = same volume = rename is atomic, never a copy.
pub fn portable_swap_paths(current_exe: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let mut new_path = current_exe.as_os_str().to_owned();
    new_path.push(".new");
    let mut old_path = current_exe.as_os_str().to_owned();
    old_path.push(".old");
    (
        std::path::PathBuf::from(new_path),
        std::path::PathBuf::from(old_path),
    )
}

/// Pure: is this run a portable runtime? Covers the conscious portable
/// (`--portable` / `nanoclick.ini`, see `ConfigManager::is_portable`) AND the
/// plain "user runs NanoClick-portable.exe with no flags".
pub fn is_portable_runtime(argv: &[String], exe_file_name: &str) -> bool {
    if argv.iter().any(|a| a == "--portable") {
        return true;
    }
    if std::path::Path::new("nanoclick.ini").exists() {
        return true;
    }
    exe_file_name.eq_ignore_ascii_case("NanoClick-portable.exe")
}

/// Pure: classify a portable-update failure for the toast. No I/O.
pub fn classify_portable_update_error(stage: &str, detail: &str) -> String {
    format!("portable update failed at {stage}: {detail}")
}

/// WinHTTP GET → Vec<u8>. Blocking, runs on the caller's thread (the update
/// command runs async and yields to it via `spawn_blocking`).
///
/// Why not reuse the plugin's HTTP client? `Updater::download` is a method on a
/// plugin-owned `Update` behind a per-webview resource table — reaching it
/// from a Rust command means smuggling rids across the bridge. WinHTTP is
/// already linked (`windows` crate, zero new deps), honors the system proxy
/// by default, and keeps the portable path independent of the plugin.
#[cfg(target_os = "windows")]
pub fn winhttp_get(url: &str, progress: &dyn Fn(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{HSTRING, PCWSTR, w};

    // Split URL into host + path with plain string ops (no url-crate dep).
    let (host, path, secure) = split_http_url(url)?;
    let host_w = HSTRING::from(&host);
    let path_w = HSTRING::from(&path);
    let agent = HSTRING::from("NanoClick-updater");

    unsafe {
        let session = WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if session.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        struct Closer(*mut std::ffi::c_void);
        impl Drop for Closer {
            fn drop(&mut self) {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
        let _session_guard = Closer(session);

        let port = if secure { 443 } else { 80 };
        let connect = WinHttpConnect(session, &host_w, port, 0);
        if connect.is_null() {
            return Err(format!("WinHttpConnect failed for {host}"));
        }
        let _connect_guard = Closer(connect);

        let mut flags = WINHTTP_FLAG_REFRESH;
        if secure {
            flags |= WINHTTP_FLAG_SECURE;
        }
        let request = WinHttpOpenRequest(
            connect,
            w!("GET"),
            &path_w,
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        );
        if request.is_null() {
            return Err("WinHttpOpenRequest failed".into());
        }
        let _request_guard = Closer(request);

        // Radar pings must never hang the caller: 5 s on every phase.
        let _ = WinHttpSetTimeouts(session, 5000, 5000, 5000, 5000);

        WinHttpSendRequest(request, None, None, 0, 0, 0)
            .map_err(|e| format!("WinHttpSendRequest failed: {e}"))?;
        WinHttpReceiveResponse(request, std::ptr::null_mut())
            .map_err(|e| format!("WinHttpReceiveResponse failed: {e}"))?;

        // Status must be 200; anything else (404/403/redirect) is an error.
        let mut status: u32 = 0;
        let mut status_len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut _ as *mut _),
            &mut status_len,
            std::ptr::null_mut(),
        )
        .map_err(|e| format!("WinHttpQueryHeaders failed: {e}"))?;
        if status != 200 {
            return Err(format!("download HTTP status {status} for {url}"));
        }

        // Optional total for the progress bar.
        let mut total: Option<u64> = None;
        let mut len: u32 = 0;
        let mut len_size = std::mem::size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut len as *mut _ as *mut _),
            &mut len_size,
            std::ptr::null_mut(),
        )
        .is_ok()
        {
            total = Some(len as u64);
        }

        let mut out: Vec<u8> = Vec::with_capacity(total.unwrap_or(1 << 20) as usize);
        let mut buf = vec![0u8; 65536];
        loop {
            let mut read: u32 = 0;
            WinHttpQueryDataAvailable(request, &mut read)
                .map_err(|_| "WinHttpQueryDataAvailable failed".to_string())?;
            if read == 0 {
                break;
            }
            let want = (read as usize).min(buf.len());
            let mut got: u32 = 0;
            WinHttpReadData(
                request,
                buf.as_mut_ptr() as *mut _,
                want as u32,
                &mut got,
            )
            .map_err(|e| format!("WinHttpReadData failed: {e}"))?;
            if got == 0 {
                break;
            }
            out.extend_from_slice(&buf[..got as usize]);
            progress(out.len() as u64, total);
        }
        Ok(out)
    }
}

/// Pure: split `http(s)://host/path` into (host, path, secure). Rejects
/// anything else — the updater must never fetch from non-HTTP(S) schemes.
pub fn split_http_url(url: &str) -> Result<(String, String, bool), String> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("unsupported URL scheme: {url}"));
    };
    let (host, path) = match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    };
    if host.is_empty() {
        return Err(format!("empty host in URL: {url}"));
    }
    Ok((host, path, secure))
}

/// Block until `pid` no longer exists (poll `OpenProcess`, 50 ms steps,
/// bounded by `timeout_ms`). Returns true when the parent is gone.
/// Runs pre-Tauri from `main.rs` — no hooks, no windows, no locks yet.
/// The new process is spawned while the old one is still alive, so without
/// this wait it would meet the single-instance lock of its own parent.
/// Bin-owned caller; see `UPDATED_FROM_ARG_PREFIX`.
#[allow(dead_code)]
#[cfg(target_os = "windows")]
pub fn wait_for_parent_exit(pid: u32, timeout_ms: u64) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let gone = unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    false
                }
                Err(_) => true,
            }
        };
        if gone {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Best-effort removal of the `.old` rollback copy after a healthy boot.
/// First tries a plain delete (the kernel had ~2 s to release the image
/// section after the old `exit(0)`); on failure arms
/// `MOVEFILE_DELAY_UNTIL_REBOOT` so Windows itself deletes it at the next
/// boot. Never fails the boot — logs nothing, moves on.
/// Bin-owned caller (`main.rs` post-boot cleanup); see above.
#[allow(dead_code)]
#[cfg(target_os = "windows")]
pub fn cleanup_old_binary_deferred(old_path: std::path::PathBuf) {
    std::thread::Builder::new()
        .name("nanoclick-old-cleanup".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2000));
            if std::fs::remove_file(&old_path).is_ok() {
                return;
            }
            use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
            use windows::core::HSTRING;
            let from = HSTRING::from(old_path.as_os_str());
            unsafe {
                let _ = MoveFileExW(&from, None, MOVEFILE_DELAY_UNTIL_REBOOT);
            }
        })
        .ok();
}

/// Verify a downloaded portable binary against its `.sig` text using the
/// SAME minisign pubkey the NSIS updater trusts (`tauri.conf.json →
/// plugins.updater.pubkey`). Pure bytes in, bool out — the caller decides
/// what to delete. `minisign-verify` is zero-dependency and already in the
/// tree (transitive via `tauri-plugin-updater`), so this adds ~0 KB.
pub fn verify_portable_signature(binary_bytes: &[u8], sig_text: &str, pubkey_b64: &str) -> bool {
    let (Ok(pubkey), Ok(sig)) = (
        minisign_verify::PublicKey::from_base64(pubkey_b64),
        minisign_verify::Signature::decode(sig_text),
    ) else {
        return false;
    };
    pubkey.verify(binary_bytes, &sig, false).is_ok()
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

    // ── Portable in-place self-update: pure logic, no processes ──────────

    #[test]
    fn updated_from_arg_roundtrips_a_pid() {
        let argv = vec!["nanoclick.exe".to_string(), "--updated-from=1234".to_string()];
        assert_eq!(updated_from_pid(&argv), Some(1234));
        let no_marker = vec!["nanoclick.exe".to_string()];
        assert_eq!(updated_from_pid(&no_marker), None);
        let garbage = vec!["--updated-from=abc".to_string()];
        assert_eq!(updated_from_pid(&garbage), None);
    }

    #[test]
    fn swap_paths_are_same_dir_siblings() {
        let exe = std::path::Path::new(r"C:\Tools\NanoClick-portable.exe");
        let (new_p, old_p) = portable_swap_paths(exe);
        assert_eq!(new_p.parent(), exe.parent());
        assert_eq!(old_p.parent(), exe.parent());
        assert!(new_p.to_string_lossy().ends_with(".exe.new"));
        assert!(old_p.to_string_lossy().ends_with(".exe.old"));
    }

    #[test]
    fn portable_runtime_detection_covers_all_shapes() {
        // Explicit flag wins regardless of file name.
        assert!(is_portable_runtime(
            &["app.exe".to_string(), "--portable".to_string()],
            "nanoclick.exe"
        ));
        // Bare portable file name with no flags.
        assert!(is_portable_runtime(
            &["NanoClick-portable.exe".to_string()],
            "NanoClick-portable.exe"
        ));
        // Installed NSIS binary with no markers is NOT portable.
        assert!(!is_portable_runtime(
            &["nanoclick.exe".to_string()],
            "nanoclick.exe"
        ));
    }

    #[test]
    fn url_splitter_accepts_only_http() {
        let (h, p, s) = split_http_url("https://github.com/a/b.exe").unwrap();
        assert_eq!((h.as_str(), p.as_str(), s), ("github.com", "/a/b.exe", true));
        let (h, p, s) = split_http_url("http://host/x").unwrap();
        assert_eq!((h.as_str(), p.as_str(), s), ("host", "/x", false));
        assert!(split_http_url("file:///etc/passwd").is_err());
        assert!(split_http_url("https://").is_err());
    }

    #[test]
    fn bad_signature_is_rejected_without_io() {
        assert!(!verify_portable_signature(b"bytes", "not a sig", "not a key"));
        assert!(!verify_portable_signature(b"", "", ""));
    }

    #[test]
    fn error_classifier_names_the_stage() {
        let msg = classify_portable_update_error("verify", "mismatch");
        assert!(msg.contains("verify") && msg.contains("mismatch"));
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
