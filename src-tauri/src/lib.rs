use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, process::Command, thread, time::Duration};
use tauri::Manager;

mod api;
mod browser;
mod cleanup;
mod queue;
mod stats;
mod updater;
use tungstenite::{connect, Message};
use base64::{engine::general_purpose::STANDARD, Engine};

const EXTRACT_JS: &str = include_str!("extract.js");

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    plan_usage: Option<String>,
    #[serde(default)]
    additional_tokens: Option<String>,
    #[serde(default)]
    checked_at: Option<String>,
}

fn debug_port(id: &str) -> u16 {
    19_000 + (id.bytes().fold(0u16, |sum, byte| sum.wrapping_add(byte as u16)) % 1_000)
}

/// Stable debug port per account. Existing accounts keep their old hashed port (their Chrome may already be open on it); collisions move to the next free port.
pub(crate) fn account_port(app: &tauri::AppHandle, id: &str) -> Result<u16, String> {
    let mut accounts = load_accounts(app)?;
    let existing = accounts.iter().find(|a| a.id == id).ok_or("Không tìm thấy tài khoản.")?.port;
    if let Some(port) = existing { return Ok(port); }
    let mut port = debug_port(id);
    while accounts.iter().any(|a| a.port == Some(port)) { port += 1; }
    if let Some(a) = accounts.iter_mut().find(|a| a.id == id) { a.port = Some(port); }
    save_accounts(app, &accounts)?;
    Ok(port)
}

/// Folder for everything the app stores per machine (accounts, browser profiles, queue, settings).
/// `KMUSE_DATA_DIR` points it elsewhere, which lets tests run against throw-away data instead of the real thing.
pub(crate) fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    match std::env::var_os("KMUSE_DATA_DIR") {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => app.path().app_data_dir().map_err(|e| e.to_string()),
    }
}

fn accounts_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = data_dir(app)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("accounts.json"))
}

pub(crate) fn load_accounts(app: &tauri::AppHandle) -> Result<Vec<Account>, String> {
    let path = accounts_file(app)?;
    if !path.exists() { return Ok(Vec::new()); }
    let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

fn save_accounts(app: &tauri::AppHandle, accounts: &[Account]) -> Result<(), String> {
    let path = accounts_file(app)?;
    let data = serde_json::to_string_pretty(accounts).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_accounts(app: tauri::AppHandle) -> Result<Vec<Account>, String> { load_accounts(&app) }

#[tauri::command]
fn add_account(app: tauri::AppHandle, name: String) -> Result<Account, String> {
    let name = name.trim().to_string();
    if name.is_empty() { return Err("Tên tài khoản không được để trống.".into()); }
    let mut accounts = load_accounts(&app)?;
    if accounts.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) { return Err("Tên tài khoản đã tồn tại.".into()); }
    let account = Account { id: uuid::Uuid::new_v4().to_string(), name, status: "Chưa đăng nhập".into(), port: None, plan_usage: None, additional_tokens: None, checked_at: None };
    accounts.push(account.clone());
    save_accounts(&app, &accounts)?;
    Ok(account)
}

#[tauri::command]
fn open_account(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let port = account_port(&app, &id)?;
    // A window this app opened itself (hidden or minimized) uses the same profile, so it has to go before a visible one can open.
    browser::close_auto_opened(&id, port);
    let mut accounts = load_accounts(&app)?;
    let account = accounts.iter_mut().find(|a| a.id == id).ok_or("Không tìm thấy tài khoản.")?;
    account.status = "Đã mở hồ sơ · chưa xác minh đăng nhập".into();
    save_accounts(&app, &accounts)?;
    browser::launch(&app, &id, port, browser::Mode::Visible)
}


/// Longest a single command to Chrome may take to answer before the page counts as frozen.
const CDP_REPLY_TIMEOUT: Duration = Duration::from_secs(90);

/// Opens the debugging socket with read/write time limits, so a frozen tab fails instead of blocking a job forever.
fn connect_ws(ws_url: &str) -> Result<tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>, String> {
    let (socket, _) = connect(ws_url).map_err(|e| format!("Không kết nối được Chrome: {e}"))?;
    if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
        let _ = stream.set_read_timeout(Some(CDP_REPLY_TIMEOUT));
        let _ = stream.set_write_timeout(Some(CDP_REPLY_TIMEOUT));
    }
    Ok(socket)
}

/// Turns a socket error into text; a missing answer is reported as a timeout so the queue stops the job instead of retrying it.
fn socket_error(e: tungstenite::Error) -> String {
    match e {
        tungstenite::Error::Io(ref io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            format!("{TIMED_OUT}: Chrome không trả lời sau {} giây (trang Muse có thể bị đơ).", CDP_REPLY_TIMEOUT.as_secs()),
        other => other.to_string(),
    }
}

/// Error prefix for a job that ran out of time; the queue stops such a job at once so the caller can resend it.
pub(crate) const TIMED_OUT: &str = "Quá thời gian";

fn cdp_eval(ws_url: &str, expression: &str, id: u64) -> Result<serde_json::Value, String> {
    let mut socket = connect_ws(ws_url)?;
    let request = serde_json::json!({
        "id": id,
        "method": "Runtime.evaluate",
        "params": { "expression": expression, "returnByValue": true, "awaitPromise": true }
    });
    socket.send(Message::Text(request.to_string().into())).map_err(|e| e.to_string())?;
    loop {
        let message = socket.read().map_err(socket_error)?;
        if let Message::Text(text) = message {
            let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return Ok(value);
            }
        }
    }
}

/// Clicks, with real mouse events, the element whose center the script returns as {x, y}.
fn mouse_click_on(ws_url: &str, center_js: &str) -> Result<(), String> {
    let center = eval_value(ws_url, center_js)?;
    let (Some(x), Some(y)) = (center.get("x").and_then(|v| v.as_f64()), center.get("y").and_then(|v| v.as_f64())) else {
        // Say what the page does show, so a layout that differs between machines (e.g. Chrome on macOS) can be told apart from a missing login.
        let seen = eval_value(ws_url, "(() => JSON.stringify({w: innerWidth, h: innerHeight, labels: [...document.querySelectorAll('button,[role=button]')].map(e => e.getAttribute('aria-label') || e.getAttribute('data-testid') || (e.innerText||'').trim().slice(0,20)).filter(Boolean).slice(0,25)}))()")
            .ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        return Err(format!("Không tìm thấy nút Settings trên trang Muse. Trang đang có: {seen}"));
    };
    for (kind, id) in [("mouseMoved", 11), ("mousePressed", 12), ("mouseReleased", 13)] {
        cdp_call(ws_url, "Input.dispatchMouseEvent", serde_json::json!({ "type": kind, "x": x, "y": y, "button": "left", "buttons": if kind == "mousePressed" { 1 } else { 0 }, "clickCount": 1 }), id)?;
    }
    Ok(())
}

/// Opens Muse's Settings dialog in the given tab, reads the Usage block, and closes the dialog again so the chat stays usable.
fn read_usage(ws_url: &str) -> Result<serde_json::Value, String> {
    // Muse ignores script-made clicks on these menus, so use real mouse events at the element's position.
    // The button can appear late, and other layouts label it differently, so wait for it and accept looser matches.
    let dock_center = r#"(() => { const vis=e=>{const r=e.getBoundingClientRect(); return r.width>0&&r.height>0}; const all=[...document.querySelectorAll('button,[role=button]')].filter(vis); const b=document.querySelector('[data-testid=hatch-dock-more]')||all.find(e=>e.getAttribute('aria-label')==='Settings')||all.find(e=>/settings|cài đặt/i.test((e.getAttribute('aria-label')||'')+' '+(e.getAttribute('title')||'')+' '+(e.getAttribute('data-testid')||'')+' '+(e.innerText||'').slice(0,20))); if(!b) return null; const r=b.getBoundingClientRect(); return {x:r.x+r.width/2, y:r.y+r.height/2} })()"#;
    for _ in 0..8 {
        if eval_value(ws_url, dock_center).map(|v| v.get("x").is_some()).unwrap_or(false) { break; }
        thread::sleep(Duration::from_secs(1));
    }
    mouse_click_on(ws_url, dock_center)?;
    thread::sleep(Duration::from_millis(600));
    let item_center = r#"(() => { const item=[...document.querySelectorAll('button,[role=menuitem],[role=button],a,div,span')].filter(e=>e.children.length===0 && /^(Settings|Cài đặt)$/.test((e.innerText||'').trim())).pop(); if(!item) return null; const r=item.getBoundingClientRect(); return {x:r.x+r.width/2, y:r.y+r.height/2} })()"#;
    mouse_click_on(ws_url, item_center)?;
    thread::sleep(Duration::from_secs(4));
    let read = r#"(() => { const text=(document.body.innerText||'').replace(/\r/g,''); const loggedIn=!/Log in or create an account/i.test(text); const lines=text.split('\n').map(s=>s.trim()).filter(Boolean); const usage=lines.findIndex(x=>x==='Usage'); let block=usage>=0?lines.slice(usage,usage+12):[]; const plan=block.find(x=>/% used/i.test(x)&&! /tokens left/i.test(x))||null; const extra=block.find(x=>/tokens left/i.test(x))||null; const tier=block.find(x=>/plan$/i.test(x))||null; const reset=block.find(x=>/reset/i.test(x))||null; return {loggedIn,tier,plan,reset,extra,block}; })()"#;
    let result = cdp_eval(ws_url, read, 3);
    // Leave the page as found even when reading failed.
    let _ = key_event(ws_url, "keyDown", "Escape", "Escape", 27, None);
    let _ = key_event(ws_url, "keyUp", "Escape", "Escape", 27, None);
    result?.pointer("/result/result/value").cloned().ok_or_else(|| "Không đọc được dữ liệu Usage từ Muse.".to_string())
}

/// Stores a Usage reading on the account; a logged-out reading clears it.
fn apply_usage(saved: &mut Account, data: &serde_json::Value) {
    if data.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false) {
        if !browser::is_ready(&saved.status) { saved.status = "Đã kết nối".into(); }
        let tier = data.get("tier").and_then(|v| v.as_str()).unwrap_or("Gói Muse");
        let plan = data.get("plan").and_then(|v| v.as_str());
        let reset = data.get("reset").and_then(|v| v.as_str());
        saved.plan_usage = Some([Some(tier), plan, reset].into_iter().flatten().collect::<Vec<_>>().join(" · "));
        saved.additional_tokens = data.get("extra").and_then(|v| v.as_str()).map(str::to_string);
    } else {
        saved.status = "Chưa đăng nhập".into();
        saved.plan_usage = None;
        saved.additional_tokens = None;
    }
    saved.checked_at = Some("Vừa kiểm tra".into());
}

static USAGE_PROGRESS: std::sync::Mutex<(bool, usize, usize)> = std::sync::Mutex::new((false, 0, 0));

/// Reads Usage for every logged-in account in the background, opening closed windows on demand (hidden if so configured).
#[tauri::command]
fn refresh_all_usage(app: tauri::AppHandle, queue: tauri::State<queue::Queue>) -> Result<(), String> {
    start_usage_refresh(&app, &queue.0)
}

pub(crate) fn start_usage_refresh(app: &tauri::AppHandle, queue: &std::sync::Arc<std::sync::Mutex<queue::Runtime>>) -> Result<(), String> {
    let app = app.clone();
    let queue = queue.clone();
    {
        let mut p = USAGE_PROGRESS.lock().unwrap();
        if p.0 { return Ok(()); }
        *p = (true, 0, 0);
    }
    let accounts = match load_accounts(&app) {
        Ok(accounts) => accounts,
        Err(e) => { USAGE_PROGRESS.lock().unwrap().0 = false; return Err(e); }
    };
    let busy = queue::busy_accounts(&queue);
    // Accounts marked "not logged in" are included too: if that verdict was a false alarm (for example saved while Windows was shutting down), this puts them right.
    let targets: Vec<Account> = accounts.into_iter().filter(|a| (browser::is_usable(&a.status) || a.status == "Chưa đăng nhập") && !busy.contains(&a.id)).collect();
    USAGE_PROGRESS.lock().unwrap().2 = targets.len();
    if targets.is_empty() { USAGE_PROGRESS.lock().unwrap().0 = false; return Ok(()); }
    thread::spawn(move || {
        let handles: Vec<_> = targets.into_iter().enumerate().map(|(i, account)| {
            let app = app.clone();
            let queue = queue.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(2 * i as u64));
                // A job may have claimed this account while it waited its turn; real work comes first.
                if queue::busy_accounts(&queue).contains(&account.id) {
                    USAGE_PROGRESS.lock().unwrap().1 += 1;
                    return;
                }
                // A marked-logged-out account whose window is already open is probably being logged in by the user right now: leave it alone.
                if account.status == "Chưa đăng nhập" && account_port(&app, &account.id).is_ok_and(browser::is_up) {
                    USAGE_PROGRESS.lock().unwrap().1 += 1;
                    return;
                }
                let outcome = browser::ensure_open(&app, &account.id).and_then(|port| {
                    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(3)).build().map_err(|e| e.to_string())?;
                    let tabs: Vec<serde_json::Value> = client.get(format!("http://127.0.0.1:{port}/json")).send().map_err(|e| e.to_string())?.json().map_err(|e| e.to_string())?;
                    let ws = tabs.iter().find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page") && t.get("url").and_then(|v| v.as_str()).is_some_and(|u| u.contains("muse.ai")))
                        .and_then(|t| t.get("webSocketDebuggerUrl")).and_then(|v| v.as_str()).ok_or("Không thấy trang Muse.")?.to_string();
                    read_usage(&ws)
                });
                browser::touch(&account.id);
                if let Ok(data) = outcome {
                    let _guard = ACCOUNTS_LOCK.lock().unwrap();
                    if let Ok(mut fresh) = load_accounts(&app) {
                        if let Some(saved) = fresh.iter_mut().find(|a| a.id == account.id) {
                            apply_usage(saved, &data);
                            let _ = save_accounts(&app, &fresh);
                        }
                    }
                }
                USAGE_PROGRESS.lock().unwrap().1 += 1;
            })
        }).collect();
        for h in handles { let _ = h.join(); }
        USAGE_PROGRESS.lock().unwrap().0 = false;
    });
    Ok(())
}

/// (running, done, total) of the background Usage refresh.
#[tauri::command]
fn usage_progress() -> (bool, usize, usize) { *USAGE_PROGRESS.lock().unwrap() }

pub(crate) fn usage_state() -> (bool, usize, usize) { *USAGE_PROGRESS.lock().unwrap() }

#[tauri::command(async)]
fn check_account(app: tauri::AppHandle, id: String) -> Result<Account, String> {
    let port = account_port(&app, &id)?;
    let mut accounts = load_accounts(&app)?;
    accounts.iter().find(|a| a.id == id).ok_or("Không tìm thấy tài khoản.")?;
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(3)).build().map_err(|e| e.to_string())?;
    let tabs: Vec<serde_json::Value> = client.get(format!("http://127.0.0.1:{port}/json"))
        .send().map_err(|_| "Hồ sơ chưa sẵn sàng để kiểm tra. Hãy đóng cửa sổ Muse cũ, bấm ‘Đăng nhập / Mở Muse’, rồi thử lại.".to_string())?
        .json().map_err(|e| e.to_string())?;
    let tab = tabs.iter().find(|tab| tab.get("url").and_then(|v| v.as_str()).is_some_and(|url| url.contains("muse.ai")))
        .ok_or("Không tìm thấy trang Muse đang mở trong hồ sơ này.")?;
    let ws_url = tab.get("webSocketDebuggerUrl").and_then(|v| v.as_str()).ok_or("Chrome không cho phép đọc trạng thái trang.")?;
    let data = read_usage(ws_url)?;
    let saved = accounts.iter_mut().find(|a| a.id == id).unwrap();
    apply_usage(saved, &data);
    let output = saved.clone();
    save_accounts(&app, &accounts)?;
    Ok(output)
}

fn cdp_call(ws_url: &str, method: &str, params: serde_json::Value, id: u64) -> Result<serde_json::Value, String> {
    let mut socket = connect_ws(ws_url)?;
    let request = serde_json::json!({ "id": id, "method": method, "params": params });
    socket.send(Message::Text(request.to_string().into())).map_err(|e| e.to_string())?;
    loop {
        let message = socket.read().map_err(socket_error)?;
        if let Message::Text(text) = message {
            let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if value.get("id").and_then(|v| v.as_u64()) == Some(id) { return Ok(value); }
        }
    }
}

fn eval_value(ws_url: &str, expression: &str) -> Result<serde_json::Value, String> {
    let r = cdp_eval(ws_url, expression, 1)?;
    if let Some(ex) = r.pointer("/result/exceptionDetails") { return Err(format!("Lỗi chạy trên trang Muse: {ex}")); }
    Ok(r.pointer("/result/result/value").cloned().unwrap_or(serde_json::Value::Null))
}

fn key_event(ws_url: &str, kind: &str, key: &str, code: &str, vk: u32, text: Option<&str>) -> Result<(), String> {
    let mut params = serde_json::json!({ "type": kind, "key": key, "code": code, "windowsVirtualKeyCode": vk });
    if let Some(t) = text { params["text"] = t.into(); }
    cdp_call(ws_url, "Input.dispatchKeyEvent", params, 2).map(|_| ())
}

pub(crate) fn media_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut dir = exe.parent().ok_or("Không xác định được thư mục ứng dụng.")?.to_path_buf();
    // Dev/build runs live under src-tauri\target; keep results next to the project instead.
    if let Some(pos) = dir.ancestors().position(|a| a.file_name().is_some_and(|n| n == "src-tauri")) {
        dir = dir.ancestors().nth(pos + 1).unwrap_or(&dir).to_path_buf();
    }
    // On macOS the executable lives inside the .app bundle, which is replaced on every update and must not hold results; use a folder in the home directory.
    #[cfg(target_os = "macos")]
    {
        if dir.ancestors().any(|a| a.extension().is_some_and(|e| e == "app")) {
            if let Some(home) = std::env::var_os("HOME") { dir = PathBuf::from(home).join("Muse Tool"); }
        }
    }
    dir.push("output");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[derive(Serialize)]
pub(crate) struct MediaResult { pub(crate) path: String, pub(crate) thumb: Option<String> }

/// Puts a local image into Muse's hidden file input. Node ids only live as long as one connection, so every step shares one socket.
fn attach_file(ws_url: &str, path: &str) -> Result<(), String> {
    let mut socket = connect_ws(ws_url)?;
    let mut call = |id: u64, method: &str, params: serde_json::Value| -> Result<serde_json::Value, String> {
        socket.send(Message::Text(serde_json::json!({ "id": id, "method": method, "params": params }).to_string().into())).map_err(|e| e.to_string())?;
        loop {
            if let Message::Text(text) = socket.read().map_err(socket_error)? {
                let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    if let Some(err) = value.get("error") { return Err(format!("Chrome từ chối ({method}): {err}")); }
                    return Ok(value);
                }
            }
        }
    };
    let root = call(1, "DOM.getDocument", serde_json::json!({ "depth": 0 }))?;
    let root_id = root.pointer("/result/root/nodeId").and_then(|v| v.as_u64()).ok_or("Không đọc được trang Muse.")?;
    let input = call(2, "DOM.querySelector", serde_json::json!({ "nodeId": root_id, "selector": "input[type=file]" }))?;
    let node_id = input.pointer("/result/nodeId").and_then(|v| v.as_u64()).filter(|id| *id != 0).ok_or("Muse không có ô đính kèm ảnh.")?;
    call(3, "DOM.setFileInputFiles", serde_json::json!({ "nodeId": node_id, "files": [path] }))?;
    Ok(())
}

/// Only files inside the results folder may be sent to Muse as a reference, so the local API cannot make it upload arbitrary files.
pub(crate) fn check_reference(path: &str) -> Result<String, String> {
    let file = PathBuf::from(path).canonicalize().map_err(|_| "Không tìm thấy ảnh tham chiếu.".to_string())?;
    let dir = media_dir()?.canonicalize().map_err(|e| e.to_string())?;
    if !file.starts_with(&dir) { return Err("Ảnh tham chiếu phải nằm trong thư mục kết quả (output).".into()); }
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) { return Err("Ảnh tham chiếu phải là PNG, JPG hoặc WEBP.".into()); }
    Ok(file.display().to_string().trim_start_matches(r"\\?\").to_string())
}

#[tauri::command]
fn save_reference(name: String, data: String) -> Result<String, String> {
    let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
    if bytes.len() > 20_000_000 { return Err("Ảnh tham chiếu quá lớn (tối đa 20 MB).".into()); }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let ext = if ["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) { ext } else { return Err("Ảnh tham chiếu phải là PNG, JPG hoặc WEBP.".into()); };
    let mut dir = media_dir()?;
    dir.push("refs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = dir.join(format!("ref-{stamp}.{ext}"));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Error prefix for an account that has used up its Muse credits; the queue keys off it.
/// Longest a single image / video may take, from sending the prompt to getting the file.
pub(crate) const IMAGE_LIMIT_SECS: u64 = 6 * 60;
pub(crate) const VIDEO_LIMIT_SECS: u64 = 12 * 60;

pub(crate) const OUT_OF_USAGE: &str = "Hết lượt";

pub(crate) fn run_media_job(port: u16, kind: &str, prompt: &str, ratio: &str, reference: Option<&str>) -> Result<MediaResult, String> {
    let (sel, prefix, timeout) = match kind {
        "video" => ("[aria-label^=\"Open animated video\"]", "Video", VIDEO_LIMIT_SECS),
        _ => ("[aria-label^=\"Open generated image\"]", "Ảnh", IMAGE_LIMIT_SECS),
    };
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(3)).build().map_err(|e| e.to_string())?;
    let tabs: Vec<serde_json::Value> = client.get(format!("http://127.0.0.1:{port}/json"))
        .send().map_err(|_| "Cửa sổ Muse của tài khoản này chưa mở. Hãy bấm ‘Đăng nhập / Mở Muse’ ở trang Tài khoản.".to_string())?
        .json().map_err(|e| e.to_string())?;
    let tab = tabs.iter().find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page") && t.get("url").and_then(|v| v.as_str()).is_some_and(|u| u.contains("muse.ai")))
        .ok_or("Không tìm thấy trang Muse đang mở trong hồ sơ này.")?;
    let ws = tab.get("webSocketDebuggerUrl").and_then(|v| v.as_str()).ok_or("Chrome không cho phép điều khiển trang.")?;

    // Close any open viewer, then count existing results so only the new one is picked up.
    key_event(ws, "keyDown", "Escape", "Escape", 27, None)?;
    key_event(ws, "keyUp", "Escape", "Escape", 27, None)?;
    // Muse only keeps recent messages mounted, so counting results is unreliable; compare the newest one instead.
    let sig_js = format!("(() => {{ const n=[...document.querySelectorAll('{sel}')]; const e=n[n.length-1]; if(!e) return ''; return (e.getAttribute('aria-label')||'')+'|'+(e.getAttribute('data-hatch-video-src')||(e.querySelector('img')||{{}}).src||''); }})()");
    let before = eval_value(ws, &sig_js)?.as_str().unwrap_or("").to_string();
    let focused = eval_value(ws, "(() => { const t=document.querySelector('textarea[aria-label=Message]'); if(!t) return false; t.focus(); return true })()")?;
    if focused != true { return Err("Không thấy ô nhập tin nhắn của Muse (có thể chưa đăng nhập).".into()); }
    let ratio_note = ratio.split('·').next().unwrap_or("").trim();
    let reference_note = if reference.is_some() { "Use the attached image as the reference and keep the same subject and style. " } else { "" };
    let text = if kind == "video" {
        format!("{prefix}: {reference_note}{prompt}. Make this an animated video clip of about 10 seconds with natural motion, not a still image. {ratio_note} aspect ratio. No text, no watermark.")
    } else {
        format!("{prefix}: {reference_note}{prompt}. {ratio_note} aspect ratio. No text, no watermark.")
    };
    if let Some(path) = reference {
        attach_file(ws, path)?;
        thread::sleep(Duration::from_millis(2500));
        eval_value(ws, "(() => { const t=document.querySelector('textarea[aria-label=Message]'); if(t) t.focus(); return true })()")?;
    }
    // Muse flags a message it could not deliver with a red "Delivery not confirmed"; count them so a new one can be told from old ones.
    let alerts_js = "[...document.querySelectorAll('[role=alert]')].filter(e => /Delivery not confirmed/i.test(e.textContent)).length";
    let mut alerts_seen = eval_value(ws, alerts_js)?.as_u64().unwrap_or(0);
    let mut resends = 0;
    cdp_call(ws, "Input.insertText", serde_json::json!({ "text": text }), 3)?;
    key_event(ws, "keyDown", "Enter", "Enter", 13, Some("\r"))?;
    key_event(ws, "keyUp", "Enter", "Enter", 13, None)?;

    // For video, Muse sometimes answers with a still image; give it a short grace period, then fail clearly.
    let img_sig_js = "(() => { const n=[...document.querySelectorAll('[aria-label^=\"Open generated image\"]')]; const e=n[n.length-1]; return e ? (e.getAttribute('aria-label')||'')+'|'+((e.querySelector('img')||{}).src||'') : ''; })()";
    let img_before = if kind == "video" { eval_value(ws, img_sig_js)?.as_str().unwrap_or("").to_string() } else { String::new() };
    let mut still_seen: Option<std::time::Instant> = None;
    let start = std::time::Instant::now();
    loop {
        thread::sleep(Duration::from_secs(1));
        let alerts = eval_value(ws, alerts_js)?.as_u64().unwrap_or(0);
        if alerts > alerts_seen {
            alerts_seen = alerts;
            if resends >= 2 { return Err("Muse không nhận được tin nhắn (Delivery not confirmed) sau 2 lần gửi lại.".into()); }
            resends += 1;
            eval_value(ws, "(() => { const t=document.querySelector('textarea[aria-label=Message]'); if(t) t.focus(); return true })()")?;
            cdp_call(ws, "Input.insertText", serde_json::json!({ "text": text }), 3)?;
            key_event(ws, "keyDown", "Enter", "Enter", 13, Some("\r"))?;
            key_event(ws, "keyUp", "Enter", "Enter", 13, None)?;
        }
        if kind == "video" {
            let img_now = eval_value(ws, img_sig_js)?;
            let img_now = img_now.as_str().unwrap_or("");
            if !img_now.is_empty() && img_now != img_before && still_seen.is_none() { still_seen = Some(std::time::Instant::now()); }
            if still_seen.is_some_and(|t| t.elapsed().as_secs() > 60) { return Err("Muse chỉ trả ảnh tĩnh, chưa tạo video. Hãy mô tả chuyển động rõ hơn trong prompt.".into()); }
        }
        let now = eval_value(ws, &sig_js)?;
        let now = now.as_str().unwrap_or("");
        if !now.is_empty() && now != before { break; }
        // An account without credits never produces a result; Muse shows "Out of usage" instead, so fail fast.
        let waited = start.elapsed().as_secs();
        if waited >= 15 && waited % 5 == 0 && eval_value(ws, "/\\bOut of usage\\b/i.test(document.body.innerText)")?.as_bool() == Some(true) {
            return Err(format!("{OUT_OF_USAGE}: tài khoản này đã hết lượt tạo trên Muse (Out of usage)."));
        }
        if waited > timeout { return Err(format!("{TIMED_OUT}: Muse chưa tạo xong sau {} phút, đã dừng tác vụ. Hãy gửi lại prompt.", timeout / 60)); }
    }
    thread::sleep(Duration::from_millis(800));
    let data = eval_value(ws, &format!("{EXTRACT_JS}({})", serde_json::json!(kind)))?;
    if let Some(err) = data.get("error").and_then(|v| v.as_str()) { return Err(format!("Không lấy được file từ Muse ({err}).")); }
    let bytes = STANDARD.decode(data.get("b64").and_then(|v| v.as_str()).ok_or("Muse không trả dữ liệu.")?).map_err(|e| e.to_string())?;
    let ext = match data.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "video/mp4" => "mp4", "image/png" => "png", "image/jpeg" => "jpg", "image/webp" => "webp", _ => if kind == "video" { "mp4" } else { "png" },
    };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = media_dir()?.join(format!("{kind}-{stamp}.{ext}"));
    let mime = data.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let thumb = if kind == "video" {
        data.get("poster").and_then(|v| v.as_str()).map(str::to_string)
    } else {
        Some(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
    };
    Ok(MediaResult { path: path.display().to_string(), thumb })
}

#[tauri::command]
fn open_media_file(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.starts_with(media_dir()?) { return Err("Đường dẫn không nằm trong thư mục kết quả.".into()); }
    open_in_system(&p)
}

/// Opens a file or folder with the system's default handler (Explorer on Windows, Finder on macOS).
fn open_in_system(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let opener = "xdg-open";
    Command::new(opener).arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_output_folder() -> Result<(), String> {
    let dir = media_dir()?;
    open_in_system(&dir)
}

#[tauri::command]
fn remove_account(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let mut accounts = load_accounts(&app)?;
    let before = accounts.len();
    accounts.retain(|a| a.id != id);
    if accounts.len() == before { return Err("Không tìm thấy tài khoản.".into()); }
    save_accounts(&app, &accounts)
}

/// Cheap login probe: no UI clicking, only checks that the chat box exists in the account's Muse tab.
fn probe_status(port: u16) -> Option<&'static str> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(2)).build().ok()?;
    let tabs: Vec<serde_json::Value> = match client.get(format!("http://127.0.0.1:{port}/json")).send().and_then(|r| r.json()) {
        Ok(tabs) => tabs,
        Err(_) => return Some("Chưa mở cửa sổ Muse"),
    };
    let Some(tab) = tabs.iter().find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page") && t.get("url").and_then(|v| v.as_str()).is_some_and(|u| u.contains("muse.ai"))) else {
        return Some("Chưa vào trang Muse");
    };
    let ws = tab.get("webSocketDebuggerUrl").and_then(|v| v.as_str())?;
    // Missing chat box alone proves nothing (page still loading, window closing at shutdown): only the login form counts as "logged out".
    let value = eval_value(ws, "(() => document.querySelector('textarea[aria-label=Message]') ? 'chat' : (/Log in or create an account/i.test(document.body.innerText) ? 'login' : 'loading'))()").ok()?;
    Some(match value.as_str() {
        Some("chat") => "Đã kết nối",
        Some("login") => "Chưa đăng nhập",
        _ => "Đang tải trang Muse",
    })
}

static ACCOUNTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn mark_status(app: &tauri::AppHandle, id: &str, status: &str) {
    let _guard = ACCOUNTS_LOCK.lock().unwrap();
    if let Ok(mut accounts) = load_accounts(app) {
        if let Some(saved) = accounts.iter_mut().find(|a| a.id == id) {
            saved.status = status.into();
            let _ = save_accounts(app, &accounts);
        }
    }
}

/// Keeps every opened account's status current, so logging in or closing a window shows up without pressing "Kiểm tra".
fn watch_accounts(app: tauri::AppHandle) {
    thread::spawn(move || {
    // Consecutive "login form shown" readings per account: a single one can be a page mid-navigation or a window closing.
    let mut logged_out_seen: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
    loop {
        thread::sleep(Duration::from_secs(5));
        let Ok(accounts) = load_accounts(&app) else { continue };
        for account in accounts.iter().filter(|a| a.port.is_some()) {
            let Some(mut status) = probe_status(account.port.unwrap()) else { continue };
            // Pages still loading or not on Muse yet say nothing about the login: keep the last known status.
            if status == "Đang tải trang Muse" || status == "Chưa vào trang Muse" { logged_out_seen.remove(&account.id); continue; }
            if status == "Chưa đăng nhập" {
                let seen = logged_out_seen.entry(account.id.clone()).or_insert(0);
                *seen += 1;
                if *seen < 3 && account.status != "Chưa đăng nhập" { continue; }
            } else {
                logged_out_seen.remove(&account.id);
            }
            // A closed window is not a logged-out account: it opens on demand when a job needs it.
            if status == "Chưa mở cửa sổ Muse" {
                if browser::settings().auto_open && browser::is_ready(&account.status) { status = browser::READY_CLOSED; }
                // Already known to be logged out: keep saying so instead of looking like a merely closed window.
                else if account.status == "Chưa đăng nhập" { continue; }
            }
            if account.status == status { continue; }
            let _guard = ACCOUNTS_LOCK.lock().unwrap();
            if let Ok(mut fresh) = load_accounts(&app) {
                if let Some(saved) = fresh.iter_mut().find(|a| a.id == account.id) {
                    saved.status = status.into();
                    if !browser::is_ready(status) { saved.plan_usage = None; saved.additional_tokens = None; }
                    let _ = save_accounts(&app, &fresh);
                }
            }
        }
    }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let runtime = std::sync::Arc::new(std::sync::Mutex::new(queue::load(app.handle())));
            app.manage(queue::Queue(runtime.clone()));
            browser::init(app.handle());
            { let app = app.handle().clone(); thread::spawn(move || browser::close_orphans(&app)); }
            api::init(app.handle().clone(), runtime.clone());
            cleanup::start_auto_clean(app.handle().clone(), runtime.clone());
            queue::start(app.handle().clone(), runtime);
            watch_accounts(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_accounts, add_account, open_account, check_account, refresh_all_usage, usage_progress, remove_account, open_output_folder, open_media_file, save_reference, queue::enqueue_jobs, queue::list_jobs, queue::get_thumb, queue::cancel_job, queue::cancel_waiting, stats::get_runtime_stats, queue::retry_job, queue::delete_job, queue::set_paused, browser::get_run_settings, browser::set_run_settings, api::get_api_settings, api::set_api_settings, api::get_api_guide, updater::get_app_version, cleanup::get_storage_info, cleanup::clear_history, cleanup::clear_cache, cleanup::get_auto_clean, cleanup::set_auto_clean, updater::check_update, updater::install_update])
        .build(tauri::generate_context!())
        .expect("error while building Muse Tool")
        .run(|app, event| {
            // Closing the app (or an update restarting it) should not leave browsers this app opened to be killed mid-write.
            if let tauri::RunEvent::Exit = event { browser::close_all_auto(app); }
        });
}
