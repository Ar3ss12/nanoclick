//! Stage D single-instance: the DAEMON owns the mutex, not the window.
//! Second UI launch forwards to the daemon (no second engine, ever).
//! Win32 `CreateMutexW` via the `windows` crate; non-Windows = file lock stub.
pub const DAEMON_MUTEX_NAME: &str = "Local\\NanoClickDaemonSingleInstance";
/// Try to become the daemon. `Ok(guard)` = we are the daemon;
/// `Err(_)` = a daemon already runs (forward + exit).
pub fn try_become_daemon() -> Result<DaemonGuard, String> {
    #[cfg(target_os = "windows")]
    {
        use windows::core::w;
        use windows::Win32::Foundation::{CloseHandle, GetLastError};
        use windows::Win32::System::Threading::CreateMutexW;
        unsafe {
            // CreateMutexW -> Result<HANDLE>. Second daemon = ERROR_ALREADY_EXISTS:
            // the call still succeeds with an opened handle, so GetLastError()
            // returning Err(ALREADY_EXISTS) means "forward + exit".
            match CreateMutexW(None, false, w!("Local\\NanoClickDaemonSingleInstance")) {
                Ok(h) => {
                    if GetLastError().is_err() {
                        let _ = CloseHandle(h);
                        return Err("another daemon already runs".into());
                    }
                    // HANDLE is Copy: just keep the raw value in the guard.
                    Ok(DaemonGuard { handle: h.0 })
                }
                Err(e) => Err(format!("mutex failed: {e}")),
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(DaemonGuard { handle: 0 })
    }
}
/// Held for the daemon lifetime; drop = release.
pub struct DaemonGuard {
    handle: isize,
}
impl Drop for DaemonGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::Foundation::{CloseHandle, HANDLE};
            if self.handle != 0 {
                let _ = CloseHandle(HANDLE(self.handle as _));
            }
        }
    }
}
