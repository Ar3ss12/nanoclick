//! Stage D daemon entry: owns iron, config, truth. No windows, no Tauri.
//! Boot: single-instance mutex -> config load -> scheduler+hooks+tray ->
//! IPC server -> spawn UI child under supervision. UI death = restart,
//! daemon death = external recovery (Task Scheduler / service).
use nanoclick::daemon_server::DaemonState;
use std::sync::Arc;

fn main() {
    // Guardian host for the daemon: `--input-watchdog <pid>`. The daemon is
    // the process that owns the scheduler and the hooks, so its death is the
    // one that can leave a button down. Handled before the daemon mutex.
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = nanoclick::input_guardian::run_if_watchdog(&args) {
        std::process::exit(code);
    }
    let _guard = match nanoclick::daemon_single::try_become_daemon() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("[daemon] already running: {e}");
            std::process::exit(0);
        }
    };
    // Armed after the single-instance gate: a forwarded second daemon must not
    // spawn a guardian that injects a release on its way out.
    nanoclick::input_guardian::spawn_guardian();
    // DaemonState::new already loaded the file and armed the scheduler —
    // the daemon OWNS config from here on, UI only asks via IPC.
    let state = Arc::new(DaemonState::new());
    // UI supervisor on its own thread: death of the window never touches
    // the engine. Daemon shutdown stops the loop (child exits with us).
    std::thread::Builder::new()
        .name("nanoclick-ui-supervisor".into())
        .spawn(|| {
            nanoclick::daemon_supervise::supervise_ui(|exit| {
                eprintln!("[daemon] ui exit: {exit:?}");
            });
        })
        .ok();
    eprintln!(
        "[daemon] serving {} (pid {})",
        nanoclick::ipc::DAEMON_PIPE_NAME,
        std::process::id()
    );
    if let Err(e) = nanoclick::daemon_server::serve(Arc::clone(&state)) {
        eprintln!("[daemon] serve failed: {e}");
        std::process::exit(1);
    }
    // Deliberate stop: the guardian must not inject a release.
    nanoclick::input_guardian::mark_clean_exit();
}
