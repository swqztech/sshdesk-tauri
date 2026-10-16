//! 本机 Qdrant 设置与恢复（移植自 qdrantLocalSettings.ts / qdrantLocalRestore.ts）

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use flate2::read::GzDecoder;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::process::Command as StdCommand;
use tokio::process::Command;
use tokio::time::sleep;

pub use crate::local_backups::list_pc_qdrant_backups;
use crate::platform::user_data_dir;
use crate::qdrant::config::QdrantProgress;

pub const QDRANT_INSTALL_DIR_WIN: &str = r"C:\qdrant";
pub const EXPECTED_QDRANT_VERSION_MM: &str = "1.19";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QdrantLocalSettings {
    pub qdrant_exe: String,
    pub qdrant_data_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QdrantBackupMetaPeek {
    pub collection: Option<String>,
    pub timestamp: Option<String>,
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QdrantLocalStatus {
    pub running: bool,
    pub qdrant_url: String,
    pub data_dir: String,
    pub qdrant_exe: Option<String>,
    pub version: Option<String>,
    pub points_count: Option<u64>,
    pub collection: Option<String>,
    pub status: Option<String>,
    pub message: String,
    pub conflict: Option<bool>,
}

pub fn default_qdrant_install_dir() -> PathBuf {
    if cfg!(windows) && Path::new(QDRANT_INSTALL_DIR_WIN).is_dir() {
        return PathBuf::from(QDRANT_INSTALL_DIR_WIN);
    }
    if let Ok(app) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(app).join("qdrant-data");
    }
    PathBuf::from("/var/lib/qdrant")
}

pub fn default_qdrant_exe() -> String {
    let dir = default_qdrant_install_dir();
    let exe = dir.join("qdrant.exe");
    if exe.is_file() {
        return exe.to_string_lossy().to_string();
    }
    if cfg!(windows) {
        let canon = Path::new(QDRANT_INSTALL_DIR_WIN).join("qdrant.exe");
        if canon.is_file() {
            return canon.to_string_lossy().to_string();
        }
    }
    String::new()
}

fn settings_file() -> PathBuf {
    user_data_dir().join("qdrant-local.json")
}

pub fn default_qdrant_local_settings() -> QdrantLocalSettings {
    QdrantLocalSettings {
        qdrant_exe: default_qdrant_exe(),
        qdrant_data_dir: default_qdrant_install_dir().to_string_lossy().to_string(),
    }
}

pub fn load_qdrant_local_settings() -> QdrantLocalSettings {
    let base = default_qdrant_local_settings();
    match fs::read_to_string(settings_file()) {
        Ok(raw) => {
            let parsed: QdrantLocalSettings = serde_json::from_str(&raw).unwrap_or_else(|_| base.clone());
            migrate_settings(parsed, base)
        }
        Err(_) => base,
    }
}

pub fn save_qdrant_local_settings(input: QdrantLocalSettings) -> QdrantLocalSettings {
    let next = migrate_settings(
        QdrantLocalSettings {
            qdrant_exe: input.qdrant_exe,
            qdrant_data_dir: input.qdrant_data_dir,
        },
        default_qdrant_local_settings(),
    );
    let _ = fs::create_dir_all(user_data_dir());
    let _ = fs::write(
        settings_file(),
        serde_json::to_string_pretty(&next).unwrap_or_default(),
    );
    next
}

fn migrate_settings(parsed: QdrantLocalSettings, base: QdrantLocalSettings) -> QdrantLocalSettings {
    let mut data_dir = parsed.qdrant_data_dir.trim().to_string();
    let mut exe = parsed.qdrant_exe.trim().to_string();
    if cfg!(windows) && Path::new(QDRANT_INSTALL_DIR_WIN).is_dir() {
        let canon = Path::new(QDRANT_INSTALL_DIR_WIN).join("qdrant.exe");
        if data_dir.is_empty() || data_dir.to_lowercase().contains("qdrant-data") {
            data_dir = QDRANT_INSTALL_DIR_WIN.to_string();
        }
        if exe.is_empty() || exe.to_lowercase().contains("qdrant-svc") {
            if canon.is_file() {
                exe = canon.to_string_lossy().to_string();
            }
        }
    }
    QdrantLocalSettings {
        qdrant_exe: if exe.is_empty() { base.qdrant_exe } else { exe },
        qdrant_data_dir: if data_dir.is_empty() {
            base.qdrant_data_dir
        } else {
            data_dir
        },
    }
}

pub fn detect_local_qdrant_exe() -> Option<String> {
    let mut candidates = Vec::new();
    let canon = Path::new(QDRANT_INSTALL_DIR_WIN).join("qdrant.exe");
    if canon.is_file() {
        candidates.push(canon);
    }
    let def = default_qdrant_exe();
    if !def.is_empty() {
        candidates.push(PathBuf::from(def));
    }
    if cfg!(windows) {
        candidates.push(PathBuf::from(r"D:\qdrant\qdrant.exe"));
    }
    let mut seen = std::collections::HashSet::new();
    for p in candidates {
        let key = p.to_string_lossy().to_lowercase();
        if seen.insert(key) && p.is_file() {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}

pub fn resolve_local_qdrant_exe(settings: Option<&QdrantLocalSettings>) -> Result<String, String> {
    let saved = settings
        .cloned()
        .unwrap_or_else(load_qdrant_local_settings);
    let configured = saved.qdrant_exe.trim();
    if !configured.is_empty()
        && Path::new(configured).is_file()
        && !configured.to_lowercase().contains("qdrant-svc")
    {
        return Ok(configured.to_string());
    }
    let canon = Path::new(QDRANT_INSTALL_DIR_WIN).join("qdrant.exe");
    if canon.is_file() {
        return Ok(canon.to_string_lossy().to_string());
    }
    if let Some(d) = detect_local_qdrant_exe() {
        return Ok(d);
    }
    Err("找不到 qdrant 可执行文件。请安装 Qdrant 1.19 到 C:\\qdrant\\qdrant.exe，或在设置里手动指定路径。".into())
}

pub fn resolve_local_qdrant_data_dir(settings: Option<&QdrantLocalSettings>) -> String {
    let saved = migrate_settings(
        settings.cloned().unwrap_or_else(load_qdrant_local_settings),
        default_qdrant_local_settings(),
    );
    saved.qdrant_data_dir.trim().to_string()
}

pub fn peek_qdrant_backup_meta(archive_path: &str) -> QdrantBackupMetaPeek {
    let p = archive_path.trim();
    if p.is_empty() || !Path::new(p).is_file() {
        return QdrantBackupMetaPeek {
            collection: None,
            timestamp: None,
            snapshot: None,
        };
    }
    let out = std::process::Command::new("tar")
        .args(["-xOf", p, "backup_meta.txt"])
        .output();
    let text = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return QdrantBackupMetaPeek {
            collection: None,
            timestamp: None,
            snapshot: None,
        },
    };
    let collection = text
        .lines()
        .find_map(|l| l.strip_prefix("collection=").map(|s| s.trim().to_string()));
    let timestamp = text
        .lines()
        .find_map(|l| l.strip_prefix("timestamp=").map(|s| s.trim().to_string()));
    let snapshot = text
        .lines()
        .find_map(|l| l.strip_prefix("snapshot=").map(|s| s.trim().to_string()));
    QdrantBackupMetaPeek {
        collection,
        timestamp,
        snapshot,
    }
}

async fn fetch_server_version(qdrant_url: &str) -> String {
    let client = Client::new();
    let url = format!("{}/", qdrant_url.trim_end_matches('/'));
    match client.get(&url).timeout(Duration::from_secs(5)).send().await {
        Ok(res) if res.status().is_success() => res
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|j| j.get("version").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

async fn wait_health(qdrant_url: &str, max_sec: u32) -> bool {
    let health_url = format!("{}/healthz", qdrant_url.trim_end_matches('/'));
    let client = Client::new();
    for _ in 0..max_sec {
        if client
            .get(&health_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return true;
        }
        sleep(Duration::from_secs(1)).await;
    }
    false
}

fn write_config_yaml(data_dir: &Path) {
    let storage = data_dir.join("storage").to_string_lossy().replace('\\', "/");
    let snapshots = data_dir.join("snapshots").to_string_lossy().replace('\\', "/");
    let yaml = format!(
        "storage:\n  storage_path: {storage}\n  snapshots_path: {snapshots}\nservice:\n  host: 127.0.0.1\n  http_port: 6333\n  grpc_port: 6334\nlog_level: INFO\n"
    );
    let _ = fs::write(data_dir.join("config.yaml"), yaml);
}

async fn stop_local_qdrant_and_wait() {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/IM", "qdrant.exe", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("taskkill")
            .args(["/IM", "qdrant-svc.exe", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    for _ in 0..5 {
        if !wait_health("http://127.0.0.1:6333", 1).await {
            return;
        }
        sleep(Duration::from_millis(400)).await;
    }
}

fn start_qdrant_normal(exe: &str, data_dir: &Path) -> Result<(), String> {
    let config = data_dir.join("config.yaml");
    let mut cmd = Command::new(exe);
    cmd.current_dir(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if config.is_file() {
        cmd.args(["--config-path", "config.yaml"]);
    }
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn probe_local_qdrant_status(collection: Option<&str>) -> QdrantLocalStatus {
    let data_dir = resolve_local_qdrant_data_dir(None);
    let qdrant_url = "http://127.0.0.1:6333".to_string();
    let preferred = collection.unwrap_or("gallery_assets").trim().to_string();
    let client = Client::new();
    match client
        .get(format!("{}/healthz", qdrant_url))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => {
            let version = fetch_server_version(&qdrant_url).await;
            let coll_url = format!(
                "{}/collections/{}",
                qdrant_url,
                urlencoding::encode(&preferred)
            );
            let points = client
                .get(&coll_url)
                .timeout(Duration::from_secs(15))
                .send()
                .await;
            if let Ok(r) = points {
                if r.status().is_success() {
                    let j = r.json::<serde_json::Value>().await.ok();
                    let pc = j
                        .as_ref()
                        .and_then(|v| v.get("result"))
                        .and_then(|r| r.get("points_count").or_else(|| r.get("pointsCount")))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let st = j
                        .as_ref()
                        .and_then(|v| v.get("result"))
                        .and_then(|r| r.get("status"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let message = format!(
                        "本机运行中 · {} {} points · {}",
                        preferred, pc, data_dir
                    );
                    return QdrantLocalStatus {
                        running: true,
                        qdrant_url,
                        data_dir,
                        qdrant_exe: None,
                        version: Some(version),
                        points_count: Some(pc),
                        collection: Some(preferred),
                        status: Some(st),
                        message,
                        conflict: None,
                    };
                }
            }
            let message = format!(
                "本机运行中（未找到集合 {}）· {}",
                preferred, data_dir
            );
            QdrantLocalStatus {
                running: true,
                qdrant_url,
                data_dir,
                qdrant_exe: None,
                version: Some(version),
                points_count: None,
                collection: Some(preferred),
                status: None,
                message,
                conflict: None,
            }
        }
        _ => {
            let message = format!(
                "本机 Qdrant 未运行（数据目录 {}），请点「启动本机 Qdrant」",
                data_dir
            );
            QdrantLocalStatus {
                running: false,
                qdrant_url,
                data_dir,
                qdrant_exe: None,
                version: None,
                points_count: None,
                collection: None,
                status: None,
                message,
                conflict: None,
            }
        }
    }
}

pub async fn start_local_qdrant_service(collection: Option<&str>) -> Result<QdrantLocalStatus, String> {
    let data_dir = resolve_local_qdrant_data_dir(None);
    let qdrant_exe = resolve_local_qdrant_exe(None)?;
    let coll = collection.unwrap_or("gallery_assets").trim().to_string();
    let path = PathBuf::from(&data_dir);
    fs::create_dir_all(path.join("storage")).map_err(|e| e.to_string())?;
    fs::create_dir_all(path.join("snapshots")).map_err(|e| e.to_string())?;
    write_config_yaml(&path);
    stop_local_qdrant_and_wait().await;
    start_qdrant_normal(&qdrant_exe, &path)?;
    sleep(Duration::from_millis(1500)).await;
    let url = "http://127.0.0.1:6333";
    if !wait_health(url, 60).await {
        return Err(format!(
            "本机 Qdrant 启动失败（{} → {}）。请确认端口 6333 未被占用。",
            qdrant_exe, data_dir
        ));
    }
    Ok(probe_local_qdrant_status(Some(&coll)).await)
}

pub async fn stop_local_qdrant_service() -> QdrantLocalStatus {
    let data_dir = resolve_local_qdrant_data_dir(None);
    stop_local_qdrant_and_wait().await;
    let message = format!("已停止本机 Qdrant（6333），数据目录 {}", data_dir);
    QdrantLocalStatus {
        running: false,
        qdrant_url: "http://127.0.0.1:6333".into(),
        data_dir,
        qdrant_exe: None,
        version: None,
        points_count: None,
        collection: None,
        status: None,
        message,
        conflict: None,
    }
}

fn assert_gzip_intact(path: &Path) -> Result<(), String> {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size < 64 {
        return Err("备份文件过小，可能下载不完整，请重新下载或重新备份".into());
    }
    let f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = GzDecoder::new(f);
    let mut buf = [0u8; 64 * 1024];
    while dec.read(&mut buf).map_err(|e| e.to_string())? > 0 {}
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("tar")
        .args([
            "-xzf",
            archive.to_string_lossy().as_ref(),
            "-C",
            dest.to_string_lossy().as_ref(),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(())
}

fn find_snapshot(dir: &Path) -> Option<PathBuf> {
    for entry in walkdir_simple(dir) {
        if entry.extension().map(|e| e == "snapshot").unwrap_or(false) {
            return Some(entry);
        }
    }
    None
}

fn walkdir_simple(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn to_file_url(file_path: &Path) -> String {
    let abs = fs::canonicalize(file_path)
        .unwrap_or_else(|_| file_path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    if abs.starts_with('/') {
        format!("file://{}", abs)
    } else {
        format!("file:///{}", abs)
    }
}

pub async fn restore_qdrant_from_local_archive(
    archive_path: &str,
    collection: Option<&str>,
    qdrant_exe: Option<&str>,
    qdrant_data_dir: Option<&str>,
    on_progress: Option<Box<dyn Fn(QdrantProgress) + Send + Sync>>,
) -> Result<serde_json::Value, String> {
    let archive_path = archive_path.trim();
    if !Path::new(archive_path).is_file() {
        return Err(format!("备份文件不存在：{}", archive_path));
    }
    let qdrant_url = "http://127.0.0.1:6333";
    let data_dir = qdrant_data_dir
        .map(|s| s.to_string())
        .unwrap_or_else(|| resolve_local_qdrant_data_dir(None));
    let peeked = peek_qdrant_backup_meta(archive_path);
    let exe = match qdrant_exe {
        Some(s) => s.to_string(),
        None => resolve_local_qdrant_exe(None)?,
    };
    let emit = |phase: &str, msg: &str, pct: u32| {
        if let Some(cb) = &on_progress {
            cb(QdrantProgress {
                phase: phase.into(),
                message: msg.into(),
                percent: pct,
            });
        }
    };
    emit("prepare", "停止其它 Qdrant 实例…", 3);
    stop_local_qdrant_and_wait().await;
    emit("prepare", "校验备份完整性…", 5);
    assert_gzip_intact(Path::new(archive_path))?;
    let tmp = std::env::temp_dir().join(format!(
        "qdrant-restore-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    emit("pack", "解压 tar.gz…", 15);
    extract_tar_gz(Path::new(archive_path), &tmp)?;
    let snapshot = find_snapshot(&tmp).ok_or("备份里没有 .snapshot 文件")?;
    let fallback = peeked
        .collection
        .as_deref()
        .or(collection)
        .unwrap_or("gallery_assets")
        .trim()
        .to_string();
    let coll = peek_qdrant_backup_meta_from_dir(&tmp).unwrap_or(fallback);
    let data_path = PathBuf::from(&data_dir);
    fs::create_dir_all(data_path.join("storage")).map_err(|e| e.to_string())?;
    fs::create_dir_all(data_path.join("snapshots")).map_err(|e| e.to_string())?;
    write_config_yaml(&data_path);
    let snap_name = snapshot.file_name().unwrap_or_default();
    let snap_dest = data_path.join("snapshots").join(snap_name);
    fs::copy(&snapshot, &snap_dest).map_err(|e| e.to_string())?;
    emit("restore", &format!("恢复集合 {}…", coll), 55);
    stop_local_qdrant_and_wait().await;
    start_qdrant_normal(&exe, &data_path)?;
    sleep(Duration::from_millis(1500)).await;
    if !wait_health(qdrant_url, 90).await {
        return Err("本机 Qdrant 启动失败，无法恢复快照".into());
    }
    let client = Client::new();
    let url = format!(
        "{}/collections/{}/snapshots/recover?wait=true",
        qdrant_url,
        urlencoding::encode(&coll)
    );
    let body = serde_json::json!({
        "location": to_file_url(&snap_dest),
        "priority": "snapshot",
        "wait": true
    });
    let res = client
        .put(&url)
        .json(&body)
        .timeout(Duration::from_secs(3600))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!(
            "API 恢复失败 HTTP {}：{}",
            res.status(),
            res.text().await.unwrap_or_default().chars().take(400).collect::<String>()
        ));
    }
    let info_url = format!(
        "{}/collections/{}",
        qdrant_url,
        urlencoding::encode(&coll)
    );
    let info = client
        .get(&info_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;
    let points = info
        .get("result")
        .and_then(|r| r.get("points_count").or_else(|| r.get("pointsCount")))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let status = info
        .get("result")
        .and_then(|r| r.get("status"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    emit("done", &format!("已恢复 {}（{} points）", coll, points), 100);
    let _ = fs::remove_dir_all(&tmp);
    Ok(serde_json::json!({
        "ok": true,
        "collection": coll,
        "qdrantUrl": qdrant_url,
        "dataDir": data_dir,
        "pointsCount": points,
        "status": status
    }))
}

fn peek_qdrant_backup_meta_from_dir(dir: &Path) -> Option<String> {
    let meta = dir.join("backup_meta.txt");
    if !meta.is_file() {
        return None;
    }
    let text = fs::read_to_string(meta).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("collection=").map(|s| s.trim().to_string()))
}
