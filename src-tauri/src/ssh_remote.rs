use crate::events;
use crate::sessions::{Connection, SessionManager};
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use std::path::Path;
use tokio::io::AsyncReadExt;
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::AsyncWriteExt;

pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub struct ExecResult {
    pub code: u32,
    pub stdout: String,
    pub stderr: String,
}

pub async fn exec_command(
    mgr: &SessionManager,
    session_id: &str,
    command: &str,
    timeout_ms: u64,
) -> Result<ExecResult, String> {
    let conn = mgr.connection_for_exec(session_id)?;
    let _guard = conn.exec_lock.lock().await;
    if conn.handle.lock().is_closed() {
        mgr.revive_connection(&conn).await?;
    }
    match exec_on_connection(&conn, command, timeout_ms).await {
        Ok(result) => Ok(result),
        Err(err) if SessionManager::transport_dead(&err) => {
            mgr.revive_connection(&conn).await?;
            exec_on_connection(&conn, command, timeout_ms)
                .await
                .map_err(|e| {
                    if SessionManager::transport_dead(&e) {
                        "SSH 重连后仍然打不开命令通道。这台机器的网络或 sshd 不稳定，请稍后再进首页".into()
                    } else {
                        e
                    }
                })
        }
        Err(err) => Err(err),
    }
}

async fn exec_on_connection(
    conn: &crate::sessions::Connection,
    command: &str,
    timeout_ms: u64,
) -> Result<ExecResult, String> {
    let timeout = Duration::from_millis(timeout_ms.max(1000));
    let wrapped = format!("bash -lc {}", sh_quote(command));
    let handle = conn.handle.lock().clone();
    if handle.is_closed() {
        return Err("Channel send error".into());
    }

    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("exec 通道失败: {}", e))?;
    if let Err(err) = channel.exec(true, wrapped).await {
        let _ = channel.close().await;
        return Err(err.to_string());
    }

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut result = Err(format!("远程命令超时（{}s）", timeout.as_secs()));

    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        let msg = channel.wait().await;
        match msg {
            Some(ChannelMsg::Data { data }) => stdout.extend_from_slice(&data),
            Some(ChannelMsg::ExtendedData { data, ext: 1 }) => stderr.extend_from_slice(&data),
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                result = Ok(ExecResult {
                    code: exit_status,
                    stdout: String::from_utf8_lossy(&stdout).to_string(),
                    stderr: String::from_utf8_lossy(&stderr).to_string(),
                });
                break;
            }
            Some(ChannelMsg::Close) | None => {
                result = Ok(ExecResult {
                    code: 0,
                    stdout: String::from_utf8_lossy(&stdout).to_string(),
                    stderr: String::from_utf8_lossy(&stderr).to_string(),
                });
                break;
            }
            _ => {}
        }
    }
    // 超时或结束后必须关掉通道。否则不稳的机器上会话会堆满，后面全部变成 Channel send error。
    let _ = channel.eof().await;
    let _ = channel.close().await;
    result
}

fn normalize_remote_path(input: &str) -> String {
    let raw = input.trim();
    if raw.is_empty() || raw == "/" {
        return "/".to_string();
    }
    if raw == "~" {
        return "~".to_string();
    }
    let p = raw.replace('\\', "/");
    if p == "." {
        return "/".to_string();
    }
    p
}

fn join_remote(base: &str, name: &str) -> String {
    if name == ".." {
        if base == "/" || base == "~" {
            return base.to_string();
        }
        let parent = Path::new(base).parent().map(|p| p.to_string_lossy().to_string());
        return parent.unwrap_or_else(|| "/".to_string());
    }
    if base == "~" {
        return format!("~/{}", name);
    }
    if base.ends_with('/') {
        return format!("{}{}", base, name);
    }
    format!("{}/{}", base, name)
}

async fn open_sftp(conn: &Connection) -> Result<SftpSession, String> {
    let handle = conn.handle.lock().clone();
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| e.to_string())?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| e.to_string())?;
    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| e.to_string())
}

pub async fn resolve_remote_path(
    mgr: &SessionManager,
    session_id: &str,
    path: &str,
) -> Result<String, String> {
    let p = path.trim();
    let p = if p.is_empty() { "/" } else { p };
    let res = exec_command(
        mgr,
        session_id,
        &format!("cd {} && pwd", sh_quote(p)),
        15_000,
    )
    .await?;
    if res.code != 0 {
        let msg = if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        };
        return Err(if msg.is_empty() {
            format!("无法进入目录：{}", p)
        } else {
            msg
        });
    }
    let resolved = res
        .stdout
        .lines()
        .last()
        .map(|l| l.trim().replace('\\', "/"))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("无法解析路径：{}", p))?;
    Ok(resolved)
}

pub async fn get_remote_home(mgr: &SessionManager, session_id: &str) -> Result<String, String> {
    let res = exec_command(mgr, session_id, "cd ~ && pwd", 10_000).await?;
    if res.code != 0 {
        return Err("无法获取家目录".into());
    }
    let home = res.stdout.lines().last().unwrap_or("/").trim();
    Ok(normalize_remote_path(home))
}

pub async fn list_remote_dir(
    mgr: &SessionManager,
    session_id: &str,
    dir_path: &str,
) -> Result<serde_json::Value, String> {
    let conn = mgr.connection_for_exec(session_id)?;
    let resolved = resolve_remote_path(mgr, session_id, dir_path).await?;
    let sftp = open_sftp(&conn).await?;
    let list = sftp.read_dir(&resolved).await.map_err(|e| e.to_string())?;

    let mut entries: Vec<serde_json::Value> = Vec::new();
    for item in list {
        let name = item.file_name().to_string();
        if name.is_empty() || name == "." {
            continue;
        }
        let path = join_remote(&resolved, &name);
        let meta = item.metadata();
        let is_dir = meta.is_dir();
        let size = meta.size.unwrap_or(0);
        let mtime_ms = meta.mtime.unwrap_or(0) as u64 * 1000;
        entries.push(serde_json::json!({
            "name": name,
            "path": path,
            "isDirectory": is_dir,
            "size": size,
            "mtimeMs": mtime_ms
        }));
    }
    entries.sort_by(|a, b| {
        let ad = a.get("isDirectory").and_then(|v| v.as_bool()).unwrap_or(false);
        let bd = b.get("isDirectory").and_then(|v| v.as_bool()).unwrap_or(false);
        match (ad, bd) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .cmp(
                    &b.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase(),
                ),
        }
    });
    let parent = if resolved == "/" {
        None
    } else {
        Some(
            Path::new(&resolved)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string()),
        )
    };
    Ok(serde_json::json!({
        "path": resolved,
        "parent": parent,
        "entries": entries
    }))
}

pub async fn stat_remote_dir_size(
    mgr: &SessionManager,
    session_id: &str,
    dir_path: &str,
) -> Result<serde_json::Value, String> {
    let resolved = resolve_remote_path(mgr, session_id, dir_path).await?;
    let res = exec_command(
        mgr,
        session_id,
        &format!("du -sb {} 2>/dev/null | awk '{{print $1}}'", sh_quote(&resolved)),
        5 * 60 * 1000,
    )
    .await?;
    if res.code != 0 {
        let msg = if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        };
        return Err(if msg.is_empty() {
            format!("无法统计目录：{}", resolved)
        } else {
            msg
        });
    }
    let raw = res.stdout.lines().last().unwrap_or("0").trim();
    let bytes: u64 = raw.parse().unwrap_or(0);
    let human_res = exec_command(
        mgr,
        session_id,
        &format!("du -sh {} 2>/dev/null | awk '{{print $1}}'", sh_quote(&resolved)),
        60_000,
    )
    .await?;
    let human = human_res
        .stdout
        .lines()
        .last()
        .unwrap_or(&format!("{} B", bytes))
        .trim()
        .to_string();
    Ok(serde_json::json!({
        "path": resolved,
        "bytes": bytes,
        "human": human
    }))
}

async fn transfer_remote_paths(
    mgr: &SessionManager,
    session_id: &str,
    sources: &[String],
    dest_dir: &str,
    mode: &str,
) -> Result<serde_json::Value, String> {
    if sources.is_empty() {
        return Err(if mode == "copy" {
            "未选择要复制的项目"
        } else {
            "未选择要移动的项目"
        }
        .into());
    }
    let dest = resolve_remote_path(mgr, session_id, dest_dir).await?;
    let mut count = 0u64;
    for src in sources {
        let resolved = resolve_remote_path(mgr, session_id, src).await?;
        if mode == "move" {
            if resolved == dest || dest.starts_with(&format!("{}/", resolved)) {
                return Err(format!("不能将 {} 移动到其子目录", resolved));
            }
            let parent = Path::new(&resolved)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            if parent == dest {
                continue;
            }
        }
        let cmd = if mode == "copy" {
            format!("cp -a {} {}/", sh_quote(&resolved), sh_quote(&dest))
        } else {
            format!("mv {} {}/", sh_quote(&resolved), sh_quote(&dest))
        };
        let res = exec_command(mgr, session_id, &cmd, 30 * 60 * 1000).await?;
        if res.code != 0 {
            let verb = if mode == "copy" { "复制" } else { "移动" };
            let msg = if !res.stderr.is_empty() {
                res.stderr
            } else {
                res.stdout
            };
            return Err(if msg.is_empty() {
                format!("{}失败：{}", verb, resolved)
            } else {
                msg
            });
        }
        count += 1;
    }
    Ok(serde_json::json!({ "count": count }))
}

pub async fn copy_remote_paths(
    mgr: &SessionManager,
    session_id: &str,
    sources: Vec<String>,
    dest_dir: &str,
) -> Result<serde_json::Value, String> {
    transfer_remote_paths(mgr, session_id, &sources, dest_dir, "copy").await
}

pub async fn move_remote_paths(
    mgr: &SessionManager,
    session_id: &str,
    sources: Vec<String>,
    dest_dir: &str,
) -> Result<serde_json::Value, String> {
    transfer_remote_paths(mgr, session_id, &sources, dest_dir, "move").await
}

pub async fn download_remote_file(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
) -> Result<serde_json::Value, String> {
    let target = local_path.trim();
    if target.is_empty() {
        return Err("缺少本机保存路径（请在前端选择保存位置）".into());
    }
    let conn = mgr.connection_for_exec(session_id)?;
    let remote = remote_path.to_string();
    let sftp = open_sftp(&conn).await?;
    let meta = sftp.metadata(&remote).await.map_err(|e| e.to_string())?;
    let total = meta.size.unwrap_or(0);

    if let Some(parent) = Path::new(target).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }

    events::emit(
        app,
        "files:downloadProgress",
        serde_json::json!({
            "sessionId": session_id,
            "remotePath": remote,
            "localPath": target,
            "transferred": 0,
            "total": total
        }),
    );

    let mut remote_file = sftp.open(&remote).await.map_err(|e| e.to_string())?;
    let mut local_file = tokio::fs::File::create(target)
        .await
        .map_err(|e| e.to_string())?;
    let mut transferred = 0u64;
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = remote_file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        local_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| e.to_string())?;
        transferred += n as u64;
        events::emit(
            app,
            "files:downloadProgress",
            serde_json::json!({
                "sessionId": session_id,
                "remotePath": remote,
                "localPath": target,
                "transferred": transferred,
                "total": total
            }),
        );
    }

    events::emit(
        app,
        "files:downloadProgress",
        serde_json::json!({
            "sessionId": session_id,
            "remotePath": remote,
            "localPath": target,
            "transferred": total,
            "total": total
        }),
    );

    Ok(serde_json::json!({
        "ok": true,
        "localPath": target,
        "bytes": total
    }))
}

/// 带进度回调的下载（Mongo/Qdrant 中转 OSS 等场景）
pub async fn download_remote_file_with_progress(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
    on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<u64, String> {
    let target = local_path.trim();
    if target.is_empty() {
        return Err("缺少本机保存路径".into());
    }
    let conn = mgr.connection_for_exec(session_id)?;
    let remote = remote_path.to_string();
    let sftp = open_sftp(&conn).await?;
    let meta = sftp.metadata(&remote).await.map_err(|e| e.to_string())?;
    let total = meta.size.unwrap_or(0);

    if let Some(parent) = Path::new(target).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }

    let emit_progress = |transferred: u64| {
        if let Some(cb) = &on_progress {
            cb(transferred, total);
        }
        events::emit(
            app,
            "files:downloadProgress",
            serde_json::json!({
                "sessionId": session_id,
                "remotePath": remote,
                "localPath": target,
                "transferred": transferred,
                "total": total
            }),
        );
    };

    emit_progress(0);

    let mut remote_file = sftp.open(&remote).await.map_err(|e| e.to_string())?;
    let mut local_file = tokio::fs::File::create(target)
        .await
        .map_err(|e| e.to_string())?;
    let mut transferred = 0u64;
    let mut buf = vec![0u8; 128 * 1024];
    let mut last_emit = std::time::Instant::now();
    loop {
        let n = remote_file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        local_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| e.to_string())?;
        transferred += n as u64;
        if last_emit.elapsed().as_millis() > 80 || transferred >= total {
            last_emit = std::time::Instant::now();
            emit_progress(transferred);
        }
    }
    emit_progress(total);
    Ok(total)
}

pub async fn upload_remote_file(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
    on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<u64, String> {
    let local = local_path.trim();
    if local.is_empty() {
        return Err("缺少本机文件路径".into());
    }
    let meta = tokio::fs::metadata(local)
        .await
        .map_err(|e| format!("本地文件不存在: {}", e))?;
    let total = meta.len();

    let conn = mgr.connection_for_exec(session_id)?;
    let remote = remote_path.to_string();
    let sftp = open_sftp(&conn).await?;

    let emit_progress = |transferred: u64| {
        if let Some(cb) = &on_progress {
            cb(transferred, total);
        }
        events::emit(
            app,
            "files:uploadProgress",
            serde_json::json!({
                "sessionId": session_id,
                "localPath": local,
                "remotePath": remote,
                "transferred": transferred,
                "total": total
            }),
        );
    };

    emit_progress(0);

    let mut local_file = tokio::fs::File::open(local)
        .await
        .map_err(|e| e.to_string())?;
    let mut remote_file = sftp
        .create(&remote)
        .await
        .map_err(|e| e.to_string())?;

    let mut transferred = 0u64;
    let mut buf = vec![0u8; 128 * 1024];
    let mut last_emit = std::time::Instant::now();
    loop {
        let n = local_file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        remote_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| e.to_string())?;
        transferred += n as u64;
        if last_emit.elapsed().as_millis() > 80 || transferred >= total {
            last_emit = std::time::Instant::now();
            emit_progress(transferred);
        }
    }
    emit_progress(total);
    Ok(total)
}
