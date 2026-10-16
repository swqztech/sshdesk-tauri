//! 远程 Qdrant 备份（移植自 qdrantRemote.ts）

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use serde_json::Value;
use tauri::AppHandle;

use crate::local_backups;
fn extract_json_array(text: &str) -> Option<&str> {
    let start = text.rfind('[')?;
    let end = text.rfind(']')?;
    if end < start {
        return None;
    }
    Some(&text[start..=end])
}

fn parse_list_block(stdout: &str) -> String {
    if let Some(m) = regex::Regex::new(r"__LIST_BEGIN__\r?\n([\s\S]*?)\r?\n__LIST_END__")
        .ok()
        .and_then(|re| re.captures(stdout))
    {
        return m.get(1).map(|x| x.as_str()).unwrap_or("").trim().to_string();
    }
    if let Some(m) = regex::Regex::new(r"__LIST_BEGIN__\r?\n([\s\S]*?)__LIST_END__")
        .ok()
        .and_then(|re| re.captures(stdout))
    {
        return m.get(1).map(|x| x.as_str()).unwrap_or("").trim().to_string();
    }
    stdout.trim().to_string()
}
use crate::oss;
use crate::platform::downloads_dir;
use crate::qdrant::backup_meta::{
    build_qdrant_backup_file_name, format_qdrant_backup_timestamp, is_qdrant_backup_file_name,
};
use crate::qdrant::config::*;
use crate::sessions::SessionManager;
use crate::ssh_remote::{
    download_remote_file_with_progress, exec_command, sh_quote,
};

struct OssPutTools {
    node: String,
    oss_put: String,
}

async fn resolve_oss_put(
    mgr: &SessionManager,
    session_id: &str,
    cfg: &QdrantConfig,
) -> Result<OssPutTools, String> {
    let root = cfg.tuku_backend_root.trim().trim_end_matches('/');
    let puts: Vec<String> = vec![
        "/opt/tuku-backup/oss-put.mjs".into(),
        format!("{}/scripts/backup/oss-put.mjs", root),
        "/www/wwwroot/backend.stwkjyxgs.com/tuku-backend/scripts/backup/oss-put.mjs".into(),
    ];
    let put_loop = puts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| sh_quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    let res = exec_command(
        mgr,
        session_id,
        &format!(
            r#"NODE=$(command -v node || true)
echo "NODE=$NODE"
for p in {put_loop}; do
  if [ -f "$p" ]; then echo "OSS_PUT=$p"; break; fi
done"#,
            put_loop = put_loop,
        ),
        15_000,
    )
    .await?;
    let mut map = HashMap::new();
    for line in res.stdout.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k == "NODE" || k == "OSS_PUT" {
                map.insert(k, v.trim().to_string());
            }
        }
    }
    Ok(OssPutTools {
        node: map.get("NODE").cloned().unwrap_or_default(),
        oss_put: map.get("OSS_PUT").cloned().unwrap_or_default(),
    })
}

fn curl_auth_prefix(api_key: &str) -> String {
    if api_key.is_empty() {
        String::new()
    } else {
        format!("-H {}", sh_quote(&format!("api-key: {}", api_key)))
    }
}

fn merge_cfg(cfg_input: &QdrantConfig) -> QdrantConfig {
    let base = default_qdrant_config();
    apply_env_map(
        &QdrantConfig {
            qdrant_url: if cfg_input.qdrant_url.is_empty() {
                base.qdrant_url
            } else {
                cfg_input.qdrant_url.clone()
            },
            qdrant_api_key: cfg_input.qdrant_api_key.clone(),
            qdrant_collection: if cfg_input.qdrant_collection.is_empty() {
                base.qdrant_collection
            } else {
                cfg_input.qdrant_collection.clone()
            },
            qdrant_backup_dir: if cfg_input.qdrant_backup_dir.is_empty() {
                base.qdrant_backup_dir
            } else {
                cfg_input.qdrant_backup_dir.clone()
            },
            qdrant_backup_retain_days: cfg_input.qdrant_backup_retain_days,
            qdrant_data_dir: if cfg_input.qdrant_data_dir.is_empty() {
                base.qdrant_data_dir
            } else {
                cfg_input.qdrant_data_dir.clone()
            },
            qdrant_config_file: if cfg_input.qdrant_config_file.is_empty() {
                base.qdrant_config_file
            } else {
                cfg_input.qdrant_config_file.clone()
            },
            tuku_backend_root: cfg_input.tuku_backend_root.clone(),
            oss_access_key_id: cfg_input.oss_access_key_id.clone(),
            oss_access_key_secret: cfg_input.oss_access_key_secret.clone(),
            oss_bucket: cfg_input.oss_bucket.clone(),
            oss_region: if cfg_input.oss_region.is_empty() {
                base.oss_region
            } else {
                cfg_input.oss_region.clone()
            },
            oss_endpoint: cfg_input.oss_endpoint.clone(),
            oss_internal: cfg_input.oss_internal,
            oss_backup_root: if cfg_input.oss_backup_root.is_empty() {
                base.oss_backup_root
            } else {
                cfg_input.oss_backup_root.clone()
            },
            oss_qdrant_prefix: cfg_input.oss_qdrant_prefix.clone(),
            oss_qdrant_retain_days: cfg_input.oss_qdrant_retain_days,
            config_path: if cfg_input.config_path.is_empty() {
                base.config_path
            } else {
                cfg_input.config_path.clone()
            },
        },
        &HashMap::new(),
    )
}

fn build_qdrant_oss_env_exports(cfg: &QdrantConfig) -> String {
    let mut lines = vec![
        format!("export OSS_ACCESS_KEY_ID={}", sh_quote(&cfg.oss_access_key_id)),
        format!(
            "export OSS_ACCESS_KEY_SECRET={}",
            sh_quote(&cfg.oss_access_key_secret)
        ),
        format!("export OSS_BUCKET={}", sh_quote(&cfg.oss_bucket)),
        format!("export OSS_REGION={}", sh_quote(&cfg.oss_region)),
        format!("export OSS_ENDPOINT={}", sh_quote(&cfg.oss_endpoint)),
        format!(
            "export OSS_INTERNAL={}",
            sh_quote(if cfg.oss_internal { "true" } else { "false" })
        ),
    ];
    if !cfg.tuku_backend_root.trim().is_empty() {
        lines.push(format!(
            "export TUKU_BACKEND_ROOT={}",
            sh_quote(&cfg.tuku_backend_root)
        ));
    }
    lines.join("\n")
}

pub async fn load_remote_qdrant_config(
    mgr: &SessionManager,
    session_id: &str,
) -> Result<QdrantConfig, String> {
    let mut cfg = default_qdrant_config();
    let script = format!(
        r#"set +e
for CFG in {qdrant} {tuku}; do
  if [ -f "$CFG" ]; then
    echo "__FILE_BEGIN__:$CFG"
    cat "$CFG"
    echo
    echo "__FILE_END__"
  fi
done
ROOT=""
if [ -f {tuku} ]; then
  ROOT=$(grep -E '^TUKU_BACKEND_ROOT=' {tuku} | head -n1 | cut -d= -f2- | tr -d "\"'" || true)
fi
if [ -z "$ROOT" ]; then ROOT={root}; fi
ENVF="$ROOT/.env"
if [ -f "$ENVF" ]; then
  echo "__ENV_BEGIN__"
  grep -E '^(OSS_[A-Z0-9_]+|QDRANT_[A-Z0-9_]+|TUKU_BACKEND_ROOT)=' "$ENVF" || true
  echo "__ENV_END__"
fi"#,
        qdrant = sh_quote(DEFAULT_CONFIG_PATH),
        tuku = sh_quote(TUKU_CONFIG_PATH),
        root = sh_quote(&cfg.tuku_backend_root),
    );
    let read = exec_command(mgr, session_id, &script, 30_000).await?;
    let out = read.stdout;
    let file_re = regex::Regex::new(r"__FILE_BEGIN__:[^\n]*\n([\s\S]*?)\n__FILE_END__").unwrap();
    for m in file_re.captures_iter(&out) {
        cfg = apply_env_map(&cfg, &parse_env_text(m.get(1).map(|x| x.as_str()).unwrap_or("")));
    }
    if let Some(m) = regex::Regex::new(r"__ENV_BEGIN__\n([\s\S]*?)\n__ENV_END__")
        .ok()
        .and_then(|re| re.captures(&out))
    {
        let env = parse_env_text(m.get(1).map(|x| x.as_str()).unwrap_or(""));
        let mut filled = HashMap::new();
        for (ek, empty) in [
            ("QDRANT_URL", cfg.qdrant_url.is_empty()),
            ("QDRANT_API_KEY", cfg.qdrant_api_key.is_empty()),
            ("QDRANT_COLLECTION", false),
            ("OSS_ACCESS_KEY_ID", cfg.oss_access_key_id.is_empty()),
            ("OSS_ACCESS_KEY_SECRET", cfg.oss_access_key_secret.is_empty()),
            ("OSS_BUCKET", cfg.oss_bucket.is_empty()),
            ("OSS_REGION", cfg.oss_region.is_empty()),
            ("OSS_ENDPOINT", cfg.oss_endpoint.is_empty()),
            ("OSS_QDRANT_PREFIX", cfg.oss_qdrant_prefix.is_empty()),
            ("TUKU_BACKEND_ROOT", cfg.tuku_backend_root.is_empty()),
        ] {
            if empty {
                if let Some(v) = env.get(ek) {
                    filled.insert(ek.to_string(), v.clone());
                }
            }
        }
        cfg = apply_env_map(&cfg, &filled);
    }
    cfg.config_path = DEFAULT_CONFIG_PATH.into();
    if cfg.oss_endpoint.is_empty() {
        cfg.oss_endpoint = format!("{}.aliyuncs.com", cfg.oss_region);
    }
    if cfg.oss_qdrant_prefix.is_empty() {
        cfg.oss_qdrant_prefix = format!("{}/qdrant", cfg.oss_backup_root);
    }
    Ok(cfg)
}

pub async fn save_remote_qdrant_config(
    mgr: &SessionManager,
    session_id: &str,
    cfg: QdrantConfig,
) -> Result<QdrantConfig, String> {
    let path = cfg.config_path.trim();
    let path = if path.is_empty() {
        DEFAULT_CONFIG_PATH
    } else {
        path
    };
    let content = serialize_qdrant_config(&QdrantConfig {
        config_path: path.into(),
        ..cfg.clone()
    });
    let b64 = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
    let res = exec_command(
        mgr,
        session_id,
        &format!(
            r#"set -e
PATH_CFG={path}
umask 027
printf '%s' {b64} | base64 -d > "$PATH_CFG"
chmod 640 "$PATH_CFG" || true
echo SAVE_OK"#,
            path = sh_quote(path),
            b64 = sh_quote(&b64),
        ),
        30_000,
    )
    .await?;
    if res.code != 0 || !res.stdout.contains("SAVE_OK") {
        return Err(if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        });
    }
    Ok(QdrantConfig {
        config_path: path.into(),
        ..cfg
    })
}

async fn throw_qdrant_connect_error<T>(
    mgr: &SessionManager,
    session_id: &str,
    raw: &str,
    url: &str,
) -> Result<T, String> {
    let mut msg = format_qdrant_connect_error(raw, url);
    if raw.contains("connection refused") || raw.contains("Failed to connect") {
        let hint = diagnose_qdrant_service(mgr, session_id).await.unwrap_or_default();
        if !hint.is_empty() {
            msg.push_str("\n\n");
            msg.push_str(&hint);
        }
    }
    Err(msg)
}

fn format_qdrant_connect_error(raw: &str, url: &str) -> String {
    let text = raw.trim();
    if text.contains("connection refused") || text.contains("Failed to connect") {
        return format!(
            "无法连接 {}：Qdrant 未启动或未监听该地址。\n请在服务器上启动 Qdrant（systemctl / docker），或在「设置」里修改 QDRANT_URL。",
            url
        );
    }
    if text.contains("Could not resolve host") || text.contains("Name or service not known") {
        return format!("无法解析 {} 的主机名，请检查「设置」里的 Qdrant URL。", url);
    }
    if text.contains("401") || text.contains("403") || text.contains("Unauthorized") {
        return format!("Qdrant 鉴权失败（{}），请检查 API Key。", url);
    }
    if text.is_empty() {
        format!("无法连接 Qdrant（{}），请检查 URL / API Key / 服务是否运行", url)
    } else {
        text.to_string()
    }
}

async fn diagnose_qdrant_service(mgr: &SessionManager, session_id: &str) -> Result<String, String> {
    let res = exec_command(
        mgr,
        session_id,
        r#"set +e
echo "6333 监听:"
(ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null) | grep ':6333' || echo "  无"
echo "systemd:"
for u in qdrant qdrant.service; do
  st=$(systemctl is-active "$u" 2>/dev/null)
  [ -n "$st" ] && echo "  $u: $st"
done
echo "docker:"
docker ps --format '{{.Names}} {{.Ports}}' 2>/dev/null | grep -i qdrant || echo "  无 qdrant 容器"
echo "进程:"
pgrep -a qdrant 2>/dev/null || ps aux 2>/dev/null | grep '[q]drant' || echo "  无 qdrant 进程"
"#,
        15_000,
    )
    .await?;
    Ok(if !res.stderr.is_empty() {
        res.stderr
    } else {
        res.stdout
    }
    .trim()
    .to_string())
}

pub async fn list_qdrant_collections(
    mgr: &SessionManager,
    session_id: &str,
    cfg: &QdrantConfig,
) -> Result<Vec<QdrantCollectionInfo>, String> {
    let cfg = merge_cfg(cfg);
    let url = cfg.qdrant_url.trim_end_matches('/');
    let auth = curl_auth_prefix(&cfg.qdrant_api_key);
    let res = exec_command(
        mgr,
        session_id,
        &format!(
            r#"set -e
BODY=$(curl -fsS {auth} {url})
echo "__JSON_BEGIN__"
echo "$BODY"
echo "__JSON_END__""#,
            auth = auth,
            url = sh_quote(&format!("{}/collections", url)),
        ),
        60_000,
    )
    .await?;
    if res.code != 0 {
        let raw = if !res.stderr.is_empty() {
            res.stderr.clone()
        } else {
            res.stdout.clone()
        };
        return throw_qdrant_connect_error(mgr, session_id, &raw, url).await;
    }
    let block = regex::Regex::new(r"__JSON_BEGIN__\r?\n([\s\S]*?)\r?\n__JSON_END__")
        .ok()
        .and_then(|re| re.captures(&res.stdout))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim())
        .unwrap_or("");
    let raw: Value = serde_json::from_str(block).unwrap_or(Value::Null);
    let names: Vec<String> = raw
        .get("result")
        .and_then(|r| r.get("collections"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut details = Vec::new();
    for name in names {
        let one = exec_command(
            mgr,
            session_id,
            &format!(
                "set -e; curl -fsS {} {}",
                auth,
                sh_quote(&format!("{}/collections/{}", url, urlencoding::encode(&name)))
            ),
            30_000,
        )
        .await;
        let info = match one {
            Ok(o) if o.code == 0 => {
                let j: Value = serde_json::from_str(o.stdout.trim()).unwrap_or(Value::Null);
                QdrantCollectionInfo {
                    name: name.clone(),
                    points_count: j
                        .get("result")
                        .and_then(|r| r.get("points_count").or_else(|| r.get("pointsCount")))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    status: j
                        .get("result")
                        .and_then(|r| r.get("status"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                }
            }
            _ => QdrantCollectionInfo {
                name,
                points_count: 0,
                status: "unknown".into(),
            },
        };
        details.push(info);
    }
    details.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(details)
}

pub async fn list_local_qdrant_backups(
    mgr: &SessionManager,
    session_id: &str,
    cfg: &QdrantConfig,
) -> Result<Vec<LocalBackupItem>, String> {
    let dir = cfg.qdrant_backup_dir.trim();
    let dir = if dir.is_empty() { "/var/backups/qdrant" } else { dir };
    let script = format!(
        r#"set +e
DIR={dir}
if [ ! -d "$DIR" ]; then echo "__LIST_BEGIN__"; echo "[]"; echo "__LIST_END__"; exit 0; fi
echo "__LIST_BEGIN__"
echo -n "["
first=1
shopt -s nullglob
for f in "$DIR"/qdrant_*.tar.gz; do
  [ -f "$f" ] || continue
  name=$(basename -- "$f")
  size=$(stat -c %s -- "$f" 2>/dev/null || stat -f %z -- "$f" 2>/dev/null || echo 0)
  mtime=$(stat -c %Y -- "$f" 2>/dev/null || stat -f %m -- "$f" 2>/dev/null || echo 0)
  mtimeMs=$((mtime * 1000))
  jname=$(printf '%s' "$name" | sed 's/[\\"]/\\&/g')
  jpath=$(printf '%s' "$f" | sed 's/[\\"]/\\&/g')
  if [ "$first" = 1 ]; then first=0; else echo -n ","; fi
  printf '{{"name":"%s","path":"%s","size":%s,"mtimeMs":%s}}' "$jname" "$jpath" "$size" "$mtimeMs"
done
echo "]"
echo "__LIST_END__""#,
        dir = sh_quote(dir),
    );
    let res = exec_command(mgr, session_id, &script, 60_000).await?;
    let block = parse_list_block(&res.stdout);
    let slice = extract_json_array(&block).unwrap_or("[]");
    let list: Vec<LocalBackupItem> = serde_json::from_str(slice).unwrap_or_default();
    Ok(list
        .into_iter()
        .filter(|x| is_qdrant_backup_file_name(&x.name))
        .collect())
}

pub async fn delete_local_qdrant_backup(
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
) -> Result<(), String> {
    let p = remote_path.trim();
    if !p.starts_with('/') || p.contains("..") {
        return Err("非法备份路径".into());
    }
    let name = Path::new(p).file_name().unwrap_or_default().to_string_lossy();
    if !is_qdrant_backup_file_name(&name) {
        return Err("仅允许删除 qdrant_*.tar.gz 备份".into());
    }
    let res = exec_command(
        mgr,
        session_id,
        &format!(
            "set -e; rm -f -- {}; test ! -e {}; echo DEL_OK",
            sh_quote(p),
            sh_quote(p)
        ),
        30_000,
    )
    .await?;
    if res.code != 0 || !res.stdout.contains("DEL_OK") {
        return Err(if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        });
    }
    Ok(())
}

async fn upload_qdrant_via_local_client(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    oss_key: &str,
    cfg: &QdrantConfig,
    on_msg: Option<Box<dyn Fn(String, u32) + Send + Sync>>,
) -> Result<(), String> {
    let tmp = oss::temp_download_path(
        "sshdesk-qdrant",
        Path::new(remote_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .as_ref(),
    );
    if let Some(f) = &on_msg {
        f("经本机中转上传 OSS（服务器无 oss-put）…".into(), 80);
    }
    let _ = download_remote_file_with_progress(
        app,
        mgr,
        session_id,
        remote_path,
        tmp.to_string_lossy().as_ref(),
        None,
    )
    .await?;
    if let Some(f) = &on_msg {
        f("本机上传 OSS…".into(), 90);
    }
    let oss_cfg = to_oss_config(cfg);
    let _ = oss::upload_object(
        &oss_cfg,
        tmp.to_string_lossy().as_ref(),
        oss_key,
        None,
    )
    .await?;
    if cfg.oss_qdrant_retain_days > 0 {
        let _ = oss::cleanup_retain_days(&oss_cfg, cfg.oss_qdrant_retain_days, qdrant_backup_name_ok)
            .await;
    }
    let _ = tokio::fs::remove_file(&tmp).await;
    Ok(())
}

pub async fn backup_remote_qdrant(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    collection: &str,
    cfg_input: &QdrantConfig,
    on_progress: Option<Box<dyn Fn(QdrantProgress) + Send + Sync>>,
) -> Result<Value, String> {
    let coll = collection.trim();
    let cfg = merge_cfg(cfg_input);
    let coll = if coll.is_empty() {
        cfg.qdrant_collection.trim()
    } else {
        coll
    };
    if coll.is_empty() {
        return Err("请选择集合".into());
    }
    let progress: Option<Arc<dyn Fn(QdrantProgress) + Send + Sync>> =
        on_progress.map(Arc::from);
    let emit = |phase: &str, message: &str, percent: u32| {
        if let Some(p) = &progress {
            p(QdrantProgress {
                phase: phase.into(),
                message: message.into(),
                percent,
            });
        }
    };
    let url = cfg.qdrant_url.trim_end_matches('/');
    let auth = curl_auth_prefix(&cfg.qdrant_api_key);
    let dir = cfg.qdrant_backup_dir.trim_end_matches('/');
    let data_dir = cfg.qdrant_data_dir.trim_end_matches('/');
    let config_file = &cfg.qdrant_config_file;
    let ts = format_qdrant_backup_timestamp();
    let file_name = build_qdrant_backup_file_name(coll, Some(&ts))?;
    let work = format!("{}/.work_{}", dir, ts);
    let remote_path = format!("{}/{}", dir, file_name);
    let prefix = cfg.oss_qdrant_prefix.trim_end_matches('/');
    let oss_key = format!("{}/{}", prefix, file_name);

    emit("prepare", "检查 Qdrant 健康状态…", 8);
    let health = exec_command(
        mgr,
        session_id,
        &format!(
            "set -e; curl -fsS {} {} >/dev/null; echo HEALTH_OK",
            auth,
            sh_quote(&format!("{}/healthz", url))
        ),
        30_000,
    )
    .await?;
    if health.code != 0 || !health.stdout.contains("HEALTH_OK") {
        let raw = if !health.stderr.is_empty() {
            health.stderr.clone()
        } else {
            health.stdout.clone()
        };
        return throw_qdrant_connect_error(mgr, session_id, &raw, url).await;
    }

    emit("snapshot", &format!("为集合 {} 创建快照…", coll), 25);
    let snap = exec_command(
        mgr,
        session_id,
        &format!(
            r#"set -e
mkdir -p {work}
SNAP=$(curl -fsS -X POST {auth} {url})
printf '%s\n' "$SNAP" > {json}
SNAP_NAME=$(printf '%s' "$SNAP" | python3 -c 'import json,sys
d=json.load(sys.stdin)
print((d.get("result") or {{}}).get("name") or "")
' 2>/dev/null || true)
if [ -z "$SNAP_NAME" ]; then
  SNAP_NAME=$(printf '%s' "$SNAP" | sed -n 's/.*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)
fi
test -n "$SNAP_NAME"
echo "SNAP_NAME=$SNAP_NAME"
echo SNAP_OK"#,
            work = sh_quote(&work),
            auth = auth,
            url = sh_quote(&format!("{}/collections/{}/snapshots", url, urlencoding::encode(coll))),
            json = sh_quote(&format!("{}/snapshot_create.json", work)),
        ),
        10 * 60 * 1000,
    )
    .await?;
    if snap.code != 0 || !snap.stdout.contains("SNAP_OK") {
        let _ = exec_command(mgr, session_id, &format!("rm -rf {}", sh_quote(&work)), 30_000).await;
        return Err(if !snap.stderr.is_empty() {
            snap.stderr
        } else {
            snap.stdout
        });
    }
    let snap_name = snap
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("SNAP_NAME="))
        .unwrap_or("")
        .trim()
        .to_string();
    if snap_name.is_empty() {
        return Err("无法解析快照文件名".into());
    }

    emit("pack", "打包本次快照（不含整份 storage）…", 55);
    let pack = exec_command(
        mgr,
        session_id,
        &format!(
            r#"set -e
mkdir -p {dir} {snap_dir}
SNAP_NAME={snap_name}
SNAP_SRC=$(find {data_snap} -type f -name "$SNAP_NAME" 2>/dev/null | head -n 1 || true)
if [ -n "$SNAP_SRC" ] && [ -f "$SNAP_SRC" ]; then
  cp -a "$SNAP_SRC" {snap_dest}
else
  curl -fsS -X GET {auth} {dl_url} -o {snap_dest}
fi
test -s {snap_dest}
if [ -f {cfg} ]; then cp -a {cfg} {work_cfg}; fi
cat > {meta} <<EOF
timestamp={ts}
collection={coll}
qdrant_url={url}
hostname=$(hostname)
mode=snapshot
snapshot=$SNAP_NAME
EOF
rm -f {remote}
tar -czf {remote} -C {work} .
rm -rf {work}
test -f {remote}
gzip -t {remote}
tar -tzf {remote} | grep -q 'backup_meta.txt'
tar -tzf {remote} | grep -q '\.snapshot$'
LIST=$(tar -tzf {remote} 2>&1) || true
echo "$LIST" | grep -qi truncated && {{ echo "ARCHIVE_TRUNCATED"; exit 1; }}
echo PACK_OK"#,
            dir = sh_quote(dir),
            snap_dir = sh_quote(&format!("{}/snapshots", work)),
            snap_name = sh_quote(&snap_name),
            data_snap = sh_quote(&format!("{}/snapshots", data_dir)),
            snap_dest = sh_quote(&format!("{}/snapshots/{}", work, snap_name)),
            auth = auth,
            dl_url = sh_quote(&format!(
                "{}/collections/{}/snapshots/{}",
                url,
                urlencoding::encode(coll),
                urlencoding::encode(&snap_name)
            )),
            cfg = sh_quote(config_file),
            work_cfg = sh_quote(&format!("{}/config.yaml", work)),
            meta = sh_quote(&format!("{}/backup_meta.txt", work)),
            ts = ts,
            coll = coll,
            url = url,
            remote = sh_quote(&remote_path),
            work = sh_quote(&work),
        ),
        60 * 60 * 1000,
    )
    .await?;
    if pack.code != 0 || !pack.stdout.contains("PACK_OK") {
        let _ = exec_command(
            mgr,
            session_id,
            &format!("rm -rf {}; rm -f {}", sh_quote(&work), sh_quote(&remote_path)),
            60_000,
        )
        .await;
        return Err(if !pack.stderr.is_empty() {
            pack.stderr
        } else {
            pack.stdout
        });
    }

    let size_res = exec_command(
        mgr,
        session_id,
        &format!(
            "stat -c %s {} 2>/dev/null || stat -f %z {}",
            sh_quote(&remote_path),
            sh_quote(&remote_path)
        ),
        15_000,
    )
    .await?;
    let bytes = size_res.stdout.trim().parse().unwrap_or(0u64);

    if cfg.qdrant_backup_retain_days > 0 {
        let _ = exec_command(
            mgr,
            session_id,
            &format!(
                "find {} -maxdepth 1 -name 'qdrant_*.tar.gz' -mtime +{} -delete || true",
                sh_quote(dir),
                cfg.qdrant_backup_retain_days
            ),
            60_000,
        )
        .await;
    }

    let mut uploaded = false;
    emit("upload", "正在上传到 OSS…", 78);
    if cfg.oss_access_key_id.is_empty()
        || cfg.oss_access_key_secret.is_empty()
        || cfg.oss_bucket.is_empty()
    {
        emit("done", "本地备份已完成（未配置 OSS，跳过上传）", 100);
        return Ok(serde_json::json!({
            "fileName": file_name,
            "remotePath": remote_path,
            "ossKey": oss_key,
            "bytes": bytes,
            "uploaded": false
        }));
    }

    let tools = resolve_oss_put(mgr, session_id, &cfg).await?;
    if !tools.oss_put.is_empty() && !tools.node.is_empty() {
        let env_exports = build_qdrant_oss_env_exports(&cfg);
        emit(
            "upload",
            if cfg.oss_internal {
                "服务器内网直传 OSS…"
            } else {
                "服务器直传 OSS…"
            },
            80,
        );
        let up = exec_command(
            mgr,
            session_id,
            &format!(
                r#"{env}
set -e
{node} {put} put {remote} {key}
echo UPLOAD_OK"#,
                env = env_exports,
                node = sh_quote(&tools.node),
                put = sh_quote(&tools.oss_put),
                remote = sh_quote(&remote_path),
                key = sh_quote(&oss_key),
            ),
            60 * 60 * 1000,
        )
        .await?;
        if up.code != 0 || !up.stdout.contains("UPLOAD_OK") {
            return Err(if !up.stderr.is_empty() {
                up.stderr
            } else {
                up.stdout
            });
        }
        uploaded = true;
        if cfg.oss_qdrant_retain_days > 0 {
            let _ = exec_command(
                mgr,
                session_id,
                &format!(
                    r#"{env}
{node} {put} cleanup {prefix} {days} --name-glob 'qdrant_*.tar.gz' || true"#,
                    env = env_exports,
                    node = sh_quote(&tools.node),
                    put = sh_quote(&tools.oss_put),
                    prefix = sh_quote(prefix),
                    days = sh_quote(&cfg.oss_qdrant_retain_days.to_string()),
                ),
                10 * 60 * 1000,
            )
            .await;
        }
    } else {
        upload_qdrant_via_local_client(
            app,
            mgr,
            session_id,
            &remote_path,
            &oss_key,
            &cfg,
            progress.clone().map(|p| {
                Box::new(move |msg: String, pct: u32| {
                    p(QdrantProgress {
                        phase: "upload".into(),
                        message: msg,
                        percent: pct,
                    });
                }) as Box<dyn Fn(String, u32) + Send + Sync>
            }),
        )
        .await?;
        uploaded = true;
    }

    emit("done", "备份完成并已上传 OSS（服务器本地已保留）", 100);
    Ok(serde_json::json!({
        "fileName": file_name,
        "remotePath": remote_path,
        "ossKey": oss_key,
        "bytes": bytes,
        "uploaded": uploaded
    }))
}

pub async fn upload_server_qdrant_to_oss(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    cfg_input: &QdrantConfig,
    on_progress: Option<Box<dyn Fn(QdrantProgress) + Send + Sync>>,
) -> Result<Value, String> {
    let p = remote_path.trim();
    if !p.starts_with('/') || p.contains("..") {
        return Err("非法备份路径".into());
    }
    let file_name = Path::new(p).file_name().unwrap_or_default().to_string_lossy();
    if !is_qdrant_backup_file_name(&file_name) {
        return Err("不是可上传的 Qdrant 备份文件".into());
    }
    let cfg = merge_cfg(cfg_input);
    if cfg.oss_access_key_id.is_empty()
        || cfg.oss_access_key_secret.is_empty()
        || cfg.oss_bucket.is_empty()
    {
        return Err("请先在设置里填写 OSS".into());
    }
    let progress: Option<Arc<dyn Fn(QdrantProgress) + Send + Sync>> =
        on_progress.map(Arc::from);
    let emit = |phase: &str, message: &str, percent: u32| {
        if let Some(cb) = &progress {
            cb(QdrantProgress {
                phase: phase.into(),
                message: message.into(),
                percent,
            });
        }
    };
    emit("prepare", "检查服务器备份…", 8);
    let check = exec_command(
        mgr,
        session_id,
        &format!(
            "test -f {} && (stat -c %s {} 2>/dev/null || stat -f %z {})",
            sh_quote(p),
            sh_quote(p),
            sh_quote(p)
        ),
        15_000,
    )
    .await?;
    if check.code != 0 {
        return Err("服务器上找不到该备份文件".into());
    }
    let bytes = check.stdout.trim().parse().unwrap_or(0u64);
    let prefix = cfg.oss_qdrant_prefix.trim_end_matches('/');
    let oss_key = format!("{}/{}", prefix, file_name);
    emit("upload", "准备上传 OSS…", 15);
    let via = push_remote_qdrant_to_oss(
        app,
        mgr,
        session_id,
        p,
        &oss_key,
        &cfg,
        progress.clone().map(|p| {
            Box::new(move |msg: String, pct: u32| {
                p(QdrantProgress {
                    phase: "upload".into(),
                    message: msg,
                    percent: pct,
                });
            }) as Box<dyn Fn(String, u32) + Send + Sync>
        }),
    )
    .await?;
    let via_label = match via.as_str() {
        "server-internal" => "服务器内网",
        "server-public" => "服务器公网",
        _ => "本机中转",
    };
    emit("done", &format!("已上传 OSS（{}）", via_label), 100);
    Ok(serde_json::json!({
        "fileName": file_name,
        "remotePath": p,
        "ossKey": oss_key,
        "bytes": bytes,
        "uploaded": true,
        "via": via
    }))
}

async fn push_remote_qdrant_to_oss(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    oss_key: &str,
    cfg: &QdrantConfig,
    on_msg: Option<Box<dyn Fn(String, u32) + Send + Sync>>,
) -> Result<String, String> {
    let tools = resolve_oss_put(mgr, session_id, cfg).await?;
    let prefix = cfg.oss_qdrant_prefix.trim_end_matches('/');
    if !tools.oss_put.is_empty() && !tools.node.is_empty() {
        let via = if cfg.oss_internal {
            "server-internal"
        } else {
            "server-public"
        };
        if let Some(f) = &on_msg {
            f(
                if cfg.oss_internal {
                    "服务器内网直传 OSS…".into()
                } else {
                    "服务器直传 OSS…".into()
                },
                35,
            );
        }
        let env_exports = build_qdrant_oss_env_exports(cfg);
        let up = exec_command(
            mgr,
            session_id,
            &format!(
                r#"{env}
set -e
test -f {remote}
{node} {put} put {remote} {key}
echo UPLOAD_OK"#,
                env = env_exports,
                remote = sh_quote(remote_path),
                node = sh_quote(&tools.node),
                put = sh_quote(&tools.oss_put),
                key = sh_quote(oss_key),
            ),
            60 * 60 * 1000,
        )
        .await?;
        if up.code != 0 || !up.stdout.contains("UPLOAD_OK") {
            return Err(if !up.stderr.is_empty() {
                up.stderr
            } else {
                up.stdout
            });
        }
        if let Some(f) = &on_msg {
            f("上传完成".into(), 92);
        }
        if cfg.oss_qdrant_retain_days > 0 {
            let _ = exec_command(
                mgr,
                session_id,
                &format!(
                    r#"{env}
{node} {put} cleanup {prefix} {days} --name-glob 'qdrant_*.tar.gz' || true"#,
                    env = env_exports,
                    node = sh_quote(&tools.node),
                    put = sh_quote(&tools.oss_put),
                    prefix = sh_quote(prefix),
                    days = sh_quote(&cfg.oss_qdrant_retain_days.to_string()),
                ),
                10 * 60 * 1000,
            )
            .await;
        }
        return Ok(via.into());
    }
    upload_qdrant_via_local_client(app, mgr, session_id, remote_path, oss_key, cfg, on_msg).await?;
    Ok("local-relay".into())
}

pub async fn list_oss_qdrant(cfg: &QdrantConfig) -> Result<Vec<oss::OssBackupItem>, String> {
    oss::list_by_prefix(&to_oss_config(cfg), qdrant_backup_name_ok).await
}

pub async fn download_qdrant_from_oss(
    cfg: &QdrantConfig,
    object_key: &str,
    local_dir: &str,
    on_progress: Option<Box<dyn Fn(QdrantProgress) + Send + Sync>>,
) -> Result<Value, String> {
    tokio::fs::create_dir_all(local_dir)
        .await
        .map_err(|e| e.to_string())?;
    let file_name = Path::new(object_key)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let local_path = local_backups::unique_local_path(
        local_dir,
        &file_name,
        r"(?i)\.tar\.gz$",
    );
    let progress: Option<Arc<dyn Fn(QdrantProgress) + Send + Sync>> =
        on_progress.map(Arc::from);
    if let Some(cb) = &progress {
        cb(QdrantProgress {
            phase: "download".into(),
            message: "正在从 OSS 下载…".into(),
            percent: 5,
        });
    }
    let oss_cfg = to_oss_config(cfg);
    let dl_name = file_name.clone();
    let bytes = oss::download_object(
        &oss_cfg,
        object_key,
        local_path.to_string_lossy().as_ref(),
        progress.clone().map(|p| {
            Box::new(move |t: u64, total: u64| {
                let pct = if total > 0 {
                    5 + ((t * 90) / total.max(1)) as u32
                } else {
                    50
                };
                p(QdrantProgress {
                    phase: "download".into(),
                    message: format!("OSS 下载中 {}", dl_name),
                    percent: pct.min(95),
                });
            }) as Box<dyn Fn(u64, u64) + Send + Sync>
        }),
    )
    .await?;
    if let Some(cb) = &progress {
        cb(QdrantProgress {
            phase: "done".into(),
            message: "下载完成".into(),
            percent: 100,
        });
    }
    Ok(serde_json::json!({
        "fileName": local_path.file_name().unwrap_or_default().to_string_lossy(),
        "localPath": local_path.to_string_lossy(),
        "bytes": bytes
    }))
}

pub async fn remove_oss_qdrant_backup(cfg: &QdrantConfig, object_key: &str) -> Result<(), String> {
    oss::delete_object(&to_oss_config(cfg), object_key).await
}

pub fn default_download_dir() -> String {
    downloads_dir().to_string_lossy().to_string()
}
