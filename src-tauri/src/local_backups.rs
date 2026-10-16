use std::fs;
use std::path::{Path, PathBuf};

use crate::mongo::backup_meta::is_mongo_backup_file_name;
use crate::qdrant::backup_meta::is_qdrant_backup_file_name;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcBackupItem {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
}

fn list_dir(dir: &str, name_ok: fn(&str) -> bool) -> Vec<PcBackupItem> {
    let root = dir.trim();
    if root.is_empty() {
        return Vec::new();
    }
    let path = Path::new(root);
    if !path.is_dir() {
        return Vec::new();
    }
    let mut items = Vec::new();
    let entries = fs::read_dir(path).ok();
    for entry in entries.into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name_ok(&name) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let full = entry.path();
        items.push(PcBackupItem {
            name,
            path: full.to_string_lossy().to_string(),
            size: meta.len(),
            mtime_ms: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        });
    }
    items.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    items
}

pub fn list_pc_mongo_backups(dir: &str) -> Vec<PcBackupItem> {
    list_dir(dir, is_mongo_backup_file_name)
}

pub fn list_pc_qdrant_backups(dir: &str) -> Vec<PcBackupItem> {
    list_dir(dir, is_qdrant_backup_file_name)
}

pub fn unique_local_path(dir: &str, file_name: &str, suffix_pattern: &str) -> PathBuf {
    let base = Path::new(dir).join(file_name);
    if !base.exists() {
        return base;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let re = regex::Regex::new(suffix_pattern).ok();
    if let Some(re) = re {
        let replaced = re.replace(file_name, format!("_{}$1", stamp));
        return Path::new(dir).join(replaced.to_string());
    }
    Path::new(dir).join(format!("{}_{}", stamp, file_name))
}
