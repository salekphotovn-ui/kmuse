use crate::{account_port, cdp_call, probe_status};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    process::Command,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};
use tauri::AppHandle;

/// Status of an account whose window is closed but whose login was good when it closed. It still accepts jobs; the window opens on demand.
pub const READY_CLOSED: &str = "Sẵn sàng · cửa sổ đang tắt";

#[derive(Clone, Serialize, Deserialize)]
pub struct RunSettings {
    /// Open an account's window automatically when a job needs it.
    pub auto_open: bool,
    /// Windows opened automatically have no visible window at all (headless).
    pub hidden: bool,
    /// Close an automatically opened window after this many idle minutes (0 = never).
    pub idle_minutes: u64,
}

impl Default for RunSettings {
    fn default() -> Self { Self { auto_open: true, hidden: false, idle_minutes: 5 } }
}

fn settings_cell() -> &'static Mutex<RunSettings> {
    static CELL: OnceLock<Mutex<RunSettings>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(RunSettings::default()))
}

/// Accounts whose window this app opened itself (not windows the user opened), with the time of their last activity.
fn auto_opened() -> &'static Mutex<HashMap<String, Instant>> {
    static CELL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn settings() -> RunSettings {
    settings_cell().lock().unwrap().clone()
}

fn settings_file(app: &AppHandle) -> Option<PathBuf> {
    let dir = crate::data_dir(app).ok()?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("run.json"))
}

pub fn init(app: &AppHandle) {
    let saved: RunSettings = settings_file(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    *settings_cell().lock().unwrap() = saved;
}

#[tauri::command]
pub fn get_run_settings() -> RunSettings { settings() }

#[tauri::command]
pub fn set_run_settings(app: AppHandle, auto_open: bool, hidden: bool, idle_minutes: u64) -> Result<RunSettings, String> {
    let next = RunSettings { auto_open, hidden, idle_minutes: idle_minutes.min(1440) };
    if let Some(path) = settings_file(&app) {
        fs::write(path, serde_json::to_string(&next).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    *settings_cell().lock().unwrap() = next.clone();
    Ok(next)
}

pub enum Mode { Visible, Minimized, Hidden }

fn browser_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let candidates: Vec<PathBuf> = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ].iter().map(PathBuf::from).collect();
    #[cfg(target_os = "macos")]
    let candidates: Vec<PathBuf> = {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut list = Vec::new();
        for name in ["Google Chrome", "Microsoft Edge"] {
            let inner = format!("{name}.app/Contents/MacOS/{name}");
            list.push(PathBuf::from("/Applications").join(&inner));
            if let Some(home) = &home { list.push(home.join("Applications").join(&inner)); }
        }
        list
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let candidates: Vec<PathBuf> = ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/microsoft-edge"].iter().map(PathBuf::from).collect();
    candidates.into_iter().find(|p| p.exists()).ok_or_else(|| "Không tìm thấy Google Chrome hoặc Microsoft Edge. Hãy cài một trong hai.".to_string())
}

pub fn launch(app: &AppHandle, id: &str, port: u16, mode: Mode) -> Result<(), String> {
    let browser = browser_path()?;
    let mut profile = crate::data_dir(app)?;
    profile.push("profiles");
    profile.push(id);
    // Chromium browsers must not share a data directory, even for the same account.
    profile.push(browser.file_stem().ok_or("Đường dẫn trình duyệt không hợp lệ.")?);
    fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let mut command = Command::new(&browser);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--remote-debugging-port={port}"))
        .args(["--no-first-run", "--no-default-browser-check", "--disable-sync", "--remote-allow-origins=http://localhost", "--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding", "--disable-background-timer-throttling"]);
    match mode {
        Mode::Visible => { command.arg("--new-window"); }
        Mode::Minimized => { command.args(["--new-window", "--start-minimized"]); }
        Mode::Hidden => { command.args(["--headless=new", "--window-size=1366,900"]); }
    }
    command.arg("https://muse.ai/").spawn().map_err(|e| format!("Không mở được trình duyệt: {e}"))?;
    Ok(())
}

pub fn is_up(port: u16) -> bool {
    reqwest::blocking::Client::builder().timeout(Duration::from_millis(1500)).build().ok()
        .and_then(|c| c.get(format!("http://127.0.0.1:{port}/json/version")).send().ok())
        .is_some_and(|r| r.status().is_success())
}

pub fn close(port: u16) {
    let version: Option<serde_json::Value> = reqwest::blocking::Client::builder().timeout(Duration::from_secs(2)).build().ok()
        .and_then(|c| c.get(format!("http://127.0.0.1:{port}/json/version")).send().ok())
        .and_then(|r| r.json().ok());
    if let Some(ws) = version.as_ref().and_then(|v| v.get("webSocketDebuggerUrl")).and_then(|v| v.as_str()) {
        let _ = cdp_call(ws, "Browser.close", serde_json::json!({}), 1);
    }
    for _ in 0..20 {
        if !is_up(port) { break; }
        thread::sleep(Duration::from_millis(250));
    }
}

/// Whether the browser on this port runs without a window. Only this app starts those, never the user.
fn is_headless(port: u16) -> bool {
    reqwest::blocking::Client::builder().timeout(Duration::from_millis(1500)).build().ok()
        .and_then(|c| c.get(format!("http://127.0.0.1:{port}/json/version")).send().ok())
        .and_then(|r| r.json::<serde_json::Value>().ok())
        .and_then(|v| v.get("User-Agent").and_then(|u| u.as_str()).map(|u| u.contains("HeadlessChrome")))
        .unwrap_or(false)
}

/// Closes a window this app opened itself, e.g. before the user opens the same profile visibly.
/// A headless one left over from an earlier run of the app is not tracked any more but must go too: a visible launch on the same profile would just hand over to it and show nothing.
pub fn close_auto_opened(id: &str, port: u16) {
    let tracked = auto_opened().lock().unwrap().remove(id).is_some();
    if tracked || is_headless(port) { close(port); }
}

/// At startup: closes headless browsers that an earlier run of the app left behind, since nothing tracks or idle-closes them.
pub fn close_orphans(app: &AppHandle) {
    let Ok(accounts) = crate::load_accounts(app) else { return };
    for account in accounts {
        if let Ok(port) = account_port(app, &account.id) {
            if is_headless(port) { close(port); }
        }
    }
}

/// Closes every browser this app opened itself, gracefully so Chrome saves its cookies. Called when the app exits.
pub fn close_all_auto(app: &AppHandle) {
    let ids: Vec<String> = auto_opened().lock().unwrap().keys().cloned().collect();
    let handles: Vec<_> = ids.into_iter().filter_map(|id| account_port(app, &id).ok()).map(|port| thread::spawn(move || close(port))).collect();
    for h in handles { let _ = h.join(); }
}

pub fn touch(id: &str) {
    if let Some(last) = auto_opened().lock().unwrap().get_mut(id) { *last = Instant::now(); }
}

/// Returns the debug port of an account whose Muse window is up and logged in, opening it first if allowed.
pub fn ensure_open(app: &AppHandle, id: &str) -> Result<u16, String> {
    let port = account_port(app, id)?;
    if is_up(port) {
        touch(id);
        return Ok(port);
    }
    let run = settings();
    if !run.auto_open {
        return Err("Cửa sổ Muse của tài khoản này chưa mở. Hãy bấm ‘Đăng nhập / Mở Muse’ ở trang Tài khoản.".into());
    }
    launch(app, id, port, if run.hidden { Mode::Hidden } else { Mode::Minimized })?;
    auto_opened().lock().unwrap().insert(id.to_string(), Instant::now());
    let start = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(1));
        match probe_status(port) {
            Some("Đã kết nối") => {
                // The chat box shows up before Muse's live connection is ready; sending too early gets "Delivery not confirmed".
                thread::sleep(Duration::from_secs(6));
                return Ok(port);
            }
            // The chat box renders a moment after the page loads, so only trust "not logged in" after a grace period.
            Some("Chưa đăng nhập") if start.elapsed().as_secs() > 15 => {
                close_auto_opened(id, port);
                crate::mark_status(app, id, "Chưa đăng nhập");
                return Err("Tài khoản chưa đăng nhập hoặc phiên đã hết hạn. Hãy bấm ‘Đăng nhập / Mở Muse’ để đăng nhập lại.".into());
            }
            _ => {}
        }
        if start.elapsed().as_secs() > 90 {
            close_auto_opened(id, port);
            return Err("Mở cửa sổ Muse quá lâu, đã hủy.".into());
        }
    }
}

/// Called regularly by the queue: closes automatically opened windows that have no job and have been idle long enough.
pub fn close_idle(app: &AppHandle, busy: &HashSet<String>) {
    let idle = settings().idle_minutes;
    if idle == 0 { return; }
    let due: Vec<String> = auto_opened().lock().unwrap().iter()
        .filter(|(id, last)| !busy.contains(*id) && last.elapsed() >= Duration::from_secs(idle * 60))
        .map(|(id, _)| id.clone()).collect();
    for id in due {
        if let Ok(port) = account_port(app, &id) { close_auto_opened(&id, port); }
    }
}

pub fn is_ready(status: &str) -> bool {
    status == "Đã kết nối" || status == READY_CLOSED
}

/// Whether an account may receive jobs: ready ones, plus closed windows that can be opened on demand (login is verified when opening).
pub fn is_usable(status: &str) -> bool {
    is_ready(status) || (status == "Chưa mở cửa sổ Muse" && settings().auto_open)
}

