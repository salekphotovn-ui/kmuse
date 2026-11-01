use crate::{browser, load_accounts, media_dir, queue};
use serde::Serialize;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
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
    let (jobs_removed, protected) = queue::clear_finished(&app, &queue.0);
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
    let open = open_accounts(&app);
    let mut report = CleanReport { jobs_removed: 0, files_removed: 0, bytes_freed: 0, skipped: 0 };
    let mut skipped_accounts = HashSet::new();
    for (id, dir) in cache_folders(&app) {
        if open.contains(&id) { skipped_accounts.insert(id); continue; }
        let size = dir_size(&dir);
        if fs::remove_dir_all(&dir).is_ok() { report.bytes_freed += size; report.files_removed += 1; }
    }
    report.skipped = skipped_accounts.len();
    report
}
