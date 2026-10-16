use crate::mongo::backup_meta::{
    resolve_local_backup_chain, LocalBackupRef, MongoBackupMeta,
};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tokio::task;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoLocalConn {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub auth_db: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MongoLocalRestoreProgress {
    pub phase: String,
    pub message: String,
    pub percent: u32,
}

pub fn default_local_mongo_conn() -> MongoLocalConn {
    MongoLocalConn {
        host: "127.0.0.1".into(),
        port: 27017,
        username: Some(String::new()),
        password: Some(String::new()),
        auth_db: Some("admin".into()),
    }
}

pub fn build_local_mongo_uri(conn: Option<&MongoLocalConn>) -> String {
    let c = conn.cloned().unwrap_or_else(default_local_mongo_conn);
    let host = c.host.trim();
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    let port = if c.port > 0 { c.port } else { 27017 };
    let user = c.username.as_deref().unwrap_or("").trim();
    let pass = c.password.as_deref().unwrap_or("");
    let auth_db = c.auth_db.as_deref().unwrap_or("admin").trim();
    if !user.is_empty() {
        format!(
            "mongodb://{}:{}@{}:{}/?authSource={}",
            urlencoding::encode(user),
            urlencoding::encode(pass),
            host,
            port,
            urlencoding::encode(auth_db)
        )
    } else {
        format!("mongodb://{}:{}", host, port)
    }
}

pub fn mask_mongo_uri(uri: &str) -> String {
    regex::Regex::new(r"//([^:@/]+):([^@/]+)@")
        .ok()
        .map(|re| re.replace(uri, "//$1:***@").to_string())
        .unwrap_or_else(|| uri.to_string())
}

fn path_if_exists(p: &str) -> Option<String> {
    if Path::new(p).is_file() {
        Some(p.to_string())
    } else {
        None
    }
}

pub async fn resolve_local_mongorestore() -> Result<String, String> {
    let exe_name = if cfg!(windows) {
        "mongorestore.exe"
    } else {
        "mongorestore"
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("MONGO_TOOLS_DIR") {
        candidates.push(PathBuf::from(dir).join(exe_name));
    }
    candidates.push(PathBuf::from("tools/bin").join(exe_name));
    candidates.push(PathBuf::from("tools").join(exe_name));
    if let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
        candidates.push(
            home.join("Downloads")
                .join("mongodb-database-tools-windows-x86_64-100.16.0")
                .join("bin")
                .join(exe_name),
        );
    }
    if cfg!(windows) {
        candidates.push(PathBuf::from(r"C:\Program Files\MongoDB\Tools\100\bin\mongorestore.exe"));
        candidates.push(PathBuf::from(r"C:\mongodb\bin\mongorestore.exe"));
    } else {
        candidates.push(PathBuf::from("/usr/bin/mongorestore"));
        candidates.push(PathBuf::from("/usr/local/bin/mongorestore"));
    }
    for item in candidates {
        if let Some(hit) = path_if_exists(&item.to_string_lossy()) {
            return Ok(hit);
        }
    }
    let which_cmd = if cfg!(windows) { "where" } else { "which" };
    if let Ok(out) = Command::new(which_cmd)
        .arg("mongorestore")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let first = stdout
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty());
        if let Some(p) = first {
            return Ok(p.to_string());
        }
    }
    Err("本机未找到 mongorestore。请安装 MongoDB Database Tools，或设置环境变量 MONGO_TOOLS_DIR 指向其 bin 目录。".into())
}

fn run_tool(bin: &str, args: &[String]) -> Result<(), String> {
    let out = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).to_string()
        } else {
            err.to_string()
        };
        return Err(if msg.is_empty() {
            format!("mongorestore 失败: {}", bin)
        } else {
            msg
        });
    }
    Ok(())
}

fn read_magic(path: &str) -> (bool, bool) {
    let mut head = [0u8; 4];
    if let Ok(mut f) = File::open(path) {
        if f.read(&mut head).unwrap_or(0) >= 2 {
            let is_zip = head[0] == 0x50 && head[1] == 0x4b;
            let is_gzip = head[0] == 0x1f && head[1] == 0x8b;
            return (is_zip, is_gzip);
        }
    }
    (false, false)
}

fn unzip_to_directory(zip_path: &str, out_dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    if cfg!(windows) {
        let ps = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            zip_path.replace('\'', "''"),
            out_dir.replace('\'', "''")
        );
        let out = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &ps])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).to_string());
        }
        return Ok(());
    }
    let out = Command::new("unzip")
        .args(["-q", zip_path, "-d", out_dir])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(())
}

fn find_dump_root(extract_dir: &str) -> String {
    fn walk(dir: &Path) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut has_bson = false;
        let mut subdirs = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_file() && name.ends_with(".bson") {
                has_bson = true;
            }
            if e.path().is_dir() {
                subdirs.push(e.path());
            }
        }
        if has_bson {
            return Some(dir.to_path_buf());
        }
        for child in &subdirs {
            if std::fs::read_dir(child)
                .ok()
                .map(|rd| {
                    rd.flatten()
                        .any(|x| x.file_name().to_string_lossy().ends_with(".bson"))
                })
                .unwrap_or(false)
            {
                return Some(dir.to_path_buf());
            }
        }
        for child in subdirs {
            if let Some(found) = walk(&child) {
                return Some(found);
            }
        }
        None
    }
    walk(Path::new(extract_dir))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| extract_dir.to_string())
}

pub async fn restore_local_database_from_file(
    local_path: &str,
    target_db: &str,
    mode: &str,
    conn: Option<&MongoLocalConn>,
    source_db_hint: Option<&str>,
    on_progress: Option<Box<dyn Fn(MongoLocalRestoreProgress) + Send + Sync>>,
) -> Result<serde_json::Value, String> {
    let local_path = local_path.trim();
    let target_db = target_db.trim();
    if target_db.is_empty() {
        return Err("请指定目标数据库名".into());
    }
    if local_path.is_empty() || !Path::new(local_path).is_file() {
        return Err(format!("备份文件不存在：{}", local_path));
    }
    let uri = build_local_mongo_uri(conn);
    let connection_label = mask_mongo_uri(&uri);
    let drop_args: Vec<String> = if mode == "replace" {
        vec!["--drop".into()]
    } else {
        vec![]
    };
    let hint = source_db_hint.unwrap_or("").trim();

    let emit = |phase: &str, message: &str, percent: u32| {
        if let Some(cb) = &on_progress {
            cb(MongoLocalRestoreProgress {
                phase: phase.to_string(),
                message: message.to_string(),
                percent,
            });
        }
    };

    emit("prepare", "探测本机 mongorestore…", 8);
    let mongorestore = resolve_local_mongorestore().await?;
    emit("prepare", &format!("准备恢复到本机 {}", connection_label), 18);

    let work = std::env::temp_dir().join(format!(
        "sshdesk-mongo-restore-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    emit("restore", &format!("正在恢复到本机库 {}…", target_db), 40);

    let result = task::spawn_blocking({
        let work = work.clone();
        let local_path = local_path.to_string();
        let target_db = target_db.to_string();
        let uri = uri.clone();
        let drop_args = drop_args.clone();
        let hint = hint.to_string();
        let mongorestore = mongorestore.clone();
        move || -> Result<(String, String), String> {
            let (is_zip, is_gzip) = read_magic(&local_path);

            let restore_zip = |extract_dir: &str| -> Result<(), String> {
                unzip_to_directory(&local_path, extract_dir)?;
                let dump_root = find_dump_root(extract_dir);
                let entries = std::fs::read_dir(&dump_root).map_err(|e| e.to_string())?;
                let db_dirs: Vec<String> = entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                let source_db = if !hint.is_empty() {
                    hint.clone()
                } else {
                    db_dirs.first().cloned().unwrap_or_else(|| target_db.clone())
                };
                if !db_dirs.is_empty() {
                    let mut args = vec![
                        "--uri".into(),
                        uri.clone(),
                    ];
                    args.extend(drop_args.clone());
                    args.push("--dir".into());
                    args.push(dump_root);
                    args.push("--nsFrom".into());
                    args.push(format!("{}.*", source_db));
                    args.push("--nsTo".into());
                    args.push(format!("{}.*", target_db));
                    run_tool(&mongorestore, &args)?;
                } else {
                    let mut args = vec!["--uri".into(), uri.clone()];
                    args.extend(drop_args.clone());
                    args.push("--db".into());
                    args.push(target_db.clone());
                    args.push("--dir".into());
                    args.push(dump_root);
                    run_tool(&mongorestore, &args)?;
                }
                Ok(())
            };

            if is_zip {
                let extract = work.join("extract");
                restore_zip(&extract.to_string_lossy())?;
                return Ok(("bson-zip".into(), connection_label.clone()));
            }
            if is_gzip {
                let mut args = vec!["--uri".into(), uri.clone()];
                args.extend(drop_args.clone());
                args.push("--gzip".into());
                args.push(format!("--archive={}", local_path));
                let source_db = if !hint.is_empty() {
                    hint.clone()
                } else {
                    target_db.clone()
                };
                if source_db != target_db {
                    args.push("--nsFrom".into());
                    args.push(format!("{}.*", source_db));
                    args.push("--nsTo".into());
                    args.push(format!("{}.*", target_db));
                }
                run_tool(&mongorestore, &args)?;
                return Ok(("archive-gzip".into(), connection_label.clone()));
            }

            let extract = work.join("extract-raw");
            if restore_zip(&extract.to_string_lossy()).is_ok() {
                return Ok(("bson-zip".into(), connection_label.clone()));
            }
            let mut args = vec!["--uri".into(), uri.clone()];
            args.extend(drop_args.clone());
            args.push("--gzip".into());
            args.push(format!("--archive={}", local_path));
            let source_db = if !hint.is_empty() {
                hint.clone()
            } else {
                target_db.clone()
            };
            if source_db != target_db {
                args.push("--nsFrom".into());
                args.push(format!("{}.*", source_db));
                args.push("--nsTo".into());
                args.push(format!("{}.*", target_db));
            }
            run_tool(&mongorestore, &args)?;
            Ok(("archive-gzip".into(), connection_label.clone()))
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    let _ = std::fs::remove_dir_all(&work);
    emit("done", &format!("已恢复到本机 {}", target_db), 100);
    Ok(serde_json::json!({
        "ok": true,
        "db": target_db,
        "mode": mode,
        "format": result.0,
        "connectionLabel": result.1
    }))
}

pub async fn restore_local_backup_chain(
    chain_paths: Vec<String>,
    target_db: &str,
    mode: &str,
    conn: Option<&MongoLocalConn>,
    source_db_hint: Option<&str>,
    on_progress: Option<Box<dyn Fn(MongoLocalRestoreProgress) + Send + Sync>>,
) -> Result<serde_json::Value, String> {
    let chain_paths: Vec<String> = chain_paths.into_iter().filter(|p| !p.is_empty()).collect();
    if chain_paths.is_empty() {
        return Err("恢复链为空".into());
    }
    let target_db = target_db.trim();
    if target_db.is_empty() {
        return Err("请指定目标数据库名".into());
    }
    let mut last: Option<serde_json::Value> = None;
    let n = chain_paths.len();
    for (i, path) in chain_paths.iter().enumerate() {
        let is_first = i == 0;
        let step_mode = if is_first { mode } else { "merge" };
        let pct_base = 5 + ((i as u32 * 90) / n as u32);
        if let Some(cb) = &on_progress {
            cb(MongoLocalRestoreProgress {
                phase: "restore".into(),
                message: format!(
                    "链式恢复 {}/{}：{}",
                    i + 1,
                    n,
                    Path::new(path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
                percent: pct_base,
            });
        }
        last = Some(
            restore_local_database_from_file(
                path,
                target_db,
                step_mode,
                conn,
                source_db_hint,
                None,
            )
            .await?,
        );
    }
    if let Some(cb) = &on_progress {
        cb(MongoLocalRestoreProgress {
            phase: "done".into(),
            message: format!("链式恢复完成（{} 步）", n),
            percent: 100,
        });
    }
    let mut v = last.unwrap_or(serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("steps".into(), serde_json::json!(n));
    }
    Ok(v)
}

pub async fn restore_local_backup_auto_chain(
    target_path: &str,
    siblings: &[LocalBackupRef],
    target_db: &str,
    mode: &str,
    conn: Option<&MongoLocalConn>,
    source_db_hint: Option<&str>,
    on_progress: Option<Box<dyn Fn(MongoLocalRestoreProgress) + Send + Sync>>,
) -> Result<serde_json::Value, String> {
    let (chain, meta) = resolve_local_backup_chain(target_path, siblings).await?;
    if chain.len() == 1 && meta.backup_type == "full" {
        return restore_local_database_from_file(
            target_path,
            target_db,
            mode,
            conn,
            source_db_hint,
            on_progress,
        )
        .await;
    }
    restore_local_backup_chain(chain, target_db, mode, conn, source_db_hint, on_progress).await
}

pub fn peek_local_backup_size(local_path: &str) -> u64 {
    std::fs::metadata(local_path)
        .map(|m| m.len())
        .unwrap_or(0)
}
