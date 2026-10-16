use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/default/qdrant";
pub const TUKU_CONFIG_PATH: &str = "/etc/default/tuku-backup";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QdrantConfig {
    pub qdrant_url: String,
    pub qdrant_api_key: String,
    pub qdrant_collection: String,
    pub qdrant_backup_dir: String,
    pub qdrant_backup_retain_days: u32,
    pub qdrant_data_dir: String,
    pub qdrant_config_file: String,
    pub tuku_backend_root: String,
    pub oss_access_key_id: String,
    pub oss_access_key_secret: String,
    pub oss_bucket: String,
    pub oss_region: String,
    pub oss_endpoint: String,
    pub oss_internal: bool,
    pub oss_backup_root: String,
    pub oss_qdrant_prefix: String,
    pub oss_qdrant_retain_days: u32,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QdrantCollectionInfo {
    pub name: String,
    pub points_count: u64,
    pub status: String,
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
pub struct QdrantProgress {
    pub phase: String,
    pub message: String,
    pub percent: u32,
}

pub fn default_qdrant_config() -> QdrantConfig {
    QdrantConfig {
        qdrant_url: "http://127.0.0.1:6333".into(),
        qdrant_api_key: String::new(),
        qdrant_collection: "gallery_assets".into(),
        qdrant_backup_dir: "/var/backups/qdrant".into(),
        qdrant_backup_retain_days: 7,
        qdrant_data_dir: "/var/lib/qdrant".into(),
        qdrant_config_file: "/etc/qdrant/config.yaml".into(),
        tuku_backend_root: String::new(),
        oss_access_key_id: String::new(),
        oss_access_key_secret: String::new(),
        oss_bucket: String::new(),
        oss_region: "oss-cn-shenzhen".into(),
        oss_endpoint: "oss-cn-shenzhen.aliyuncs.com".into(),
        oss_internal: false,
        oss_backup_root: "backups".into(),
        oss_qdrant_prefix: "backups/qdrant".into(),
        oss_qdrant_retain_days: 14,
        config_path: DEFAULT_CONFIG_PATH.into(),
    }
}

pub fn parse_env_value(raw: &str) -> String {
    crate::mongo::config::parse_env_value(raw)
}

pub fn parse_env_text(text: &str) -> HashMap<String, String> {
    crate::mongo::config::parse_env_text(text)
}

pub fn apply_env_map(base: &QdrantConfig, env: &HashMap<String, String>) -> QdrantConfig {
    let n = |k: &str, fallback: &str| -> String {
        env.get(k)
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| fallback.to_string())
    };
    let num = |k: &str, fallback: u32| -> u32 {
        env.get(k).and_then(|v| v.parse().ok()).unwrap_or(fallback)
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
    next.qdrant_url = n("QDRANT_URL", &next.qdrant_url);
    next.qdrant_api_key = n("QDRANT_API_KEY", &next.qdrant_api_key);
    next.qdrant_collection = n("QDRANT_COLLECTION", &next.qdrant_collection);
    next.qdrant_backup_dir = n("QDRANT_BACKUP_DIR", &next.qdrant_backup_dir);
    next.qdrant_backup_retain_days =
        num("QDRANT_BACKUP_RETAIN_DAYS", next.qdrant_backup_retain_days);
    next.qdrant_data_dir = n("QDRANT_DATA_DIR", &next.qdrant_data_dir);
    next.qdrant_config_file = n("QDRANT_CONFIG_FILE", &next.qdrant_config_file);
    next.tuku_backend_root = n("TUKU_BACKEND_ROOT", &next.tuku_backend_root);
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
    next.oss_qdrant_prefix = n(
        "OSS_QDRANT_PREFIX",
        &if next.oss_qdrant_prefix.is_empty() {
            format!("{}/qdrant", next.oss_backup_root)
        } else {
            next.oss_qdrant_prefix.clone()
        },
    );
    next.oss_qdrant_retain_days = num("OSS_QDRANT_RETAIN_DAYS", next.oss_qdrant_retain_days);
    next
}

pub fn serialize_qdrant_config(cfg: &QdrantConfig) -> String {
    format!(
        "# 由连枢 SSH 客户端写入（对齐 tuku-backend scripts/qdrant）\n\
QDRANT_URL={}\n\
{}\n\
QDRANT_COLLECTION={}\n\
QDRANT_BACKUP_DIR={}\n\
QDRANT_BACKUP_RETAIN_DAYS={}\n\
QDRANT_DATA_DIR={}\n\
QDRANT_CONFIG_FILE={}\n\
\n\
TUKU_BACKEND_ROOT={}\n\
OSS_ACCESS_KEY_ID={}\n\
OSS_ACCESS_KEY_SECRET={}\n\
OSS_BUCKET={}\n\
OSS_REGION={}\n\
OSS_ENDPOINT={}\n\
OSS_INTERNAL={}\n\
OSS_BACKUP_ROOT={}\n\
OSS_QDRANT_PREFIX={}\n\
OSS_QDRANT_RETAIN_DAYS={}\n",
        cfg.qdrant_url,
        if cfg.qdrant_api_key.is_empty() {
            "# QDRANT_API_KEY=".to_string()
        } else {
            format!("QDRANT_API_KEY={}", cfg.qdrant_api_key)
        },
        cfg.qdrant_collection,
        cfg.qdrant_backup_dir,
        cfg.qdrant_backup_retain_days,
        cfg.qdrant_data_dir,
        cfg.qdrant_config_file,
        cfg.tuku_backend_root,
        cfg.oss_access_key_id,
        cfg.oss_access_key_secret,
        cfg.oss_bucket,
        cfg.oss_region,
        cfg.oss_endpoint,
        if cfg.oss_internal { "true" } else { "false" },
        cfg.oss_backup_root,
        cfg.oss_qdrant_prefix,
        cfg.oss_qdrant_retain_days,
    )
}

pub fn to_oss_config(cfg: &QdrantConfig) -> crate::oss::OssConfig {
    let prefix = if cfg.oss_qdrant_prefix.is_empty() {
        format!("{}/qdrant", cfg.oss_backup_root)
    } else {
        cfg.oss_qdrant_prefix.clone()
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

pub fn qdrant_backup_name_ok(name: &str) -> bool {
    crate::qdrant::backup_meta::is_qdrant_backup_file_name(name)
}
