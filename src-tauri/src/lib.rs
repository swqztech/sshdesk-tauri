mod events;
mod host_metrics;
mod hosts;
mod local_backups;
mod mongo;
mod oss;
mod persist_state;
mod platform;
mod qdrant;
mod rpc;
mod sessions;
mod ssh_remote;

use sessions::SessionManager;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(SessionManager::new(handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc::sshdesk_rpc])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
