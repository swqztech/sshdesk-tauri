use base64::Engine;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

type HmacSha1 = Hmac<Sha1>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OssConfig {
    pub access_key_id: String,
    pub access_key_secret: String,
    pub bucket: String,
    pub region: String,
    pub endpoint: String,
    pub internal: Option<bool>,
    pub prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OssBackupItem {
    pub key: String,
    pub name: String,
    pub size: u64,
    pub last_modified: String,
}

fn endpoint_host(cfg: &OssConfig) -> String {
    let ep = cfg
        .endpoint
        .trim()
        .replace("https://", "")
        .replace("http://", "");
    if ep.is_empty() {
        format!("{}.aliyuncs.com", cfg.region.trim())
    } else {
        ep
    }
}

fn base_url(cfg: &OssConfig) -> String {
    format!("https://{}.{}/", cfg.bucket.trim(), endpoint_host(cfg))
}

fn canonical_resource(cfg: &OssConfig, resource: &str) -> String {
    let bucket = cfg.bucket.trim();
    let r = resource.trim_start_matches('/');
    if r.is_empty() {
        format!("/{}/", bucket)
    } else if r.starts_with('?') {
        format!("/{}/{}", bucket, r)
    } else {
        format!("/{}/{}", bucket, r)
    }
}

fn sign(cfg: &OssConfig, method: &str, resource: &str, date: &str, content_type: &str) -> String {
    let string_to_sign = format!(
        "{}\n\n{}\n{}\n{}",
        method,
        content_type,
        date,
        canonical_resource(cfg, resource)
    );
    let mut mac = HmacSha1::new_from_slice(cfg.access_key_secret.as_bytes())
        .expect("hmac key");
    mac.update(string_to_sign.as_bytes());
    let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    format!("OSS {}:{}", cfg.access_key_id.trim(), sig)
}

fn normalize_prefix(prefix: &str) -> String {
    let p = prefix.trim().trim_start_matches('/');
    if p.is_empty() {
        return String::new();
    }
    if p.ends_with('/') {
        p.to_string()
    } else {
        format!("{}/", p)
    }
}

fn validate_cfg(cfg: &OssConfig) -> Result<(), String> {
    if cfg.access_key_id.trim().is_empty()
        || cfg.access_key_secret.trim().is_empty()
        || cfg.bucket.trim().is_empty()
    {
        return Err("请先填写完整的 OSS AccessKey / Bucket".into());
    }
    Ok(())
}

pub async fn list_by_prefix(
    cfg: &OssConfig,
    name_ok: fn(&str) -> bool,
) -> Result<Vec<OssBackupItem>, String> {
    validate_cfg(cfg)?;
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let prefix = normalize_prefix(&cfg.prefix);
    let mut items = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut query = format!("list-type=2&prefix={}", urlencoding::encode(&prefix));
        if let Some(m) = &marker {
            query.push_str(&format!("&continuation-token={}", urlencoding::encode(m)));
        }
        let resource = format!("?{}", query);
        let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let auth = sign(cfg, "GET", &resource, &date, "");
        let url = format!("{}{}", base_url(cfg).trim_end_matches('/'), resource);
        let text = client
            .get(&url)
            .header("Date", date)
            .header("Authorization", auth)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        // 极简 XML 解析
        for block in text.split("<Contents>").skip(1) {
            let key = extract_tag(block, "Key");
            let size = extract_tag(block, "Size").parse().unwrap_or(0);
            let lm = extract_tag(block, "LastModified");
            if key.is_empty() {
                continue;
            }
            let name = key.split('/').last().unwrap_or(&key).to_string();
            if !name_ok(&name) {
                continue;
            }
            items.push(OssBackupItem {
                key: key.clone(),
                name,
                size,
                last_modified: lm,
            });
        }
        let truncated = text.contains("<IsTruncated>true</IsTruncated>");
        if !truncated {
            break;
        }
        let next = extract_tag(&text, "NextContinuationToken");
        if next.is_empty() {
            break;
        }
        marker = Some(next);
    }
    items.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    Ok(items)
}

fn extract_tag(xml: &str, tag: &str) -> String {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    if let Some(s) = xml.find(&open) {
        let start = s + open.len();
        if let Some(e) = xml[start..].find(&close) {
            return xml[start..start + e].trim().to_string();
        }
    }
    String::new()
}

pub async fn download_object(
    cfg: &OssConfig,
    object_key: &str,
    local_path: &str,
    on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<u64, String> {
    validate_cfg(cfg)?;
    let key = object_key.trim_start_matches('/');
    let client = Client::new();
    let resource = key;
    let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let auth = sign(cfg, "GET", resource, &date, "");
    let url = format!("{}{}", base_url(cfg), key);
    let resp = client
        .get(&url)
        .header("Date", date)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("OSS 下载失败: {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    if let Some(parent) = Path::new(local_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let n = bytes.len() as u64;
    tokio::fs::write(local_path, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(cb) = &on_progress {
        cb(n, total.max(n));
    }
    Ok(n)
}

pub async fn delete_object(cfg: &OssConfig, object_key: &str) -> Result<(), String> {
    validate_cfg(cfg)?;
    let key = object_key.trim_start_matches('/');
    let client = Client::new();
    let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let auth = sign(cfg, "DELETE", key, &date, "");
    let url = format!("{}{}", base_url(cfg), key);
    let resp = client
        .delete(&url)
        .header("Date", date)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("OSS 删除失败: {}", resp.status()));
    }
    Ok(())
}

pub async fn upload_object(
    cfg: &OssConfig,
    local_path: &str,
    object_key: &str,
    on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<u64, String> {
    validate_cfg(cfg)?;
    let key = object_key.trim_start_matches('/');
    let data = tokio::fs::read(local_path)
        .await
        .map_err(|e| format!("本地文件不存在: {}", e))?;
    let total = data.len() as u64;
    if let Some(cb) = &on_progress {
        cb(0, total);
    }
    let client = Client::new();
    let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let content_type = "application/octet-stream";
    let auth = sign(cfg, "PUT", key, &date, content_type);
    let url = format!("{}{}", base_url(cfg), key);
    let resp = client
        .put(&url)
        .header("Date", date)
        .header("Content-Type", content_type)
        .header("Authorization", auth)
        .body(data)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("OSS 上传失败: {}", resp.status()));
    }
    if let Some(cb) = &on_progress {
        cb(total, total);
    }
    Ok(total)
}

pub async fn cleanup_retain_days(
    cfg: &OssConfig,
    retain_days: u32,
    name_ok: fn(&str) -> bool,
) -> Result<u32, String> {
    if retain_days == 0 {
        return Ok(0);
    }
    let items = list_by_prefix(cfg, name_ok).await?;
    let cutoff = chrono::Utc::now().timestamp_millis() - (retain_days as i64) * 24 * 60 * 60 * 1000;
    let mut removed = 0u32;
    for item in items {
        let t = chrono::DateTime::parse_from_rfc3339(&item.last_modified)
            .map(|d| d.timestamp_millis())
            .unwrap_or(0);
        if t >= cutoff {
            continue;
        }
        delete_object(cfg, &item.key).await?;
        removed += 1;
    }
    Ok(removed)
}

pub fn object_basename(key: &str) -> String {
    Path::new(key)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| key.to_string())
}

pub fn temp_download_path(prefix: &str, file_name: &str) -> PathBuf {
    let dir = std::env::temp_dir();
    dir.join(format!(
        "{}-{}-{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        file_name
    ))
}
