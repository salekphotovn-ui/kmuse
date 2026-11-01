use crate::{browser, check_reference, load_accounts, media_dir, run_media_job};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};

/// Spacing between job launches, so ten accounts don't hit Muse in the same second.
const LAUNCH_GAP: Duration = Duration::from_secs(2);
/// An account that just failed rests this long before it gets another job.
const COOLDOWN: Duration = Duration::from_secs(90);
const EXHAUSTED_COOLDOWN: Duration = Duration::from_secs(30 * 60);
const MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    id: u64,
    kind: String,
    prompt: String,
    ratio: String,
    requested: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    account_id: Option<String>,
    status: String,
    message: Option<String>,
    file: Option<String>,
    attempts: u32,
    blocked: Vec<String>,
    created_ms: u64,
    started_ms: Option<u64>,
    finished_ms: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    jobs: Vec<Job>,
    next_id: u64,
}

pub struct Runtime {
    jobs: Vec<Job>,
    next_id: u64,
    paused: bool,
    /// Adaptive concurrency: grows after consecutive successes, halves after a failure.
    max_active: usize,
    streak: u32,
    cooldown: HashMap<String, Instant>,
    last_launch: Option<Instant>,
}

pub struct Queue(pub Arc<Mutex<Runtime>>);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn queue_file(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("queue.json"))
}

static DIRTY: AtomicBool = AtomicBool::new(false);

/// Marks the queue as changed. The file itself is written by `spawn_writer` a moment later, so callers holding the queue lock
/// (API requests, job dispatch) never wait for the disk.
fn persist(_app: &AppHandle, _rt: &Runtime) {
    DIRTY.store(true, Ordering::Release);
}

#[derive(Serialize)]
struct PersistedRef<'a> {
    jobs: &'a [Job],
    next_id: u64,
}

fn spawn_writer(app: AppHandle, queue: Arc<Mutex<Runtime>>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(200));
        if !DIRTY.swap(false, Ordering::AcqRel) { continue; }
        let Some(path) = queue_file(&app) else { continue };
        let text = {
            let rt = queue.lock().unwrap();
            serde_json::to_string(&PersistedRef { jobs: &rt.jobs, next_id: rt.next_id })
        };
        let Ok(text) = text else { continue };
        // Write beside the file and rename, so a crash mid-write cannot leave a half-written queue behind.
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, text).is_ok() && fs::rename(&tmp, &path).is_err() { let _ = fs::remove_file(&tmp); DIRTY.store(true, Ordering::Release); }
    });
}

/// Finished jobs older than the newest `KEEP_FINISHED` are dropped, so the queue (and every list built from it) stays small however long the app runs.
const KEEP_FINISHED: usize = 1000;

fn prune(rt: &mut Runtime) {
    let finished = rt.jobs.iter().filter(|j| j.status == "done" || j.status == "error").count();
    if finished <= KEEP_FINISHED { return; }
    let mut drop = finished - KEEP_FINISHED;
    rt.jobs.retain(|j| {
        if drop > 0 && (j.status == "done" || j.status == "error") { drop -= 1; false } else { true }
    });
}

pub fn load(app: &AppHandle) -> Runtime {
    let saved: Persisted = queue_file(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut jobs = saved.jobs;
    // A job that was running when the app closed already sent its prompt to Muse. Running it again on its own would spend tokens twice, so it is
    // reported as interrupted and left to the user (or the API caller) to retry on purpose.
    for job in jobs.iter_mut().filter(|j| j.status == "running") {
        job.status = "error".into();
        job.account_id = None;
        job.finished_ms = Some(now_ms());
        job.message = Some("Bị gián đoạn vì app đã tắt khi đang chạy. Bấm Thử lại nếu cần (Muse có thể đã tạo xong kết quả trong chat).".into());
    }
    Runtime { jobs, next_id: saved.next_id.max(1), paused: false, max_active: 64, streak: 0, cooldown: HashMap::new(), last_launch: None }
}

pub fn start(app: AppHandle, queue: Arc<Mutex<Runtime>>) {
    spawn_writer(app.clone(), queue.clone());
    thread::spawn(move || {
        let mut tick = 0u32;
        let mut next_usage_refresh = Instant::now() + Duration::from_secs(45);
        loop {
            thread::sleep(Duration::from_secs(1));
            dispatch(&app, &queue);
            tick += 1;
            if tick % 15 == 0 {
                let busy = busy_accounts(&queue);
                browser::close_idle(&app, &busy);
            }
            // Keep the token counts fresh, but only while nothing is queued or running so it never competes with real work.
            if Instant::now() >= next_usage_refresh {
                let idle = queue.lock().unwrap().jobs.iter().all(|j| j.status != "waiting" && j.status != "running");
                if idle {
                    let _ = crate::start_usage_refresh(&app, &queue);
                    next_usage_refresh = Instant::now() + USAGE_REFRESH_EVERY;
                } else {
                    next_usage_refresh = Instant::now() + Duration::from_secs(60);
                }
            }
        }
    });
}

/// How often the token counts of all accounts are re-read while the app is idle.
const USAGE_REFRESH_EVERY: Duration = Duration::from_secs(30 * 60);

fn dispatch(app: &AppHandle, queue: &Arc<Mutex<Runtime>>) {
    let Ok(accounts) = load_accounts(app) else { return };
    let connected: Vec<String> = accounts.iter().filter(|a| browser::is_usable(&a.status)).map(|a| a.id.clone()).collect();
    let (job_id, account_id, kind, prompt, ratio, reference) = {
        let mut rt = queue.lock().unwrap();
        if rt.paused || connected.is_empty() { return; }
        let running = rt.jobs.iter().filter(|j| j.status == "running").count();
        if running >= rt.max_active.min(connected.len()).max(1) { return; }
        if rt.last_launch.is_some_and(|t| t.elapsed() < LAUNCH_GAP) { return; }
        let now = Instant::now();
        rt.cooldown.retain(|_, until| *until > now);
        // One job per account at a time, images and videos counted together.
        let busy: HashSet<&String> = rt.jobs.iter().filter(|j| j.status == "running").filter_map(|j| j.account_id.as_ref()).collect();
        let free: Vec<&String> = connected.iter().filter(|id| !busy.contains(id) && !rt.cooldown.contains_key(*id)).collect();
        if free.is_empty() { return; }
        let mut pick: Option<(usize, String)> = None;
        for (index, job) in rt.jobs.iter().enumerate().filter(|(_, j)| j.status == "waiting") {
            let allowed: Vec<&&String> = free.iter().filter(|id| job.requested.as_ref().map_or(true, |r| r == **id)).collect();
            let chosen = allowed.iter().find(|id| !job.blocked.contains(***id)).or(allowed.first());
            if let Some(id) = chosen { pick = Some((index, (***id).clone())); break; }
        }
        let Some((index, account_id)) = pick else { return };
        let job = &mut rt.jobs[index];
        job.status = "running".into();
        job.account_id = Some(account_id.clone());
        job.started_ms = Some(now_ms());
        job.message = None;
        let info = (job.id, account_id, job.kind.clone(), job.prompt.clone(), job.ratio.clone(), job.reference.clone());
        rt.last_launch = Some(now);
        persist(app, &rt);
        info
    };

    let app = app.clone();
    let queue = queue.clone();
    thread::spawn(move || {
        let result = browser::ensure_open(&app, &account_id).and_then(|port| run_media_job(port, &kind, &prompt, &ratio, reference.as_deref()));
        finish(&app, &queue, job_id, &account_id, result);
    });
}

fn thumb_path(id: u64) -> Result<PathBuf, String> {
    let mut dir = media_dir()?;
    dir.push(".thumbs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("{id}.txt")))
}

fn finish(app: &AppHandle, queue: &Arc<Mutex<Runtime>>, job_id: u64, account_id: &str, result: Result<crate::MediaResult, String>) {
    browser::touch(account_id);
    let mut rt = queue.lock().unwrap();
    let Some(index) = rt.jobs.iter().position(|j| j.id == job_id) else { return };
    match result {
        Ok(media) => {
            if let (Some(thumb), Ok(path)) = (media.thumb, thumb_path(job_id)) { let _ = fs::write(path, thumb); }
            let job = &mut rt.jobs[index];
            job.status = "done".into();
            job.file = Some(media.path);
            job.message = None;
            job.finished_ms = Some(now_ms());
            rt.streak += 1;
            if rt.streak >= 3 && rt.max_active < 64 { rt.max_active += 1; rt.streak = 0; }
        }
        Err(error) => {
            let exhausted = error.starts_with(crate::OUT_OF_USAGE);
            if exhausted {
                // Not a Muse hiccup: keep this account out of rotation for a while without slowing the others down.
                rt.cooldown.insert(account_id.to_string(), Instant::now() + EXHAUSTED_COOLDOWN);
            } else {
                rt.streak = 0;
                let running = rt.jobs.iter().filter(|j| j.status == "running").count();
                rt.max_active = (running.max(2) / 2).max(1);
                rt.cooldown.insert(account_id.to_string(), Instant::now() + COOLDOWN);
            }
            let job = &mut rt.jobs[index];
            job.attempts += 1;
            job.blocked.push(account_id.to_string());
            job.account_id = None;
            // A job pinned to the exhausted account has nowhere else to go.
            if exhausted && job.requested.is_some() { job.attempts = MAX_ATTEMPTS; }
            if job.attempts < MAX_ATTEMPTS {
                job.status = "waiting".into();
                job.message = Some(format!("Thử lại ({}/{}): {error}", job.attempts, MAX_ATTEMPTS - 1));
            } else {
                job.status = "error".into();
                job.message = Some(error);
                job.finished_ms = Some(now_ms());
            }
        }
    }
    prune(&mut rt);
    persist(app, &rt);
}

#[derive(Serialize)]
pub struct JobView {
    pub id: u64,
    pub kind: String,
    pub prompt: String,
    pub ratio: String,
    pub status: String,
    pub message: Option<String>,
    pub file: Option<String>,
    pub reference: Option<String>,
    pub account: Option<String>,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
}

#[derive(Serialize)]
pub struct JobsView {
    jobs: Vec<JobView>,
    paused: bool,
    now_ms: u64,
}

#[tauri::command]
pub fn enqueue_jobs(app: AppHandle, queue: State<Queue>, kind: String, prompts: Vec<String>, ratio: String, quantity: u32, account: Option<String>, reference: Option<String>) -> Result<usize, String> {
    if kind != "image" && kind != "video" { return Err("Loại công việc không hợp lệ.".into()); }
    let reference = match reference.as_deref().filter(|r| !r.is_empty()) { Some(r) => Some(check_reference(r)?), None => None };
    let mut rt = queue.0.lock().unwrap();
    let mut added = 0;
    for prompt in prompts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        for _ in 0..quantity.clamp(1, 50) {
            let id = rt.next_id;
            rt.next_id += 1;
            rt.jobs.push(Job {
                id, kind: kind.clone(), prompt: prompt.to_string(), ratio: ratio.clone(), requested: account.clone(), reference: reference.clone(), account_id: None,
                status: "waiting".into(), message: None, file: None, attempts: 0, blocked: Vec::new(), created_ms: now_ms(), started_ms: None, finished_ms: None,
            });
            added += 1;
        }
    }
    persist(&app, &rt);
    Ok(added)
}

#[tauri::command]
pub fn list_jobs(app: AppHandle, queue: State<Queue>) -> Result<JobsView, String> {
    let names = account_names(&app);
    let rt = queue.0.lock().unwrap();
    // The window polls this every second: everything unfinished, plus the newest finished jobs, instead of the whole history.
    let mut finished = 0;
    let jobs = rt.jobs.iter().rev().filter(|j| {
        if j.status == "done" || j.status == "error" { finished += 1; finished <= 200 } else { true }
    }).map(|j| make_view(j, &names)).collect();
    Ok(JobsView { jobs, paused: rt.paused, now_ms: now_ms() })
}

#[tauri::command]
pub fn get_thumb(id: u64) -> Option<String> {
    fs::read_to_string(thumb_path(id).ok()?).ok()
}

#[tauri::command]
pub fn cancel_job(app: AppHandle, queue: State<Queue>, id: u64) -> Result<(), String> {
    let mut rt = queue.0.lock().unwrap();
    let before = rt.jobs.len();
    rt.jobs.retain(|j| !(j.id == id && j.status == "waiting"));
    if rt.jobs.len() == before { return Err("Chỉ hủy được công việc đang chờ.".into()); }
    persist(&app, &rt);
    Ok(())
}

#[tauri::command]
pub fn retry_job(app: AppHandle, queue: State<Queue>, id: u64) -> Result<(), String> {
    let mut rt = queue.0.lock().unwrap();
    let job = rt.jobs.iter_mut().find(|j| j.id == id && j.status == "error").ok_or("Không tìm thấy công việc lỗi.")?;
    job.status = "waiting".into();
    job.attempts = 0;
    job.message = None;
    persist(&app, &rt);
    Ok(())
}

#[tauri::command]
pub fn delete_job(app: AppHandle, queue: State<Queue>, id: u64) -> Result<(), String> {
    let mut rt = queue.0.lock().unwrap();
    rt.jobs.retain(|j| !(j.id == id && j.status != "running"));
    persist(&app, &rt);
    Ok(())
}

#[tauri::command]
pub fn set_paused(queue: State<Queue>, paused: bool) {
    queue.0.lock().unwrap().paused = paused;
}

// ---- helpers shared with the local HTTP API (api.rs) ----

fn make_view(j: &Job, names: &HashMap<String, String>) -> JobView {
    JobView {
        id: j.id, kind: j.kind.clone(), prompt: j.prompt.clone(), ratio: j.ratio.clone(), status: j.status.clone(), message: j.message.clone(), file: j.file.clone(), reference: j.reference.clone(),
        account: j.account_id.as_ref().and_then(|id| names.get(id).cloned()), started_ms: j.started_ms, finished_ms: j.finished_ms,
    }
}

/// Names change rarely but API clients poll constantly, so reuse the account file's contents for a couple of seconds.
fn account_names(app: &AppHandle) -> HashMap<String, String> {
    static CACHE: Mutex<Option<(Instant, HashMap<String, String>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, names)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(2) { return names.clone(); }
    }
    let names: HashMap<String, String> = load_accounts(app).map(|accounts| accounts.into_iter().map(|a| (a.id, a.name)).collect()).unwrap_or_default();
    *cache = Some((Instant::now(), names.clone()));
    names
}

/// One request item for `api_enqueue_many`; check it with `validate_new` first.
pub struct NewJob {
    pub kind: String,
    pub prompt: String,
    pub ratio: String,
    pub quantity: u32,
    pub account: Option<String>,
    pub reference: Option<String>,
}

pub fn validate_new(kind: &str, prompt: &str) -> Result<(), String> {
    if kind != "image" && kind != "video" { return Err("kind phải là \"image\" hoặc \"video\".".into()); }
    let prompt = prompt.trim();
    if prompt.is_empty() { return Err("prompt không được để trống.".into()); }
    if prompt.chars().count() > 4000 { return Err("prompt quá dài (tối đa 4000 ký tự).".into()); }
    Ok(())
}

/// Adds every item under one lock and one save, so a request with many prompts costs the same as one with a single prompt.
/// Returns the new job ids of each item, in order.
pub fn api_enqueue_many(app: &AppHandle, rt: &Arc<Mutex<Runtime>>, items: Vec<NewJob>) -> Vec<Vec<u64>> {
    let mut rt = rt.lock().unwrap();
    let created = now_ms();
    let mut all = Vec::with_capacity(items.len());
    for item in items {
        let mut ids = Vec::new();
        for _ in 0..item.quantity.clamp(1, 50) {
            let id = rt.next_id;
            rt.next_id += 1;
            rt.jobs.push(Job {
                id, kind: item.kind.clone(), prompt: item.prompt.trim().to_string(), ratio: item.ratio.clone(), requested: item.account.clone(), reference: item.reference.clone(), account_id: None,
                status: "waiting".into(), message: None, file: None, attempts: 0, blocked: Vec::new(), created_ms: created, started_ms: None, finished_ms: None,
            });
            ids.push(id);
        }
        all.push(ids);
    }
    persist(app, &rt);
    all
}

pub fn api_job(app: &AppHandle, rt: &Arc<Mutex<Runtime>>, id: u64) -> Option<JobView> {
    let names = account_names(app);
    let rt = rt.lock().unwrap();
    rt.jobs.iter().find(|j| j.id == id).map(|j| make_view(j, &names))
}

pub fn api_jobs(app: &AppHandle, rt: &Arc<Mutex<Runtime>>, limit: usize) -> Vec<JobView> {
    let names = account_names(app);
    let rt = rt.lock().unwrap();
    rt.jobs.iter().rev().take(limit).map(|j| make_view(j, &names)).collect()
}

pub fn api_cancel(app: &AppHandle, rt: &Arc<Mutex<Runtime>>, id: u64) -> Result<(), String> {
    let mut rt = rt.lock().unwrap();
    let before = rt.jobs.len();
    rt.jobs.retain(|j| !(j.id == id && j.status == "waiting"));
    if rt.jobs.len() == before { return Err("Chỉ hủy được công việc đang chờ.".into()); }
    persist(app, &rt);
    Ok(())
}

pub fn busy_accounts(rt: &Arc<Mutex<Runtime>>) -> HashSet<String> {
    rt.lock().unwrap().jobs.iter().filter(|j| j.status == "running").filter_map(|j| j.account_id.clone()).collect()
}

pub fn is_paused(rt: &Arc<Mutex<Runtime>>) -> bool {
    rt.lock().unwrap().paused
}
