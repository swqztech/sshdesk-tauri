use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/default/tuku-backup";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoConn {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub auth_db: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupConfig {
    pub tuku_backend_root: String,
    pub mongodb_url: String,
    pub mongo_backup_db: String,
    pub mongo_backup_dir: String,
    pub mongo_backup_retain_days: u32,
    pub oss_access_key_id: String,
    pub oss_access_key_secret: String,
    pub oss_bucket: String,
    pub oss_region: String,
    pub oss_endpoint: String,
    pub oss_internal: bool,
    pub oss_backup_root: String,
    pub oss_mongodb_prefix: String,
    pub oss_mongodb_retain_days: u32,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoDbInfo {
    pub name: String,
    pub size_on_disk: u64,
    pub empty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBackupItem {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
}

#[derive(Debug, Clone)]
pub struct BackupProgress {
    pub phase: String,
    pub message: String,
    pub percent: u32,
}

pub fn default_backup_config() -> BackupConfig {
    BackupConfig {
        tuku_backend_root: String::new(),
        mongodb_url: String::new(),
        mongo_backup_db: String::new(),
        mongo_backup_dir: "/var/backups/mongodb".into(),
        mongo_backup_retain_days: 7,
        oss_access_key_id: String::new(),
        oss_access_key_secret: String::new(),
        oss_bucket: String::new(),
        oss_region: "oss-cn-shenzhen".into(),
        oss_endpoint: "oss-cn-shenzhen.aliyuncs.com".into(),
        oss_internal: false,
        oss_backup_root: "backups".into(),
        oss_mongodb_prefix: "backups/mongodb".into(),
        oss_mongodb_retain_days: 14,
        config_path: DEFAULT_CONFIG_PATH.into(),
    }
}

pub fn parse_env_value(raw: &str) -> String {
    let mut v = raw.trim().to_string();
    if (v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')) {
        v = v[1..v.len() - 1].to_string();
    }
    v
}

pub fn parse_env_text(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some(eq) = t.find('=') else { continue };
        if eq == 0 {
            continue;
        }
        let key = t[..eq].trim();
        if !key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            continue;
        }
        map.insert(key.to_string(), parse_env_value(&t[eq + 1..]));
    }
    map
}

pub fn apply_env_map(base: &BackupConfig, env: &HashMap<String, String>) -> BackupConfig {
    let n = |k: &str, fallback: &str| -> String {
        env.get(k)
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| fallback.to_string())
    };
    let num = |k: &str, fallback: u32| -> u32 {
        env.get(k)
            .and_then(|v| v.parse().ok())
            .unwrap_or(fallback)
    };
    let bool = |k: &str, fallback: bool| -> bool {
        match env.get(k) {
            None => fallback,
            Some(v) if v.trim().is_empty() => fallback,
            Some(v) => {
                let v = v.trim().to_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            }
        }
    };
    let mut next = base.clone();
    next.tuku_backend_root = n("TUKU_BACKEND_ROOT", &next.tuku_backend_root);
    next.mongodb_url = n("MONGODB_URL", &next.mongodb_url);
    next.mongo_backup_db = n("MONGO_BACKUP_DB", &next.mongo_backup_db);
    next.mongo_backup_dir = n("MONGO_BACKUP_DIR", &next.mongo_backup_dir);
    next.mongo_backup_retain_days = num("MONGO_BACKUP_RETAIN_DAYS", next.mongo_backup_retain_days);
    next.oss_access_key_id = n("OSS_ACCESS_KEY_ID", &next.oss_access_key_id);
    next.oss_access_key_secret = n("OSS_ACCESS_KEY_SECRET", &next.oss_access_key_secret);
    next.oss_bucket = n("OSS_BUCKET", &next.oss_bucket);
    next.oss_region = n("OSS_REGION", &next.oss_region);
    next.oss_endpoint = n(
        "OSS_ENDPOINT",
        &if next.oss_endpoint.is_empty() {
            format!("{}.aliyuncs.com", next.oss_region)
        } else {
            next.oss_endpoint.clone()
        },
    );
    next.oss_internal = bool("OSS_INTERNAL", next.oss_internal);
    next.oss_backup_root = n("OSS_BACKUP_ROOT", &next.oss_backup_root);
    next.oss_mongodb_prefix = n(
        "OSS_MONGODB_PREFIX",
        &if next.oss_mongodb_prefix.is_empty() {
            format!("{}/mongodb", next.oss_backup_root)
        } else {
            next.oss_mongodb_prefix.clone()
        },
    );
    next.oss_mongodb_retain_days = num("OSS_MONGODB_RETAIN_DAYS", next.oss_mongodb_retain_days);
    next
}

pub fn serialize_backup_config(cfg: &BackupConfig) -> String {
    format!(
        "# 由连枢 SSH 客户端写入（对齐 tuku-backend scripts/backup）\n\
TUKU_BACKEND_ROOT={}\n\
\n\
MONGO_BACKUP_DB={}\n\
{}\n\
MONGO_BACKUP_DIR={}\n\
MONGO_BACKUP_RETAIN_DAYS={}\n\
\n\
OSS_ACCESS_KEY_ID={}\n\
OSS_ACCESS_KEY_SECRET={}\n\
OSS_BUCKET={}\n\
OSS_REGION={}\n\
OSS_ENDPOINT={}\n\
OSS_INTERNAL={}\n\
OSS_BACKUP_ROOT={}\n\
OSS_MONGODB_PREFIX={}\n\
OSS_MONGODB_RETAIN_DAYS={}\n",
        cfg.tuku_backend_root,
        cfg.mongo_backup_db,
        if cfg.mongodb_url.is_empty() {
            "# MONGODB_URL=".to_string()
        } else {
            format!("MONGODB_URL={}", cfg.mongodb_url)
        },
        cfg.mongo_backup_dir,
        cfg.mongo_backup_retain_days,
        cfg.oss_access_key_id,
        cfg.oss_access_key_secret,
        cfg.oss_bucket,
        cfg.oss_region,
        cfg.oss_endpoint,
        if cfg.oss_internal { "true" } else { "false" },
        cfg.oss_backup_root,
        cfg.oss_mongodb_prefix,
        cfg.oss_mongodb_retain_days,
    )
}

pub fn to_oss_config(cfg: &BackupConfig) -> crate::oss::OssConfig {
    let prefix = if cfg.oss_mongodb_prefix.is_empty() {
        format!("{}/mongodb", cfg.oss_backup_root)
    } else {
        cfg.oss_mongodb_prefix.clone()
    };
    crate::oss::OssConfig {
        access_key_id: cfg.oss_access_key_id.clone(),
        access_key_secret: cfg.oss_access_key_secret.clone(),
        bucket: cfg.oss_bucket.clone(),
        region: cfg.oss_region.clone(),
        endpoint: cfg.oss_endpoint.clone(),
        internal: Some(cfg.oss_internal),
        prefix,
    }
}

pub fn mongo_backup_name_ok(name: &str) -> bool {
    crate::mongo::backup_meta::is_mongo_backup_file_name(name)
}
