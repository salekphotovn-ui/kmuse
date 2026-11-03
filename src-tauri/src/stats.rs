use crate::{data_dir, queue};
use serde::Serialize;
use std::sync::{Mutex, OnceLock};
use sysinfo::{get_current_pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tauri::{AppHandle, State};

#[derive(Serialize)]
pub struct RuntimeStats {
    /// Jobs being worked on right now (one per account).
    running: usize,
    waiting: usize,
    /// Browser processes started for the accounts plus the app itself.
    processes: usize,
    ram_bytes: u64,
}

fn system() -> &'static Mutex<System> {
    static CELL: OnceLock<Mutex<System>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(System::new()))
}

/// Memory of this app (with its web view) and of every browser running on an account profile of this app.
fn memory(app: &AppHandle) -> (usize, u64) {
    let profiles = data_dir(app).map(|d| d.join("profiles").display().to_string().to_lowercase()).unwrap_or_default();
    let me = get_current_pid().ok();
    let mut sys = system().lock().unwrap();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_memory().with_cmd(UpdateKind::OnlyIfNotSet),
    );
    // This app and everything it started (the web view and its helpers), found by walking down the process tree.
    let mut family: std::collections::HashSet<sysinfo::Pid> = me.into_iter().collect();
    loop {
        let before = family.len();
        for (pid, process) in sys.processes() {
            if process.parent().is_some_and(|p| family.contains(&p)) { family.insert(*pid); }
        }
        if family.len() == before { break; }
    }
    let (mut count, mut bytes) = (0, 0);
    for (pid, process) in sys.processes() {
        let browser = !profiles.is_empty() && process.cmd().iter().any(|a| a.to_string_lossy().to_lowercase().contains(&profiles));
        if family.contains(pid) || browser {
            count += 1;
            bytes += process.memory();
        }
    }
    (count, bytes)
}

#[tauri::command(async)]
pub fn get_runtime_stats(app: AppHandle, queue: State<queue::Queue>) -> RuntimeStats {
    let (running, waiting) = queue::counts(&queue.0);
    let (processes, ram_bytes) = memory(&app);
    RuntimeStats { running, waiting, processes, ram_bytes }
}
