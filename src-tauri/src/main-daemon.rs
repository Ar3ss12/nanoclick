//! Stage D daemon entry: owns iron, config, truth. No windows, no Tauri.
//! Boot: single-instance mutex -> config load -> scheduler+hooks+tray ->
//! IPC server -> spawn UI child under supervision. UI death = restart,
//! daemon death = external recovery (Task Scheduler / service).
use nanoclick::daemon_server::DaemonState;
use std::sync::Arc;

fn main() {
    let _guard = match nanoclick::daemon_single::try_become_daemon() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("[daemon] already running: {e}");
            std::process::exit(0);
        }
    };
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
}
