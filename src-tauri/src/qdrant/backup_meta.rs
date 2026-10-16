use chrono::{Datelike, Local, Timelike};
use regex::Regex;
use std::path::Path;

pub fn sanitize_collection_for_backup_file_name(collection: &str) -> Result<String, String> {
    let raw = collection.trim();
    if raw.is_empty() {
        return Err("集合名不能为空".into());
    }
    let safe = Regex::new(r"[^a-zA-Z0-9_.-]+")
        .unwrap()
        .replace_all(raw, "_")
        .to_string()
        .replace("__", "_")
        .trim_matches('_')
        .to_string();
    if safe.is_empty() {
        return Err(format!("集合名无法用于文件名：{}", collection));
    }
    Ok(safe)
}

pub fn format_qdrant_backup_timestamp() -> String {
    let d = Local::now();
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        d.year(),
        d.month(),
        d.day(),
        d.hour(),
        d.minute(),
        d.second()
    )
}

pub fn build_qdrant_backup_file_name(collection: &str, ts: Option<&str>) -> Result<String, String> {
    let coll = sanitize_collection_for_backup_file_name(collection)?;
    let default_ts = format_qdrant_backup_timestamp();
    let stamp = ts.unwrap_or(&default_ts);
    Ok(format!("qdrant_{}_{}.tar.gz", coll, stamp))
}

pub fn is_qdrant_backup_file_name(name: &str) -> bool {
    let base = Path::new(name).file_name().unwrap_or_default().to_string_lossy();
    Regex::new(r"(?i)^qdrant_.+\.tar\.gz$")
        .map(|re| re.is_match(&base))
        .unwrap_or(false)
}

pub fn parse_collection_from_qdrant_backup_file_name(file_name: &str) -> Option<String> {
    let base = Path::new(file_name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if Regex::new(r"(?i)^qdrant_\d{8}_\d{6}\.tar\.gz$")
        .map(|re| re.is_match(&base))
        .unwrap_or(false)
    {
        return None;
    }
    Regex::new(r"(?i)^qdrant_(.+)_\d{8}_\d{6}\.tar\.gz$")
        .ok()?
        .captures(&base)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}
