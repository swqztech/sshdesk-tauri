// 发布版不弹出 Windows 控制台黑框（仅 GUI）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sshdesk_tauri_lib::run()
}