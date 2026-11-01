use crate::{check_reference, load_accounts, media_dir, queue};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, State};
use tiny_http::{Header, Method, Request, Response, Server};

type Rt = Arc<Mutex<queue::Runtime>>;

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiSettings {
    pub enabled: bool,
    pub port: u16,
}

impl Default for ApiSettings {
    fn default() -> Self { Self { enabled: true, port: 17890 } }
}

#[derive(Serialize)]
pub struct ApiStatus {
    enabled: bool,
    port: u16,
    running: bool,
    error: Option<String>,
}

pub struct ApiState {
    settings: Mutex<ApiSettings>,
    server: Mutex<Option<Arc<Server>>>,
    error: Mutex<Option<String>>,
    app: AppHandle,
    queue: Rt,
}

fn settings_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("api.json"))
}

pub fn init(app: AppHandle, queue: Rt) {
    let settings: ApiSettings = settings_file(&app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    app.manage(ApiState { settings: Mutex::new(settings), server: Mutex::new(None), error: Mutex::new(None), app: app.clone(), queue });
    let state = app.state::<ApiState>();
    restart(&state);
}

fn restart(state: &ApiState) {
    if let Some(old) = state.server.lock().unwrap().take() {
        old.unblock();
        thread::sleep(Duration::from_millis(250));
    }
    *state.error.lock().unwrap() = None;
    let settings = state.settings.lock().unwrap().clone();
    if !settings.enabled { return; }
    // Loopback only: other machines on the LAN cannot reach this API.
    match Server::http(("127.0.0.1", settings.port)) {
        Ok(server) => {
            let server = Arc::new(server);
            *state.server.lock().unwrap() = Some(server.clone());
            let app = state.app.clone();
            let queue = state.queue.clone();
            let port = settings.port;
            thread::spawn(move || {
                for request in server.incoming_requests() {
                    let (app, queue) = (app.clone(), queue.clone());
                    thread::spawn(move || handle(&app, &queue, port, request));
                }
            });
        }
        Err(e) => *state.error.lock().unwrap() = Some(format!("Không mở được cổng {}: {e}", settings.port)),
    }
}

fn status(state: &ApiState) -> ApiStatus {
    let settings = state.settings.lock().unwrap().clone();
    ApiStatus { enabled: settings.enabled, port: settings.port, running: state.server.lock().unwrap().is_some(), error: state.error.lock().unwrap().clone() }
}

#[tauri::command]
pub fn get_api_settings(state: State<ApiState>) -> ApiStatus { status(&state) }

#[tauri::command]
pub fn set_api_settings(state: State<ApiState>, enabled: bool, port: u16) -> Result<ApiStatus, String> {
    if port < 1024 { return Err("Cổng phải từ 1024 trở lên.".into()); }
    let settings = ApiSettings { enabled, port };
    if let Some(path) = settings_file(&state.app) {
        fs::write(path, serde_json::to_string(&settings).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    *state.settings.lock().unwrap() = settings;
    restart(&state);
    Ok(status(&state))
}

type Reply = Response<std::io::Cursor<Vec<u8>>>;

/// The usage guide for other AI agents, with this machine's address and output folder filled in.
pub fn guide(port: u16) -> String {
    let out = media_dir().map(|p| p.display().to_string().trim_start_matches(r"\\?\").to_string()).unwrap_or_else(|_| "output".into());
    include_str!("api_guide.md")
        .replace("__BASE__", &format!("http://127.0.0.1:{port}"))
        .replace("__OUTJSON__", &out.replace('\\', "\\\\"))
        .replace("__OUT__", &out)
}

#[tauri::command]
pub fn get_api_guide(state: State<ApiState>) -> String {
    guide(state.settings.lock().unwrap().port)
}

fn json(code: u16, value: serde_json::Value) -> Reply {
    let header = Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap();
    Response::from_string(value.to_string()).with_status_code(code).with_header(header)
}

fn fail(code: u16, message: &str) -> Reply {
    json(code, serde_json::json!({ "error": message }))
}

#[derive(Deserialize, Default)]
struct Item {
    kind: Option<String>,
    prompt: Option<String>,
    prompts: Option<Vec<String>>,
    ratio: Option<String>,
    quantity: Option<u32>,
    account: Option<String>,
    /// Id of a finished job whose result file is used as the reference image.
    reference_job: Option<u64>,
    /// A file inside the results folder.
    reference_path: Option<String>,
}

#[derive(Deserialize)]
struct Body {
    jobs: Option<Vec<Item>>,
    #[serde(flatten)]
    item: Item,
}

fn resolve_account(app: &AppHandle, wanted: &Option<String>) -> Result<Option<String>, String> {
    let Some(wanted) = wanted.as_deref().map(str::trim).filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case("auto")) else { return Ok(None) };
    let accounts = load_accounts(app)?;
    accounts.iter().find(|a| a.id == wanted || a.name.eq_ignore_ascii_case(wanted)).map(|a| Some(a.id.clone())).ok_or_else(|| format!("Không tìm thấy tài khoản \"{wanted}\"."))
}

fn view_json(view: &queue::JobView, port: u16) -> serde_json::Value {
    let mut value = serde_json::to_value(view).unwrap_or_default();
    if view.status == "done" { value["file_url"] = format!("http://127.0.0.1:{port}/jobs/{}/file", view.id).into(); }
    value
}

fn create_jobs(app: &AppHandle, queue: &Rt, text: &str) -> Reply {
    let body: Body = match serde_json::from_str(text) {
        Ok(body) => body,
        Err(e) => return fail(400, &format!("JSON không hợp lệ: {e}")),
    };
    let items = body.jobs.unwrap_or_else(|| vec![body.item]);
    let mut created: Vec<serde_json::Value> = Vec::new();
    let mut pending: Vec<queue::NewJob> = Vec::new();
    let mut error: Option<String> = None;
    'items: for item in &items {
        let account = match resolve_account(app, &item.account) {
            Ok(account) => account,
            Err(e) => { error = Some(e); break; }
        };
        let reference = match (item.reference_job, &item.reference_path) {
            (Some(id), _) => match queue::api_job(app, queue, id).and_then(|v| v.file) {
                Some(file) => Some(file),
                None => { error = Some(format!("Công việc #{id} chưa có ảnh để làm tham chiếu.")); break; }
            },
            (None, Some(path)) => Some(path.clone()),
            _ => None,
        };
        let reference = match reference.as_deref().map(check_reference) {
            Some(Ok(path)) => Some(path),
            Some(Err(e)) => { error = Some(e); break; }
            None => None,
        };
        let kind = item.kind.as_deref().unwrap_or("image");
        let ratio = item.ratio.as_deref().unwrap_or("16:9");
        let mut prompts: Vec<String> = item.prompts.clone().unwrap_or_default();
        if let Some(p) = &item.prompt { prompts.insert(0, p.clone()); }
        if prompts.is_empty() { error = Some("Thiếu prompt.".into()); break; }
        for prompt in prompts {
            if let Err(e) = queue::validate_new(kind, &prompt) { error = Some(e); break 'items; }
            pending.push(queue::NewJob { kind: kind.to_string(), prompt, ratio: ratio.to_string(), quantity: item.quantity.unwrap_or(1), account: account.clone(), reference: reference.clone() });
        }
    }
    // Everything valid so far goes in under one lock and one save, however many prompts the request carries.
    let described: Vec<(String, String)> = pending.iter().map(|n| (n.kind.clone(), n.prompt.clone())).collect();
    for ((kind, prompt), ids) in described.into_iter().zip(queue::api_enqueue_many(app, queue, pending)) {
        created.extend(ids.into_iter().map(|id| serde_json::json!({ "id": id, "kind": kind, "prompt": prompt })));
    }
    let ids: Vec<serde_json::Value> = created.iter().map(|j| j["id"].clone()).collect();
    match error {
        Some(e) if created.is_empty() => fail(400, &e),
        Some(e) => json(207, serde_json::json!({ "jobs": created, "ids": ids, "error": e })),
        None => json(201, serde_json::json!({ "jobs": created, "ids": ids })),
    }
}

fn get_job(app: &AppHandle, queue: &Rt, port: u16, id: u64, wait: u64) -> Reply {
    // wait=N holds the request open until the job finishes (max 600 s), so a caller needs one request instead of a polling loop.
    let start = Instant::now();
    loop {
        let Some(view) = queue::api_job(app, queue, id) else { return fail(404, "Không tìm thấy công việc.") };
        let finished = view.status == "done" || view.status == "error";
        if finished || start.elapsed().as_secs() >= wait { return json(200, view_json(&view, port)); }
        thread::sleep(Duration::from_millis(500));
    }
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

fn handle(app: &AppHandle, queue: &Rt, port: u16, mut request: Request) {
    // Browsers attach these headers to cross-site requests; AI clients and scripts do not. Refusing them stops a web page from driving the API.
    if request.headers().iter().any(|h| h.field.equiv("Origin") || h.field.equiv("Sec-Fetch-Site")) {
        let _ = request.respond(fail(403, "Yêu cầu từ trình duyệt không được phép."));
        return;
    }
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let segments: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let method = request.method().clone();

    let response = match (&method, segments.as_slice()) {
        (Method::Get, ["docs"]) => {
            let header = Header::from_bytes("Content-Type", "text/markdown; charset=utf-8").unwrap();
            Response::from_string(guide(port)).with_header(header)
        }
        (Method::Get, []) | (Method::Get, ["health"]) => json(200, serde_json::json!({
            "ok": true,
            "docs": format!("http://127.0.0.1:{port}/docs"),
            "paused": queue::is_paused(queue),
            "endpoints": ["GET /accounts", "POST /jobs", "GET /jobs", "GET /jobs/{id}?wait=SECONDS", "GET /jobs/{id}/file", "DELETE /jobs/{id}"],
        })),
        (Method::Post, ["accounts", "refresh"]) => match crate::start_usage_refresh(app, queue) {
            Ok(()) => { let (running, done, total) = crate::usage_state(); json(200, serde_json::json!({ "running": running, "done": done, "total": total })) }
            Err(e) => fail(500, &e),
        },
        (Method::Get, ["accounts"]) => {
            let busy = queue::busy_accounts(queue);
            let accounts: Vec<_> = load_accounts(app).unwrap_or_default().into_iter().map(|a| serde_json::json!({
                "id": a.id, "name": a.name, "status": a.status, "connected": crate::browser::is_usable(&a.status), "busy": busy.contains(&a.id),
                "usage": a.plan_usage, "tokens_left": a.additional_tokens,
            })).collect();
            json(200, serde_json::json!({ "accounts": accounts }))
        }
        (Method::Post, ["jobs"]) => {
            let mut text = String::new();
            let _ = request.as_reader().take(1_000_000).read_to_string(&mut text);
            create_jobs(app, queue, &text)
        }
        (Method::Get, ["jobs"]) => {
            let limit = query_value(query, "limit").and_then(|v| v.parse().ok()).unwrap_or(50usize).min(500);
            let jobs: Vec<_> = queue::api_jobs(app, queue, limit).iter().map(|v| view_json(v, port)).collect();
            json(200, serde_json::json!({ "jobs": jobs }))
        }
        (Method::Get, ["jobs", id]) => match id.parse::<u64>() {
            Ok(id) => get_job(app, queue, port, id, query_value(query, "wait").and_then(|v| v.parse().ok()).unwrap_or(0u64).min(600)),
            Err(_) => fail(400, "id không hợp lệ."),
        },
        (Method::Get, ["jobs", id, "file"]) => {
            let path = id.parse::<u64>().ok().and_then(|id| queue::api_job(app, queue, id)).and_then(|v| v.file);
            match path.as_ref().map(|p| (p, fs::read(p))) {
                Some((path, Ok(bytes))) => {
                    let mime = match path.rsplit('.').next().unwrap_or("") { "mp4" => "video/mp4", "webp" => "image/webp", "jpg" | "jpeg" => "image/jpeg", _ => "image/png" };
                    let _ = request.respond(Response::from_data(bytes).with_header(Header::from_bytes("Content-Type", mime).unwrap()));
                    return;
                }
                Some((_, Err(_))) => fail(404, "File không còn tồn tại."),
                None => fail(404, "Công việc chưa có file."),
            }
        }
        (Method::Delete, ["jobs", id]) => match id.parse::<u64>() {
            Ok(id) => match queue::api_cancel(app, queue, id) {
                Ok(()) => json(200, serde_json::json!({ "cancelled": true })),
                Err(e) => fail(409, &e),
            },
            Err(_) => fail(400, "id không hợp lệ."),
        },
        _ => fail(404, "Đường dẫn không tồn tại."),
    };
    let _ = request.respond(response);
}
