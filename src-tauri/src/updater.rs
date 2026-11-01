use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

#[derive(Serialize)]
pub struct UpdateInfo {
    version: String,
    notes: Option<String>,
}

#[tauri::command]
pub fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Asks the release feed whether a newer signed version exists.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| format!("Không kiểm tra được bản cập nhật: {e}"))?;
    Ok(update.map(|u| UpdateInfo { version: u.version.clone(), notes: u.body.clone() }))
}

/// Downloads the newer version, installs it and restarts. Refuses while jobs are running, since restarting would interrupt them.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let queue = app.state::<crate::queue::Queue>();
    if !crate::queue::busy_accounts(&queue.0).is_empty() {
        return Err("Đang có việc chạy. Hãy đợi xong hoặc tạm dừng rồi cập nhật để không làm gián đoạn.".into());
    }
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?.ok_or("Đang là bản mới nhất.")?;
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| format!("Cập nhật thất bại: {e}"))?;
    app.restart();
}
