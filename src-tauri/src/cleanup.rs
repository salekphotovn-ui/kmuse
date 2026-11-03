use crate::{browser, load_accounts, media_dir, queue};
use serde::Serialize;
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, State};

/// Browser cache folders that Chrome rebuilds by itself. Cookies, Local Storage and IndexedDB are not listed on purpose: they hold the Muse login.
const CACHE_DIRS: &[&str] = &[
    "Default/Cache",
    "Default/Code Cache",
    "Default/GPUCache",
    "Default/DawnGraphiteCache",
    "Default/DawnWebGPUCache",
    "Default/Service Worker/CacheStorage",
    "Default/Service Worker/ScriptCache",
    "GrShaderCache",
    "ShaderCache",
    "GPUPersistentCache",
];

/// Names of the result files this app writes into the output folder. Anything else the user keeps there is left alone.
const RESULT_PREFIXES: &[&str] = &["image-", "video-", "api-ref-"];

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else { return fs::metadata(path).map(|m| m.len()).unwrap_or(0) };
    entries.flatten().map(|e| match e.file_type() {
        Ok(t) if t.is_dir() => dir_size(&e.path()),
        _ => e.metadata().map(|m| m.len()).unwrap_or(0),
    }).sum()
}

/// (account id, cache folder) of every profile, for every browser the app has used.
fn cache_folders(app: &AppHandle) -> Vec<(String, PathBuf)> {
    let Ok(base) = crate::data_dir(app).map(|d| d.join("profiles")) else { return Vec::new() };
    let mut found = Vec::new();
    let Ok(accounts) = fs::read_dir(&base) else { return found };
    for account in accounts.flatten() {
        let id = account.file_name().to_string_lossy().to_string();
        let Ok(browsers) = fs::read_dir(account.path()) else { continue };
        for browser in browsers.flatten() {
            for rel in CACHE_DIRS {
                let dir = browser.path().join(rel);
                if dir.is_dir() { found.push((id.clone(), dir)); }
            }
        }
    }
    found
}

fn is_result_file(name: &str) -> bool {
    RESULT_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn normal(path: &Path) -> String {
    path.display().to_string().trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase()
}

/// Result files in the output folder (and uploaded reference images): (path, size).
fn result_files() -> Vec<(PathBuf, u64)> {
    let Ok(dir) = media_dir() else { return Vec::new() };
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_file() && is_result_file(&name) {
                files.push((e.path(), e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    if let Ok(entries) = fs::read_dir(dir.join("refs")) {
        for e in entries.flatten() {
            if e.path().is_file() { files.push((e.path(), e.metadata().map(|m| m.len()).unwrap_or(0))); }
        }
    }
    files
}

#[derive(Serialize)]
pub struct StorageInfo {
    finished_jobs: usize,
    result_files: usize,
    result_bytes: u64,
    cache_bytes: u64,
    /// Accounts whose browser is open right now: their cache cannot be cleared until it closes.
    cache_busy: usize,
}

#[tauri::command(async)]
pub fn get_storage_info(app: AppHandle, queue: State<queue::Queue>) -> StorageInfo {
    let files = result_files();
    let open = open_accounts(&app);
    let folders = cache_folders(&app);
    StorageInfo {
        finished_jobs: queue::finished_count(&queue.0),
        result_files: files.len(),
        result_bytes: files.iter().map(|(_, n)| n).sum(),
        cache_bytes: folders.iter().map(|(_, p)| dir_size(p)).sum(),
        cache_busy: open.len(),
    }
}

/// Ids of accounts whose browser is running (its profile is locked and in use).
fn open_accounts(app: &AppHandle) -> HashSet<String> {
    load_accounts(app).unwrap_or_default().into_iter()
        .filter(|a| a.port.is_some_and(browser::is_up))
        .map(|a| a.id).collect()
}

#[derive(Serialize)]
pub struct CleanReport {
    jobs_removed: usize,
    files_removed: usize,
    bytes_freed: u64,
    skipped: usize,
}

/// Removes finished jobs and their thumbnails; with `delete_files` also the pictures/videos they produced.
/// Waiting and running jobs, and the files they still need, are never touched.
#[tauri::command(async)]
pub fn clear_history(app: AppHandle, queue: State<queue::Queue>, delete_files: bool) -> CleanReport {
    do_clear_history(&app, &queue.0, delete_files)
}

fn do_clear_history(app: &AppHandle, rt: &Arc<Mutex<queue::Runtime>>, delete_files: bool) -> CleanReport {
    let (jobs_removed, protected) = queue::clear_finished(app, rt);
    let mut report = CleanReport { jobs_removed, files_removed: 0, bytes_freed: 0, skipped: 0 };
    if delete_files {
        for (path, size) in result_files() {
            if protected.contains(&normal(&path)) { report.skipped += 1; continue; }
            if fs::remove_file(&path).is_ok() { report.files_removed += 1; report.bytes_freed += size; } else { report.skipped += 1; }
        }
    }
    report
}

/// Clears the disposable browser cache of every account whose browser is closed.
#[tauri::command(async)]
pub fn clear_cache(app: AppHandle) -> CleanReport {
    do_clear_cache(&app)
}

fn do_clear_cache(app: &AppHandle) -> CleanReport {
    let open = open_accounts(app);
    let mut report = CleanReport { jobs_removed: 0, files_removed: 0, bytes_freed: 0, skipped: 0 };
    let mut skipped_accounts = HashSet::new();
    for (id, dir) in cache_folders(app) {
        if open.contains(&id) { skipped_accounts.insert(id); continue; }
        let size = dir_size(&dir);
        if fs::remove_dir_all(&dir).is_ok() { report.bytes_freed += size; report.files_removed += 1; }
    }
    report.skipped = skipped_accounts.len();
    report
}

// ---- automatic cleaning ----

/// How often the automatic cleaning runs once switched on.
const AUTO_CLEAN_EVERY: Duration = Duration::from_secs(2 * 24 * 3600);

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct AutoClean {
    pub enabled: bool,
    /// Also delete the finished pictures/videos in the output folder (off by default: they are the user's results).
    pub delete_files: bool,
    /// When the last cleaning ran, or when the feature was switched on (the schedule counts from there).
    pub last_ms: u64,
    pub last_summary: String,
}

#[derive(Serialize)]
pub struct AutoCleanView {
    enabled: bool,
    delete_files: bool,
    last_ms: u64,
    next_ms: u64,
    last_summary: String,
}

fn auto_cell() -> &'static Mutex<AutoClean> {
    static CELL: OnceLock<Mutex<AutoClean>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(AutoClean::default()))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn auto_file(app: &AppHandle) -> Option<PathBuf> {
    let dir = crate::data_dir(app).ok()?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("autoclean.json"))
}

fn save_auto(app: &AppHandle, state: &AutoClean) {
    if let (Some(path), Ok(text)) = (auto_file(app), serde_json::to_string(state)) { let _ = fs::write(path, text); }
}

fn view(state: &AutoClean) -> AutoCleanView {
    AutoCleanView {
        enabled: state.enabled,
        delete_files: state.delete_files,
        last_ms: state.last_ms,
        next_ms: state.last_ms + AUTO_CLEAN_EVERY.as_millis() as u64,
        last_summary: state.last_summary.clone(),
    }
}

#[tauri::command]
pub fn get_auto_clean() -> AutoCleanView { view(&auto_cell().lock().unwrap()) }

#[tauri::command]
pub fn set_auto_clean(app: AppHandle, enabled: bool, delete_files: bool) -> AutoCleanView {
    let mut state = auto_cell().lock().unwrap();
    // Switching on starts the 2-day countdown from now.
    if enabled && !state.enabled { state.last_ms = now_ms(); }
    state.enabled = enabled;
    state.delete_files = delete_files;
    save_auto(&app, &state);
    view(&state)
}

/// Loads the saved setting and starts the checker: every minute it looks whether the cleaning is due and nothing is running or waiting.
pub fn start_auto_clean(app: AppHandle, rt: Arc<Mutex<queue::Runtime>>) {
    if let Some(saved) = auto_file(&app).and_then(|p| fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<AutoClean>(&t).ok()) {
        *auto_cell().lock().unwrap() = saved;
    }
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(60));
        let state = auto_cell().lock().unwrap().clone();
        if !state.enabled || now_ms() < state.last_ms + AUTO_CLEAN_EVERY.as_millis() as u64 { continue; }
        // Never clean while anything is being made or about to be made; try again a minute later.
        let (running, waiting) = queue::counts(&rt);
        if running > 0 || waiting > 0 { continue; }
        let history = do_clear_history(&app, &rt, state.delete_files);
        let cache = do_clear_cache(&app);
        let freed = history.bytes_freed + cache.bytes_freed;
        let mut current = auto_cell().lock().unwrap();
        current.last_ms = now_ms();
        current.last_summary = format!("{} mục lịch sử, {} file, {} cache, giải phóng {}", history.jobs_removed, history.files_removed, cache.files_removed, crate::cleanup::human_bytes(freed));
        save_auto(&app, &current);
    });
}

pub fn human_bytes(bytes: u64) -> String {
    if bytes >= 1 << 30 { format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64) }
    else if bytes >= 1 << 20 { format!("{} MB", bytes >> 20) }
    else { format!("{} KB", bytes >> 10) }
}
