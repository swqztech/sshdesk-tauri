use std::fs;
use std::path::PathBuf;

/// 与 Electron / Node 版对齐：~/.sshdesk
pub fn user_data_dir() -> PathBuf {
    let dir = std::env::var("SSHDESK_USER_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            directories::UserDirs::new()
                .map(|u| u.home_dir().join(".sshdesk"))
                .unwrap_or_else(|| PathBuf::from(".sshdesk"))
        });
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| user_data_dir().join("Downloads"))
}

/// 桌面端暂用明文前缀（与 Node platform.ts 一致）
pub fn encrypt_secret(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    format!("plain:{}", plain)
}

pub fn decrypt_secret(stored: Option<&str>) -> String {
    match stored {
        None | Some("") => String::new(),
        Some(s) if s.starts_with("enc:") => {
            let b64 = s.trim_start_matches("enc:");
            if let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
            {
                String::from_utf8_lossy(&bytes).to_string()
            } else {
                String::new()
            }
        }
        Some(s) if s.starts_with("plain:") => s.trim_start_matches("plain:").to_string(),
        Some(s) => s.to_string(),
    }
}
