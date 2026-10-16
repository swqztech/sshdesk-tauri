use chrono::{Datelike, Local, Timelike};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::read::ZipArchive;

pub const BACKUP_META_NAME: &str = "backup_meta.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMetaCollection {
    pub count: u64,
    pub max_id: Option<String>,
    pub update_field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoBackupMeta {
    pub version: u8,
    #[serde(rename = "type")]
    pub backup_type: String,
    pub db: String,
    pub backup_id: String,
    pub file_name: String,
    pub created_at: String,
    pub since: Option<String>,
    pub base_backup_id: String,
    pub parent_backup_id: Option<String>,
    pub collections: std::collections::HashMap<String, BackupMetaCollection>,
    pub changed_collections: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoBackupChain {
    pub version: u8,
    pub db: String,
    pub entries: Vec<MongoBackupMeta>,
}

pub fn is_mongo_backup_file_name(name: &str) -> bool {
    let base = Path::new(name).file_name().unwrap_or_default().to_string_lossy();
    Regex::new(r"(?i)^.+_bson_(incr_)?\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}_mc\.zip$")
        .map(|re| re.is_match(&base))
        .unwrap_or(false)
        || Regex::new(r"(?i)_mongodb_data\.zip$")
            .map(|re| re.is_match(&base))
            .unwrap_or(false)
        || Regex::new(r"(?i)^mongodb_.+\.archive\.gz$")
            .map(|re| re.is_match(&base))
            .unwrap_or(false)
}

pub fn is_incremental_backup_name(name: &str) -> bool {
    Regex::new(r"(?i)_bson_incr_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}_mc\.zip$")
        .map(|re| re.is_match(name))
        .unwrap_or(false)
}

pub fn parse_backup_id_from_file_name(file_name: &str) -> Option<String> {
    let base = Path::new(file_name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    Regex::new(r"(?i)_bson_(?:incr_)?(\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2})_mc\.zip$")
        .ok()?
        .captures(&base)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

pub fn parse_source_db_from_backup_file_name(file_name: &str) -> Option<String> {
    let base = Path::new(file_name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(".zip", "")
        .replace(".ZIP", "");
    Regex::new(r"(?i)^(.+)_bson_(?:incr_)?\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}")
        .ok()?
        .captures(&base)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

pub fn bt_mc_zip_name(db: &str, kind: &str) -> String {
    let now = Local::now();
    let ts = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let mid = if kind == "incremental" {
        "bson_incr"
    } else {
        "bson"
    };
    format!("{}_{}_{}_mc.zip", db, mid, ts)
}

pub fn chain_file_name(db: &str) -> String {
    format!(".sshdesk_{}_chain.json", db)
}

pub fn parse_backup_chain(text: &str) -> Option<MongoBackupChain> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }
    let data: MongoBackupChain = serde_json::from_str(raw).ok()?;
    if data.version != 1 {
        return None;
    }
    Some(data)
}

pub fn serialize_backup_chain(chain: &MongoBackupChain) -> String {
    serde_json::to_string_pretty(chain).unwrap_or_default()
}

pub fn latest_chain_entry(chain: &Option<MongoBackupChain>) -> Option<MongoBackupMeta> {
    chain
        .as_ref()
        .and_then(|c| c.entries.last().cloned())
}

pub fn read_backup_meta_from_zip(zip_path: &str) -> Option<MongoBackupMeta> {
    let path = zip_path.trim();
    if path.is_empty() {
        return None;
    }
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name(BACKUP_META_NAME).ok()?;
    let mut buf = String::new();
    entry.read_to_string(&mut buf).ok()?;
    let text = buf.trim();
    if text.is_empty() {
        return None;
    }
    serde_json::from_str(text).ok()
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBackupRef {
    pub name: String,
    pub path: String,
    pub mtime_ms: u64,
}

pub async fn resolve_local_backup_chain(
    target_path: &str,
    siblings: &[LocalBackupRef],
) -> Result<(Vec<String>, MongoBackupMeta), String> {
    let target_meta = read_backup_meta_from_zip(target_path);
    if target_meta.is_none() {
        return Ok((
            vec![target_path.to_string()],
            fallback_meta_from_name(target_path, None, None),
        ));
    }
    let target_meta = target_meta.unwrap();
    if target_meta.backup_type == "full" {
        return Ok((vec![target_path.to_string()], target_meta));
    }
    let base_id = target_meta.base_backup_id.clone();
    if base_id.is_empty() {
        return Err("增量备份缺少 baseBackupId，无法确定全量基线".into());
    }
    let db = target_meta.db.clone();
    let mut by_id: std::collections::HashMap<String, (String, MongoBackupMeta, u64)> =
        std::collections::HashMap::new();
    for item in siblings {
        if !is_mongo_backup_file_name(&item.name) {
            continue;
        }
        if let Some(hint) = parse_source_db_from_backup_file_name(&item.name) {
            if hint != db {
                continue;
            }
        }
        let id = parse_backup_id_from_file_name(&item.name);
        let id = id.ok_or_else(|| format!("无法解析备份 ID：{}", item.name))?;
        let meta = read_backup_meta_from_zip(&item.path)
            .unwrap_or_else(|| fallback_meta_from_name(&item.path, Some(id.clone()), Some(db.clone())));
        by_id.insert(id, (item.path.clone(), meta, item.mtime_ms));
    }
    let base = by_id
        .get(&base_id)
        .ok_or_else(|| {
            format!(
                "未找到全量基线 {}。请先把对应全量备份下载到同一目录（{}）",
                base_id,
                Path::new(target_path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
            )
        })?;
    let mut ordered: Vec<_> = by_id
        .values()
        .filter(|(_, meta, _)| meta.base_backup_id == base_id || meta.backup_id == base_id)
        .cloned()
        .collect();
    ordered.sort_by_key(|(_, _, ms)| *ms);
    let mut chain: Vec<String> = Vec::new();
    for (path, _, _) in &ordered {
        chain.push(path.clone());
        if path == target_path {
            break;
        }
    }
    if chain.is_empty() || chain.last().map(|s| s.as_str()) != Some(target_path) {
        chain.clear();
        chain.push(base.0.clone());
        for (path, meta, _) in ordered {
            if meta.backup_type == "incremental" {
                let p = path.clone();
                chain.push(p);
                if path == target_path {
                    break;
                }
            }
        }
    }
    if chain.first().as_deref() != Some(&base.0) {
        return Err(format!(
            "恢复链不完整：缺少全量基线 {}",
            base.1.file_name
        ));
    }
    if chain.last().map(|s| s.as_str()) != Some(target_path) {
        return Err("恢复链不完整：请下载从全量到当前增量的全部备份文件到同一目录".into());
    }
    Ok((chain, target_meta))
}

pub fn fallback_meta_from_name(
    file_path: &str,
    backup_id: Option<String>,
    db: Option<String>,
) -> MongoBackupMeta {
    let name = Path::new(file_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let id = backup_id
        .or_else(|| parse_backup_id_from_file_name(&name))
        .unwrap_or_else(|| "unknown".to_string());
    let parsed_db = db
        .or_else(|| parse_source_db_from_backup_file_name(&name))
        .unwrap_or_else(|| "restore".to_string());
    let incr = is_incremental_backup_name(&name);
    MongoBackupMeta {
        version: 1,
        backup_type: if incr { "incremental" } else { "full" }.to_string(),
        db: parsed_db,
        backup_id: id.clone(),
        file_name: name,
        created_at: chrono::Utc::now().to_rfc3339(),
        since: None,
        base_backup_id: if incr { String::new() } else { id },
        parent_backup_id: None,
        collections: std::collections::HashMap::new(),
        changed_collections: None,
    }
}
