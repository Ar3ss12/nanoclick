//! Native Win32 system tray icon — deliberately **no `tray-icon` / `muda` crate**.
//!
//! Why hand-rolled instead of tauri's `tray-icon` feature:
//! * `tray-icon` pulls `muda`, whose Windows menu code links
//!   `comctl32.dll!TaskDialogIndirect` whenever `tauri/common-controls-v6` is
//!   on. That symbol exists only in comctl32 **v6**, which an application
//!   manifest must activate; a binary without one dies at load with
//!   `0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND` (AGENTS.md §0,
//!   `test_tauri_common_controls_v6_stays_disabled`).
//! * A tray icon needs three calls: `Shell_NotifyIconW`, a hidden window and a
//!   popup menu. Cheaper than a dependency, and it cannot drag that comctl32
//!   story back in.
//!
//! Design notes (all deliberate):
//! * Hidden **top-level** window (`WS_POPUP` + `WS_EX_TOOLWINDOW`), *not*
//!   `HWND_MESSAGE`: only a real top-level window receives the broadcast
//!   `TaskbarCreated`, which is how the icon reappears after `explorer.exe`
//!   restarts. A message-only window loses the icon until the next logon.
//! * Own thread with a blocking `GetMessageW` pump: zero CPU when idle, and
//!   nothing here sits on the click/hook hot path. Window work is handed to the
//!   Tauri event loop through `AppHandle::run_on_main_thread`.
//! * No file logging inside the message pump (same rule as the LL-hook loop)
//!   and no `unwrap()`: a missing icon must never take the backend down.
//! * The icon comes from the resource this binary embeds (`build.rs` → `winres`
//!   → id 32512) with a system-icon fallback — the lib unittest target carries
//!   no resources at all, so a failed load must stay non-fatal.

use tauri::AppHandle;

/// Tray menu command ids. Unique on purpose (`menu_ids_are_unique`).
pub const MENU_ID_OPEN: u32 = 1;
pub const MENU_ID_TOGGLE_CLICKING: u32 = 2;
pub const MENU_ID_TOGGLE_MODE: u32 = 5;
pub const MENU_ID_TOGGLE_HUD: u32 = 3;
pub const MENU_ID_RELOAD_UI: u32 = 6;
pub const MENU_ID_RESTART_APP: u32 = 7;
pub const MENU_ID_QUIT: u32 = 4;

/// `WM_APP` — the base of the custom shell callback message.
pub const WM_APP_BASE: u32 = 0x8000;
/// Custom callback message the shell posts to our hidden window.
pub const WM_TRAY_CALLBACK: u32 = WM_APP_BASE + 1;

// Shell notification codes (NOTIFYICON_VERSION_4 semantics). Mirrored as plain
// numbers so the dispatch table is testable on any platform;
// `codes_match_the_windows_crate` pins them to the crate's values on Windows.
pub const TRAY_EVENT_LBUTTONUP: u32 = 0x0202;
pub const TRAY_EVENT_LBUTTONDBLCLK: u32 = 0x0203;
pub const TRAY_EVENT_RBUTTONUP: u32 = 0x0205;
pub const TRAY_EVENT_CONTEXTMENU: u32 = 0x007B;
pub const TRAY_EVENT_SELECT: u32 = 1024; // NIN_SELECT

/// What a tray interaction asks the backend to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    ToggleClicking,
    /// Work Mode <-> Autoclicker. The only way to switch modes while the UI is
    /// asleep in the tray — with deep sleep the page does not exist, so neither
    /// the mode badge nor its hotkey feedback is available.
    ToggleMode,
    ToggleHud,
    /// Reload the interface: the escape hatch when a page came up broken and the
    /// watchdog already spent its single reload. Zero windows → rebuild instead.
    ReloadUi,
    /// Full process restart (`AppHandle::restart`) — the honest way out of a
    /// wedged backend that a page reload cannot fix.
    RestartApp,
    Quit,
}

/// Pure mapping: menu id -> action. An unknown id (a future menu entry, a stray
/// `WM_COMMAND` from another control) maps to `None` and is ignored.
pub fn tray_action_for_command(id: u32) -> Option<TrayAction> {
    match id {
        MENU_ID_OPEN => Some(TrayAction::Open),
        MENU_ID_TOGGLE_CLICKING => Some(TrayAction::ToggleClicking),
        MENU_ID_TOGGLE_MODE => Some(TrayAction::ToggleMode),
        MENU_ID_TOGGLE_HUD => Some(TrayAction::ToggleHud),
        MENU_ID_RELOAD_UI => Some(TrayAction::ReloadUi),
        MENU_ID_RESTART_APP => Some(TrayAction::RestartApp),
        MENU_ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

/// `MF_CHECKED` when `on`, `MF_UNCHECKED` otherwise — both OR-ed with
/// `MF_BYCOMMAND`, which is 0 (kept for readability).
///
/// Plain numbers on purpose, like the tray event codes above: the dispatch
/// tables stay testable on any platform, and `CheckMenuItem` takes a raw u32.
fn check_flags(on: bool) -> u32 {
    const MF_CHECKED_BIT: u32 = 0x0000_0008;
    const MF_BYCOMMAND_0: u32 = 0x0000_0000;
    MF_BYCOMMAND_0 | if on { MF_CHECKED_BIT } else { 0 }
}

/// Menu flags for the mode item: checked while **Work Mode** is on.
///
/// A check mark is the only state the tray can show while the UI is asleep, so
/// it is refreshed every time the menu opens (see `win::show_menu`).
pub fn mode_item_flags(work_mode: bool) -> u32 {
    check_flags(work_mode)
}

/// Menu flags for the clicking item: checked while the clicker runs.
///
/// Same reason as the mode item — with the window destroyed there is no other
/// place to see whether NanoClick is currently clicking.
pub fn clicking_item_flags(running: bool) -> u32 {
    check_flags(running)
}

/// Tooltip text: mode + clicker state. Refreshed together with the check marks.
pub fn tip_text(work_mode: bool, running: bool) -> String {
    let mode = if work_mode { "Work Mode" } else { "Autoclicker" };
    let state = if running { "clicking" } else { "idle" };
    format!("NanoClick — {mode} · {state}")
}

/// Balloon tip for actions that happen with **zero windows alive** (deep sleep).
///
/// A `emit` into the WebView is the normal feedback path, but there is no
/// WebView to emit into while the app sleeps in the tray — a vetoed start or a
/// mode switch would leave no trace at all. Safe to call from any thread: it
/// uses a copy of the icon data recorded by the tray thread, never the pump's
/// live state pointer.
pub(crate) fn notify_balloon(title: &str, msg: &str) {
    #[cfg(target_os = "windows")]
    {
        win::notify_balloon(title, msg);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (title, msg);
    }
}

/// What a shell notification code means to the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayClick {
    /// Left click / double click / keyboard select: bring the window back.
    Open,
    /// Right click: pop up the context menu.
    Menu,
    /// Anything else (balloon events, hover): ignore.
    Ignore,
}

/// Pure mapping: shell notification code -> click intent.
pub fn tray_click_for_event(code: u32) -> TrayClick {
    match code {
        TRAY_EVENT_LBUTTONUP | TRAY_EVENT_LBUTTONDBLCLK | TRAY_EVENT_SELECT => TrayClick::Open,
        TRAY_EVENT_RBUTTONUP | TRAY_EVENT_CONTEXTMENU => TrayClick::Menu,
        _ => TrayClick::Ignore,
    }
}

/// Perform an action. Called on the Tauri main thread only — never from the
/// tray pump thread.
pub fn perform(action: TrayAction, app: &AppHandle) {
    match action {
        TrayAction::Open => crate::restore_main_window(app),
        TrayAction::ToggleClicking => crate::toggle_clicking_from_tray(app),
        TrayAction::ToggleMode => crate::toggle_mode_from_tray(app),
        TrayAction::ToggleHud => crate::toggle_hud_from_tray(app),
        TrayAction::ReloadUi => crate::reload_interface_from_tray(app),
        TrayAction::RestartApp => crate::restart_app_from_tray(app),
        TrayAction::Quit => crate::shutdown_application(app),
    }
}

/// Tooltip text. Kept under 64 UTF-16 units: Explorer truncates the tip there.
pub(crate) const TRAY_TIP: &str = "NanoClick — clicker stays in the background";

/// Copy a Rust string into a fixed-size UTF-16 buffer, always NUL-terminated.
/// Never panics: a too-long tip is truncated instead of overflowing (a Win32
/// struct overflow here would be a memory-corruption bug, not a cosmetic one).
pub(crate) fn fill_wide(dst: &mut [u16], src: &str) {
    if dst.is_empty() {
        return;
    }
    let mut n = 0usize;
    for unit in src.encode_utf16() {
        if n + 1 >= dst.len() {
            break;
        }
        dst[n] = unit;
        n += 1;
    }
    dst[n] = 0;
}

/// Live handle to the tray thread. Dropping it removes the icon and stops the
/// thread, so an icon can never outlive the process state that owns it.
pub struct TrayHandle {
    #[cfg(target_os = "windows")]
    hwnd: isize,
    #[cfg(target_os = "windows")]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TrayHandle {
    /// Start the tray. `None` means "no icon" (shell refused, no resources,
    /// ...) — never fatal: the app keeps working as a plain window.
    pub fn spawn(app: AppHandle) -> Option<Self> {
        #[cfg(target_os = "windows")]
        {
            win::spawn(app)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = app;
            None
        }
    }

    /// True while the message window still exists.
    pub fn is_alive(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.hwnd != 0
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    /// Ask the pump to stop and join it. Idempotent, never panics.
    pub fn shutdown(&mut self) {
        #[cfg(target_os = "windows")]
        {
            if self.hwnd != 0 {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        // windows 0.52 handles wrap `isize`, not a pointer.
                        windows::Win32::Foundation::HWND(self.hwnd),
                        windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                }
                self.hwnd = 0;
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::{
        clicking_item_flags, fill_wide, mode_item_flags, perform, tip_text, tray_action_for_command,
        tray_click_for_event, TrayAction, TrayClick, TrayHandle, MENU_ID_OPEN, MENU_ID_QUIT,
        MENU_ID_RELOAD_UI, MENU_ID_RESTART_APP, MENU_ID_TOGGLE_CLICKING, MENU_ID_TOGGLE_HUD,
        MENU_ID_TOGGLE_MODE, TRAY_TIP, WM_TRAY_CALLBACK,
    };
    use std::sync::atomic::{AtomicPtr, Ordering};
    use std::sync::mpsc::{channel, Sender};
    use std::time::Duration;
    use tauri::AppHandle;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::HBRUSH;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD,
        NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CheckMenuItem, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, HCURSOR,
        HICON, HMENU, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
        MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
        RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateMessage, UnregisterClassW, WM_CLOSE,
        WM_DESTROY, WM_ENDSESSION, WM_NULL, WNDCLASS_STYLES, WNDCLASSW, WS_EX_TOOLWINDOW,
        WS_POPUP,
    };

    /// Class name of the invisible message window. It is also written literally
    /// in the `w!(...)` calls below — keep both in sync (a test pins this).
    const CLASS_NAME: &str = "NanoClickTrayWnd";
    /// The single tray icon this process owns.
    const TRAY_UID: u32 = 1;
    /// Icon resource id embedded by `build.rs` → `winres` (`resource.rc:26`).
    /// `MAKEINTRESOURCEW` is the id cast to a pointer, and 32512 fits in 16 bits.
    const APP_ICON_RESOURCE_ID: u16 = 32512;

    /// The pump owns the single tray state and the wndproc reaches it through
    /// this pointer: set before the window exists, cleared after it is
    /// destroyed, so no callback can ever see freed memory.
    static TRAY_STATE: AtomicPtr<TrayState> = AtomicPtr::new(std::ptr::null_mut());

    struct TrayState {
        app: AppHandle,
        hwnd: HWND,
        nid: NOTIFYICONDATAW,
        menu: HMENU,
        /// `RegisterWindowMessageW("TaskbarCreated")` — broadcast Explorer sends
        /// after it restarts, so the icon can be re-added instead of vanishing.
        taskbar_created: u32,
    }

    pub(super) fn spawn(app: AppHandle) -> Option<TrayHandle> {
        let (tx, rx) = channel::<isize>();
        let thread = std::thread::Builder::new()
            .name("nanoclick-tray".into())
            .spawn(move || run(app, tx))
            .ok()?;
        match rx.recv_timeout(Duration::from_millis(4000)) {
            Ok(hwnd) if hwnd != 0 => Some(TrayHandle {
                hwnd,
                thread: Some(thread),
            }),
            _ => {
                crate::debug_log_internal(
                    "warn",
                    &format!("[Tray] {CLASS_NAME}: icon unavailable, continuing without one"),
                );
                None
            }
        }
    }

    /// Log a Win32 failure at `error` level (release builds drop `info`).
    fn err(msg: &str, e: windows::core::Error) {
        crate::debug_log_internal("error", &format!("[Tray] {msg}: {e}"));
    }

    /// Icon data recorded by the tray thread. A balloon (`NIF_INFO`) is raised
    /// from the MAIN thread — tray actions are dispatched there — and this copy
    /// is what makes that safe: `TRAY_STATE` is the pump's live pointer, cleared
    /// and freed on shutdown, so reading it cross-thread could race with a free.
    static ICON_COPY: std::sync::Mutex<Option<NOTIFYICONDATAW>> = std::sync::Mutex::new(None);

    /// Balloon tip — the only feedback a tray action can give with zero windows
    /// alive (deep sleep). Fire-and-forget: any failure is simply ignored.
    pub(super) fn notify_balloon(title: &str, msg: &str) {
        let Some(mut balloon) = ICON_COPY.lock().ok().and_then(|guard| *guard) else {
            return; // no icon yet (or it was never created)
        };
        // A COPY on purpose: setting NIF_INFO on the stored nid would re-pop the
        // balloon on every `TaskbarCreated` re-add.
        balloon.uFlags = NIF_INFO;
        balloon.dwInfoFlags = NIIF_INFO;
        fill_wide(&mut balloon.szInfo, msg);
        fill_wide(&mut balloon.szInfoTitle, title);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &balloon);
        }
    }

    /// Refresh the tooltip in place (`NIF_TIP`), same copy trick as the balloon.
    /// Called right before the menu opens, so the tip carries the live
    /// mode/clicking state even when the UI is asleep.
    fn update_tip(state: &TrayState, text: &str) {
        let mut tip = state.nid;
        tip.uFlags = NIF_TIP;
        fill_wide(&mut tip.szTip, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &tip);
        }
    }

    /// The tray thread body: build window + icon, then pump messages until the
    /// window is destroyed. Everything is best-effort, nothing unwraps, and the
    /// pump itself does no file I/O.
    fn run(app: AppHandle, ready: Sender<isize>) {
        unsafe {
            let hinstance = match GetModuleHandleW(None) {
                Ok(h) => HINSTANCE(h.0),
                Err(e) => {
                    err("GetModuleHandleW failed", e);
                    let _ = ready.send(0);
                    return;
                }
            };
            let icon = load_tray_icon(hinstance);
            let wc = WNDCLASSW {
                style: WNDCLASS_STYLES(0),
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: icon,
                hCursor: HCURSOR::default(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: w!("NanoClickTrayWnd"),
            };
            if RegisterClassW(&wc) == 0 {
                err("RegisterClassW failed", windows::core::Error::from_win32());
                let _ = ready.send(0);
                return;
            }
            let menu = match CreatePopupMenu() {
                Ok(m) => m,
                Err(e) => {
                    err("CreatePopupMenu failed", e);
                    let _ = UnregisterClassW(w!("NanoClickTrayWnd"), hinstance);
                    let _ = ready.send(0);
                    return;
                }
            };
            build_menu(menu);
            let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));

            let mut state = Box::new(TrayState {
                app,
                hwnd: HWND::default(),
                nid: NOTIFYICONDATAW::default(),
                menu,
                taskbar_created,
            });
            TRAY_STATE.store(&mut *state as *mut TrayState, Ordering::Release);

            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("NanoClickTrayWnd"),
                w!("NanoClick"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                HWND::default(),
                HMENU::default(),
                hinstance,
                None,
            );
            if hwnd.0 == 0 {
                err("CreateWindowExW failed", windows::core::Error::from_win32());
                TRAY_STATE.store(std::ptr::null_mut(), Ordering::Release);
                let _ = UnregisterClassW(w!("NanoClickTrayWnd"), hinstance);
                let _ = ready.send(0);
                return;
            }

            state.hwnd = hwnd;
            state.nid = icon_data(hwnd, icon);
            // Cross-thread copy for balloons/tips (see `ICON_COPY`): the main
            // thread must never dereference the pump's live state pointer.
            if let Ok(mut slot) = ICON_COPY.lock() {
                *slot = Some(state.nid);
            }
            if Shell_NotifyIconW(NIM_ADD, &state.nid).as_bool() {
                // Version 4 semantics: NIN_SELECT for a left click,
                // WM_CONTEXTMENU for a right click.
                let mut ver = state.nid;
                ver.uFlags = NIF_SHOWTIP;
                ver.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &ver);
                crate::debug_log_internal("info", "[Tray] icon added (Shell_NotifyIconW)");
            } else {
                err(
                    "Shell_NotifyIconW(NIM_ADD) failed",
                    windows::core::Error::from_win32(),
                );
            }
            let _ = ready.send(hwnd.0 as isize);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = Shell_NotifyIconW(NIM_DELETE, &state.nid);
            let _ = DestroyWindow(hwnd);
            TRAY_STATE.store(std::ptr::null_mut(), Ordering::Release);
            let _ = UnregisterClassW(w!("NanoClickTrayWnd"), hinstance);
            crate::debug_log_internal("info", "[Tray] icon removed, tray thread stopped");
        }
    }


    /// Load the tray icon. Primary: the icon resource embedded in this binary.
    /// Fallback: a generic system icon — a binary without resources (the lib
    /// unittest target, precisely the `0xC0000139` case) must not fail here.
    unsafe fn load_tray_icon(hinstance: HINSTANCE) -> HICON {
        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        if let Ok(h) = LoadImageW(
            hinstance,
            PCWSTR(APP_ICON_RESOURCE_ID as *const u16),
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        ) {
            if h.0 != 0 {
                return HICON(h.0 as isize);
            }
        }
        match LoadIconW(HINSTANCE::default(), IDI_APPLICATION) {
            Ok(i) => i,
            Err(e) => {
                err("LoadIconW fallback failed", e);
                HICON::default()
            }
        }
    }

    /// `NOTIFYICONDATAW` for our one icon (version 4 callback semantics).
    fn icon_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_UID;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY_CALLBACK;
        nid.hIcon = icon;
        fill_wide(&mut nid.szTip, TRAY_TIP);
        nid
    }

    /// Build the context menu once, for the lifetime of the process: a tray menu
    /// is one small kernel object, and rebuilding it per click would only add
    /// failure paths to the click that the user is waiting on.
    fn build_menu(menu: HMENU) {
        unsafe {
            let _ = AppendMenuW(menu, MF_STRING, MENU_ID_OPEN as usize, w!("Open NanoClick"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_TOGGLE_CLICKING as usize,
                w!("Start / Stop clicking"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_TOGGLE_MODE as usize,
                w!("Work / Autoclicker mode"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_TOGGLE_HUD as usize,
                w!("Show / Hide HUD"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_RELOAD_UI as usize,
                w!("Reload interface"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_RESTART_APP as usize,
                w!("Restart app"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_ID_QUIT as usize, w!("Quit"));
        }
    }

    /// Message pump callback for the invisible window.
    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let ptr = TRAY_STATE.load(Ordering::Acquire);
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let state = &*ptr;
        if msg == WM_TRAY_CALLBACK {
            // Version 4: LOWORD(lParam) carries the notification event.
            match tray_click_for_event((lparam.0 as u32) & 0xFFFF) {
                TrayClick::Open => {
                    // Take the foreground for our process BEFORE the main window
                    // asks for it. `set_focus()` is silently dropped while another
                    // process owns the foreground (the Windows foreground lock),
                    // and that is exactly the tray case: the user clicks our icon
                    // while a game is focused and the window appears BEHIND it.
                    // The shell grants us the right because the click was on our
                    // icon — the same trick `show_menu` uses for the popup.
                    let _ = SetForegroundWindow(state.hwnd);
                    dispatch(state, TrayAction::Open)
                }
                TrayClick::Menu => show_menu(state),
                TrayClick::Ignore => {}
            }
            return LRESULT(0);
        }
        if msg == WM_CLOSE || msg == WM_ENDSESSION {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        // Explorer restarted: without this re-add the icon is gone for good.
        if state.taskbar_created != 0 && msg == state.taskbar_created {
            if Shell_NotifyIconW(NIM_ADD, &state.nid).as_bool() {
                let mut ver = state.nid;
                ver.uFlags = NIF_SHOWTIP;
                ver.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &ver);
            }
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// Hand the action to the Tauri main thread: the tray thread must never
    /// touch windows, and the main thread is where the runtime lives.
    fn dispatch(state: &TrayState, action: TrayAction) {
        // Two bindings on purpose: the closure must move an owned handle, while
        // the method call borrows `self`. One binding would fail to borrow-check.
        let app = state.app.clone();
        let worker = app.clone();
        let _ = app.run_on_main_thread(move || perform(action, &worker));
    }

    /// Pop up the context menu at the cursor and act on the chosen item.
    fn show_menu(state: &TrayState) {
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Refresh the state shown by the menu BEFORE it appears: with the UI
            // asleep (deep sleep) the menu is the only place the current mode and
            // the clicker state can be read, and the user is about to act on them.
            let work_mode = crate::tray_work_mode_active(&state.app);
            let clicking = crate::tray_clicking_active(&state.app);
            let _ = CheckMenuItem(state.menu, MENU_ID_TOGGLE_MODE, mode_item_flags(work_mode));
            let _ = CheckMenuItem(
                state.menu,
                MENU_ID_TOGGLE_CLICKING,
                clicking_item_flags(clicking),
            );
            update_tip(state, &tip_text(work_mode, clicking));
            // The shell requires the owner window to be foreground, otherwise the
            // menu stays on screen when the user clicks elsewhere.
            let _ = SetForegroundWindow(state.hwnd);
            let flags = TPM_RIGHTBUTTON.0 | TPM_RETURNCMD.0;
            let cmd = TrackPopupMenuEx(state.menu, flags, pt.x, pt.y, state.hwnd, None);
            let _ = PostMessageW(state.hwnd, WM_NULL, WPARAM(0), LPARAM(0));
            if cmd.0 != 0 {
                if let Some(action) = tray_action_for_command(cmd.0 as u32) {
                    dispatch(state, action);
                }
            }
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_actions() {
        assert_eq!(tray_action_for_command(MENU_ID_OPEN), Some(TrayAction::Open));
        assert_eq!(
            tray_action_for_command(MENU_ID_TOGGLE_CLICKING),
            Some(TrayAction::ToggleClicking)
        );
        assert_eq!(
            tray_action_for_command(MENU_ID_TOGGLE_MODE),
            Some(TrayAction::ToggleMode)
        );
        assert_eq!(
            tray_action_for_command(MENU_ID_TOGGLE_HUD),
            Some(TrayAction::ToggleHud)
        );
        assert_eq!(
            tray_action_for_command(MENU_ID_RELOAD_UI),
            Some(TrayAction::ReloadUi)
        );
        assert_eq!(
            tray_action_for_command(MENU_ID_RESTART_APP),
            Some(TrayAction::RestartApp)
        );
        assert_eq!(tray_action_for_command(MENU_ID_QUIT), Some(TrayAction::Quit));
    }

    /// The mode item carries a check mark while Work Mode is on: with deep sleep
    /// there is no page to read the state from, so the menu itself must show it.
    #[test]
    fn mode_item_check_mark_follows_work_mode() {
        const MF_CHECKED: u32 = 0x0000_0008;
        assert_eq!(mode_item_flags(false) & MF_CHECKED, 0);
        assert_eq!(mode_item_flags(true) & MF_CHECKED, MF_CHECKED);
        // Anything else in the flags would have to be MF_BYCOMMAND (0).
        assert_eq!(mode_item_flags(false), 0);
        assert_eq!(mode_item_flags(true), MF_CHECKED);
    }

    /// Both state items show the live state, and the tooltip repeats it — with
    /// deep sleep the menu is the only place either can be seen.
    #[test]
    fn state_items_and_tooltip_follow_the_live_state() {
        const MF_CHECKED: u32 = 0x0000_0008;
        assert_eq!(clicking_item_flags(true) & MF_CHECKED, MF_CHECKED);
        assert_eq!(clicking_item_flags(false), 0);

        assert_eq!(tip_text(false, false), "NanoClick — Autoclicker · idle");
        assert_eq!(tip_text(true, true), "NanoClick — Work Mode · clicking");
        for (work, run) in [(false, false), (false, true), (true, false), (true, true)] {
            assert!(
                tip_text(work, run).encode_utf16().count() < 64,
                "Explorer truncates tips longer than 63 units"
            );
        }
    }

    #[test]
    fn unknown_menu_id_is_ignored() {
        assert_eq!(tray_action_for_command(0), None);
        assert_eq!(tray_action_for_command(99), None);
        assert_eq!(tray_action_for_command(u32::MAX), None);
    }

    #[test]
    fn menu_ids_are_unique() {
        let ids = [
            MENU_ID_OPEN,
            MENU_ID_TOGGLE_CLICKING,
            MENU_ID_TOGGLE_MODE,
            MENU_ID_TOGGLE_HUD,
            MENU_ID_RELOAD_UI,
            MENU_ID_RESTART_APP,
            MENU_ID_QUIT,
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b, "tray menu ids must be unique: {a} == {b}");
            }
        }
    }

    #[test]
    fn click_events_map_to_intents() {
        assert_eq!(tray_click_for_event(TRAY_EVENT_LBUTTONUP), TrayClick::Open);
        assert_eq!(
            tray_click_for_event(TRAY_EVENT_LBUTTONDBLCLK),
            TrayClick::Open
        );
        assert_eq!(tray_click_for_event(TRAY_EVENT_SELECT), TrayClick::Open);
        assert_eq!(tray_click_for_event(TRAY_EVENT_RBUTTONUP), TrayClick::Menu);
        assert_eq!(
            tray_click_for_event(TRAY_EVENT_CONTEXTMENU),
            TrayClick::Menu
        );
        // Balloon notifications (1026-1031) and anything unknown must not open
        // the window — a stray click handler would steal focus mid-game.
        assert_eq!(tray_click_for_event(1027), TrayClick::Ignore);
        assert_eq!(tray_click_for_event(0), TrayClick::Ignore);
    }

    #[test]
    fn callback_message_sits_in_wm_app_range() {
        // Below WM_APP belongs to Windows; above WM_APP+0x3FFF to other apps.
        assert_eq!(WM_TRAY_CALLBACK, WM_APP_BASE + 1);
        assert!(WM_TRAY_CALLBACK > 0x8000 && WM_TRAY_CALLBACK < 0xC000);
    }

    #[test]
    fn tip_fits_the_shell_limit() {
        assert!(
            TRAY_TIP.encode_utf16().count() < 64,
            "Explorer truncates tips longer than 63 units"
        );
    }

    #[test]
    fn fill_wide_always_nul_terminates() {
        let mut buf = [0xFFFFu16; 8];
        fill_wide(&mut buf, "abc");
        assert_eq!(&buf[..4], &[b'a' as u16, b'b' as u16, b'c' as u16, 0]);
        assert_eq!(buf[4], 0xFFFF, "nothing may be written past the terminator");

        // A too-long string is truncated, never overflowed.
        let mut tight = [0xFFFFu16; 3];
        fill_wide(&mut tight, "abcdef");
        assert_eq!(&tight[..3], &[b'a' as u16, b'b' as u16, 0]);

        // Degenerate buffer: no panic.
        let mut empty: [u16; 0] = [];
        fill_wide(&mut empty, "x");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn codes_match_the_windows_crate() {
        use windows::Win32::UI::Shell::NIN_SELECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            WM_APP, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
        };
        assert_eq!(WM_APP_BASE, WM_APP);
        assert_eq!(TRAY_EVENT_LBUTTONUP, WM_LBUTTONUP);
        assert_eq!(TRAY_EVENT_LBUTTONDBLCLK, WM_LBUTTONDBLCLK);
        assert_eq!(TRAY_EVENT_RBUTTONUP, WM_RBUTTONUP);
        assert_eq!(TRAY_EVENT_CONTEXTMENU, WM_CONTEXTMENU);
        assert_eq!(TRAY_EVENT_SELECT, NIN_SELECT);
    }

    #[test]
    fn class_name_is_written_literally_in_the_pump() {
        // The pump registers/creates/unregisters the class with a `w!()` literal.
        // If it ever drifts from CLASS_NAME, UnregisterClassW would leak a class
        // entry and a second app instance could fail oddly.
        assert!(
            include_str!("tray.rs").contains("\"NanoClickTrayWnd\""),
            "class name literal must exist in the pump"
        );
    }
}


