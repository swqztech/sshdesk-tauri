use crate::mongo::backup_meta::{
    bt_mc_zip_name, chain_file_name, is_mongo_backup_file_name, latest_chain_entry,
    parse_backup_chain, parse_backup_id_from_file_name, serialize_backup_chain, BACKUP_META_NAME,
    BackupMetaCollection, MongoBackupChain, MongoBackupMeta,
};
use crate::mongo::config::{
    apply_env_map, default_backup_config, parse_env_text, serialize_backup_config, to_oss_config,
    BackupConfig, BackupProgress, LocalBackupItem, MongoConn, MongoDbInfo, DEFAULT_CONFIG_PATH,
};
use crate::oss::{self, OssBackupItem};
use crate::sessions::SessionManager;
use crate::ssh_remote::{
    exec_command, sh_quote, upload_remote_file, download_remote_file_with_progress,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tauri::AppHandle;

/// 解析远程 shell 输出的 `__LIST_BEGIN__` … `__LIST_END__` 块（与 Node 版一致）
pub fn parse_list_block(text: &str) -> String {
    if let Some(m) = regex::Regex::new(r"__LIST_BEGIN__\r?\n([\s\S]*?)\r?\n__LIST_END__")
        .ok()
        .and_then(|re| re.captures(text))
    {
        return m.get(1).map(|c| c.as_str()).unwrap_or("").trim().to_string();
    }
    regex::Regex::new(r"__LIST_BEGIN__\r?\n([\s\S]*?)__LIST_END__")
        .ok()
        .and_then(|re| re.captures(text))
        .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .unwrap_or_else(|| text.trim().to_string())
}

/// 从文本中提取最后一个 JSON 数组片段
pub fn extract_json_array(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let start = trimmed.rfind('[')?;
    let end = trimmed.rfind(']')?;
    if end < start {
        return None;
    }
    Some(&trimmed[start..=end])
}

const COLLECTION_STATS_JS: &str = r"
function collectStats(dbname) {
  const out = {};
  const names = db.getSiblingDB(dbname).getCollectionNames();
  for (let i = 0; i < names.length; i++) {
    const name = names[i];
    if (name.indexOf('system.') === 0) continue;
    const col = db.getSiblingDB(dbname).getCollection(name);
    const count = col.estimatedDocumentCount();
    let maxId = null;
    if (count > 0) {
      const doc = col.find().sort({ _id: -1 }).limit(1).next();
      if (doc && doc._id) {
        maxId = doc._id.toHexString ? doc._id.toHexString() : String(doc._id);
      }
    }
    let updateField = null;
    const sample = col.findOne();
    if (sample) {
      const fields = ['updatedAt', 'updated_at', 'modifiedAt', 'modifyTime', 'lastModified', 'updateTime'];
      for (let j = 0; j < fields.length; j++) {
        const f = fields[j];
        if (sample[f] instanceof Date) { updateField = f; break; }
      }
    }
    out[name] = { count: count, maxId: maxId, updateField: updateField };
  }
  return out;
}
";

struct RemoteTools {
    mongodump: String,
    mongorestore: String,
    shell: String,
    node: String,
    zip: String,
    unzip: String,
    oss_put: String,
}

struct MongoAuth {
    shell_auth: String,
    restore_auth: String,
    label: String,
}

fn normalize_conn(conn: Option<&MongoConn>) -> MongoConn {
    let c = conn.cloned();
    MongoConn {
        host: Some(
            c.as_ref()
                .and_then(|x| x.host.clone())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "127.0.0.1".into()),
        ),
        port: Some(
            c.as_ref()
                .and_then(|x| x.port)
                .filter(|p| *p > 0)
                .unwrap_or(27017),
        ),
        username: c.as_ref().and_then(|x| x.username.clone()),
        password: c.as_ref().and_then(|x| x.password.clone()),
        auth_db: Some(
            c.as_ref()
                .and_then(|x| x.auth_db.clone())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "admin".into()),
        ),
    }
}

fn auth_args(conn: &MongoConn) -> String {
    let host = conn.host.as_deref().unwrap_or("127.0.0.1");
    let port = conn.port.unwrap_or(27017);
    let mut parts = vec![
        format!("--host {}", sh_quote(host)),
        format!("--port {}", sh_quote(&port.to_string())),
    ];
    let user = conn.username.as_deref().unwrap_or("").trim();
    if !user.is_empty() {
        parts.push(format!("-u {}", sh_quote(user)));
        if let Some(pass) = &conn.password {
            parts.push(format!("-p {}", sh_quote(pass)));
        }
        let auth_db = conn.auth_db.as_deref().unwrap_or("admin");
        parts.push(format!("--authenticationDatabase {}", sh_quote(auth_db)));
    }
    parts.join(" ")
}

fn parse_mongo_uri_to_conn(uri: &str) -> Option<MongoConn> {
    let raw = uri.trim();
    if raw.is_empty() || !raw.starts_with("mongodb://") {
        return None;
    }
    let without = raw.trim_start_matches("mongodb://");
    if without.contains(',') {
        return None;
    }
    let (auth_host, path_query) = without.split_once('/').unwrap_or((without, ""));
    let (auth, host_port) = if let Some((a, h)) = auth_host.rsplit_once('@') {
        (Some(a), h)
    } else {
        (None, auth_host)
    };
    let (host, port_str) = host_port
        .rsplit_once(':')
        .map(|(h, p)| (h, p.split('/').next().unwrap_or(p)))
        .unwrap_or((host_port, "27017"));
    let port: u16 = port_str.parse().unwrap_or(27017);
    let (username, password) = if let Some(a) = auth {
        let (u, p) = a.split_once(':').unwrap_or((a, ""));
        (
            Some(urlencoding::decode(u).map(|s| s.into_owned()).unwrap_or_else(|_| u.to_string())),
            Some(urlencoding::decode(p).map(|s| s.into_owned()).unwrap_or_else(|_| p.to_string())),
        )
    } else {
        (None, None)
    };
    let path_db = path_query.split('?').next().unwrap_or("").trim_matches('/');
    let auth_db = if let Some(q) = path_query.split('?').nth(1) {
        q.split('&')
            .find_map(|pair| {
                let mut kv = pair.splitn(2, '=');
                let k = kv.next()?;
                let v = kv.next()?;
                if k == "authSource" {
                    Some(v.to_string())
                } else {
                    None
                }
            })
            .or_else(|| {
                if path_db.is_empty() {
                    None
                } else {
                    Some(path_db.to_string())
                }
            })
    } else if !path_db.is_empty() {
        Some(path_db.to_string())
    } else {
        None
    };
    Some(normalize_conn(Some(&MongoConn {
        host: Some(host.to_string()),
        port: Some(port),
        username,
        password,
        auth_db,
    })))
}

fn resolve_mongo_auth(cfg: &BackupConfig, conn: Option<&MongoConn>) -> MongoAuth {
    let uri = cfg.mongodb_url.trim();
    if !uri.is_empty() {
        if let Some(parsed) = parse_mongo_uri_to_conn(uri) {
            let args = auth_args(&parsed);
            return MongoAuth {
                shell_auth: args.clone(),
                restore_auth: args,
                label: mask_mongo_uri(uri),
            };
        }
        return MongoAuth {
            shell_auth: sh_quote(uri),
            restore_auth: format!("--uri={}", sh_quote(uri)),
            label: mask_mongo_uri(uri),
        };
    }
    let c = normalize_conn(conn);
    let args = auth_args(&c);
    let label = format!(
        "服务器本机 Mongo {}:{}",
        c.host.as_deref().unwrap_or("127.0.0.1"),
        c.port.unwrap_or(27017)
    );
    MongoAuth {
        shell_auth: args.clone(),
        restore_auth: args,
        label,
    }
}

fn mask_mongo_uri(uri: &str) -> String {
    regex::Regex::new(r"//([^:@/]+):([^@/]+)@")
        .ok()
        .map(|re| re.replace(uri, "//$1:***@").to_string())
        .unwrap_or_else(|| uri.to_string())
}

fn normalize_size(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => s.parse().unwrap_or(0),
        Value::Object(o) => {
            if let Some(x) = o.get("$numberLong").or_else(|| o.get("$numberInt")) {
                return normalize_size(x);
            }
            0
        }
        _ => 0,
    }
}

async fn resolve_remote_tools(
    mgr: &SessionManager,
    session_id: &str,
    tuku_backend_root: &str,
) -> Result<RemoteTools, String> {
    let root = tuku_backend_root.trim().trim_end_matches('/');
    let mut put_paths = vec![sh_quote("/opt/tuku-backup/oss-put.mjs")];
    if !root.is_empty() {
        put_paths.push(sh_quote(&format!("{}/scripts/backup/oss-put.mjs", root)));
    }
    put_paths.push(sh_quote(
        "/www/wwwroot/backend.stwkjyxgs.com/tuku-backend/scripts/backup/oss-put.mjs",
    ));
    let put_candidates = put_paths.join(" ");

    let script = format!(
        r#"
set -e
find_bin() {{
  local name="$1"
  for p in \
    "/www/server/mongodb/bin/$name" \
    "/usr/local/mongodb/bin/$name" \
    "/opt/mongodb/bin/$name" \
    "/usr/bin/$name" \
    "/bin/$name"
  do
    if [ -x "$p" ]; then echo "$p"; return 0; fi
  done
  if command -v "$name" >/dev/null 2>&1; then command -v "$name"; return 0; fi
  return 1
}}
DUMP=$(find_bin mongodump || true)
RESTORE=$(find_bin mongorestore || true)
SHELL=$(find_bin mongosh || true)
if [ -z "$SHELL" ]; then SHELL=$(find_bin mongo || true); fi
NODE=$(command -v node || true)
ZIP=$(command -v zip || true)
UNZIP=$(command -v unzip || true)
echo "DUMP=$DUMP"
echo "RESTORE=$RESTORE"
echo "SHELL=$SHELL"
echo "NODE=$NODE"
echo "ZIP=$ZIP"
echo "UNZIP=$UNZIP"
for p in {puts}; do
  if [ -f "$p" ]; then echo "OSS_PUT=$p"; break; fi
done
"#,
        puts = put_candidates
    );
    let res = exec_command(mgr, session_id, &script, 30_000).await?;
    let mut map = HashMap::new();
    for line in res.stdout.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "DUMP" | "RESTORE" | "SHELL" | "NODE" | "ZIP" | "UNZIP" | "OSS_PUT" => {
                    map.insert(k, v.trim().to_string());
                }
                _ => {}
            }
        }
    }
    Ok(RemoteTools {
        mongodump: map.get("DUMP").cloned().unwrap_or_default(),
        mongorestore: map.get("RESTORE").cloned().unwrap_or_default(),
        shell: map.get("SHELL").cloned().unwrap_or_default(),
        node: map.get("NODE").cloned().unwrap_or_default(),
        zip: map.get("ZIP").cloned().unwrap_or_default(),
        unzip: map.get("UNZIP").cloned().unwrap_or_default(),
        oss_put: map.get("OSS_PUT").cloned().unwrap_or_default(),
    })
}

fn build_mongo_oss_env_exports(cfg: &BackupConfig) -> String {
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
    if !cfg.tuku_backend_root.is_empty() {
        lines.push(format!(
            "export TUKU_BACKEND_ROOT={}",
            sh_quote(&cfg.tuku_backend_root)
        ));
    }
    lines.join("\n")
}

pub async fn load_remote_backup_config(
    mgr: &SessionManager,
    session_id: &str,
) -> Result<BackupConfig, String> {
    let mut cfg = default_backup_config();
    let path = cfg.config_path.clone();
    let script = format!(
        r#"
set +e
CFG={path_q}
if [ -f "$CFG" ]; then
  echo "__CFG_BEGIN__"
  cat "$CFG"
  echo
  echo "__CFG_END__"
fi
ROOT=""
if [ -f "$CFG" ]; then
  ROOT=$(grep -E '^TUKU_BACKEND_ROOT=' "$CFG" | head -n1 | cut -d= -f2- | tr -d "\"'" || true)
fi
if [ -z "$ROOT" ]; then ROOT={root_q}; fi
ENVF="$ROOT/.env"
if [ -f "$ENVF" ]; then
  echo "__ENV_BEGIN__"
  grep -E '^(OSS_[A-Z0-9_]+|MONGODB_URL|TUKU_BACKEND_ROOT)=' "$ENVF" || true
  echo "__ENV_END__"
fi
"#,
        path_q = sh_quote(&path),
        root_q = sh_quote(&cfg.tuku_backend_root)
    );
    let read = exec_command(mgr, session_id, &script, 30_000).await?;
    let out = &read.stdout;
    if let Some(m) = regex::Regex::new(r"__CFG_BEGIN__\n([\s\S]*?)\n__CFG_END__")
        .ok()
        .and_then(|re| re.captures(out))
    {
        cfg = apply_env_map(&cfg, &parse_env_text(m.get(1).map(|c| c.as_str()).unwrap_or("")));
    }
    if let Some(m) = regex::Regex::new(r"__ENV_BEGIN__\n([\s\S]*?)\n__ENV_END__")
        .ok()
        .and_then(|re| re.captures(out))
    {
        let env = parse_env_text(m.get(1).map(|c| c.as_str()).unwrap_or(""));
        let cur = apply_env_map(&default_backup_config(), &parse_env_text(""));
        let mut filled = HashMap::new();
        let keys: [(&str, &str); 10] = [
            ("TUKU_BACKEND_ROOT", "tuku_backend_root"),
            ("MONGODB_URL", "mongodb_url"),
            ("OSS_ACCESS_KEY_ID", "oss_access_key_id"),
            ("OSS_ACCESS_KEY_SECRET", "oss_access_key_secret"),
            ("OSS_BUCKET", "oss_bucket"),
            ("OSS_REGION", "oss_region"),
            ("OSS_ENDPOINT", "oss_endpoint"),
            ("OSS_INTERNAL", "oss_internal"),
            ("OSS_BACKUP_ROOT", "oss_backup_root"),
            ("OSS_MONGODB_PREFIX", "oss_mongodb_prefix"),
        ];
        for (ek, field) in keys {
            let existing = match field {
                "tuku_backend_root" => &cur.tuku_backend_root,
                "mongodb_url" => &cur.mongodb_url,
                "oss_access_key_id" => &cur.oss_access_key_id,
                "oss_access_key_secret" => &cur.oss_access_key_secret,
                "oss_bucket" => &cur.oss_bucket,
                "oss_region" => &cur.oss_region,
                "oss_endpoint" => &cur.oss_endpoint,
                "oss_backup_root" => &cur.oss_backup_root,
                "oss_mongodb_prefix" => &cur.oss_mongodb_prefix,
                _ => &String::new(),
            };
            if existing.is_empty() {
                if let Some(v) = env.get(ek) {
                    filled.insert(ek.to_string(), v.clone());
                }
            }
        }
        cfg = apply_env_map(&cfg, &filled);
    }
    cfg.config_path = path;
    if cfg.oss_endpoint.is_empty() {
        cfg.oss_endpoint = format!("{}.aliyuncs.com", cfg.oss_region);
    }
    if cfg.oss_mongodb_prefix.is_empty() {
        cfg.oss_mongodb_prefix = format!("{}/mongodb", cfg.oss_backup_root);
    }
    Ok(cfg)
}

pub async fn save_remote_backup_config(
    mgr: &SessionManager,
    session_id: &str,
    cfg: BackupConfig,
) -> Result<BackupConfig, String> {
    let path = if cfg.config_path.trim().is_empty() {
        DEFAULT_CONFIG_PATH.to_string()
    } else {
        cfg.config_path.trim().to_string()
    };
    let content = serialize_backup_config(&BackupConfig {
        config_path: path.clone(),
        ..cfg.clone()
    });
    let b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        content.as_bytes(),
    );
    let res = exec_command(
        mgr,
        session_id,
        &format!(
            r#"
set -e
PATH_CFG={path_q}
umask 027
printf '%s' {b64_q} | base64 -d > "$PATH_CFG"
chmod 640 "$PATH_CFG" || true
echo SAVE_OK
"#,
            path_q = sh_quote(&path),
            b64_q = sh_quote(&b64)
        ),
        30_000,
    )
    .await?;
    if res.code != 0 || !res.stdout.contains("SAVE_OK") {
        let stderr = res.stderr;
        let stdout = res.stdout;
        return Err(if !stderr.trim().is_empty() {
            stderr
        } else if !stdout.trim().is_empty() {
            stdout
        } else {
            "保存备份配置失败（可能需要 root 权限）".into()
        });
    }
    Ok(BackupConfig {
        config_path: path,
        ..cfg
    })
}

pub async fn list_remote_databases(
    mgr: &SessionManager,
    session_id: &str,
    conn: Option<MongoConn>,
    cfg_input: Option<BackupConfig>,
) -> Result<Vec<MongoDbInfo>, String> {
    let cfg = cfg_input.unwrap_or_else(default_backup_config);
    let auth = resolve_mongo_auth(&cfg, conn.as_ref());
    let tools = resolve_remote_tools(mgr, session_id, &cfg.tuku_backend_root).await?;
    if tools.shell.is_empty() {
        return Err("远程未找到 mongosh/mongo，请先在服务器安装 MongoDB Shell".into());
    }
    let eval_js = "JSON.stringify(db.adminCommand({listDatabases:1}).databases.map(function(d){return {name:String(d.name),sizeOnDisk:Number(d.sizeOnDisk)||0,empty:!!d.empty}}))";
    let cmd = format!(
        "{} --quiet {} --eval {}",
        sh_quote(&tools.shell),
        auth.shell_auth,
        sh_quote(eval_js)
    );
    let res = exec_command(mgr, session_id, &cmd, 60_000).await?;
    if res.code != 0 {
        return Err(
            if !res.stderr.is_empty() {
                res.stderr
            } else {
                res.stdout
            },
        );
    }
    let text = res.stdout.trim();
    let start = text.rfind('[').unwrap_or(0);
    let end = text.rfind(']').unwrap_or(0);
    if end < start {
        return Err(format!("无法解析数据库列表：{}", text.chars().take(200).collect::<String>()));
    }
    let raw: Vec<Value> = serde_json::from_str(&text[start..=end]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for d in raw {
        let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() || name == "local" {
            continue;
        }
        out.push(MongoDbInfo {
            name,
            size_on_disk: normalize_size(d.get("sizeOnDisk").unwrap_or(&Value::Null)),
            empty: d.get("empty").and_then(|v| v.as_bool()).unwrap_or(false),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn parse_local_backup_list(stdout: &str) -> Vec<LocalBackupItem> {
    let block = regex::Regex::new(r"__LIST_BEGIN__\r?\n([\s\S]*?)\r?\n__LIST_END__")
        .ok()
        .and_then(|re| re.captures(stdout))
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .or_else(|| {
            regex::Regex::new(r"__LIST_BEGIN__\r?\n([\s\S]*?)__LIST_END__")
                .ok()
                .and_then(|re| re.captures(stdout))
                .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        })
        .unwrap_or_else(|| stdout.trim().to_string());
    let trimmed = block.trim();
    let start = trimmed.rfind('[').unwrap_or(0);
    let end = trimmed.rfind(']').unwrap_or(0);
    if end < start {
        return Vec::new();
    }
    let list: Vec<LocalBackupItem> =
        serde_json::from_str(&trimmed[start..=end]).unwrap_or_default();
    list.into_iter()
        .filter(|x| !x.name.is_empty() && !x.path.is_empty() && is_mongo_backup_file_name(&x.name))
        .collect()
}

pub async fn list_local_backups(
    mgr: &SessionManager,
    session_id: &str,
    cfg: &BackupConfig,
) -> Result<Vec<LocalBackupItem>, String> {
    let dir = cfg.mongo_backup_dir.trim();
    let script = format!(
        r#"
set +e
DIR={dir_q}
if [ ! -d "$DIR" ]; then echo "__LIST_BEGIN__"; echo "[]"; echo "__LIST_END__"; exit 0; fi
echo "__LIST_BEGIN__"
echo -n "["
first=1
shopt -s nullglob
for f in "$DIR"/*_mc.zip "$DIR"/*_mongodb_data.zip "$DIR"/mongodb_*.archive.gz; do
  [ -f "$f" ] || continue
  name=$(basename -- "$f")
  size=$(stat -c %s -- "$f" 2>/dev/null || stat -f %z -- "$f" 2>/dev/null || echo 0)
  mtime=$(stat -c %Y -- "$f" 2>/dev/null || stat -f %m -- "$f" 2>/dev/null || echo 0)
  mtimeMs=$((mtime * 1000))
  jname=$(printf '%s' "$name" | sed 's/[\\\\"]/\\\\&/g')
  jpath=$(printf '%s' "$f" | sed 's/[\\\\"]/\\\\&/g')
  if [ "$first" = 1 ]; then first=0; else echo -n ","; fi
  printf '{{"name":"%s","path":"%s","size":%s,"mtimeMs":%s}}' "$jname" "$jpath" "$size" "$mtimeMs"
done
echo "]"
echo "__LIST_END__"
"#,
        dir_q = sh_quote(dir)
    );
    let res = exec_command(mgr, session_id, &script, 60_000).await?;
    let mut list = parse_local_backup_list(&res.stdout);
    list.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    Ok(list)
}

pub async fn delete_local_backup(
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
) -> Result<(), String> {
    let p = remote_path.trim();
    if !p.starts_with('/') || p.contains("..") {
        return Err("非法备份路径".into());
    }
    let name = Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !is_mongo_backup_file_name(name) {
        return Err("仅允许删除宝塔备份 zip（*_mc.zip）或兼容旧档".into());
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

async fn collect_remote_collection_stats(
    mgr: &SessionManager,
    session_id: &str,
    db: &str,
    auth: &MongoAuth,
    shell: &str,
) -> Result<HashMap<String, BackupMetaCollection>, String> {
    let eval_js = format!(
        "{}; JSON.stringify(collectStats({}))",
        COLLECTION_STATS_JS,
        serde_json::to_string(db).unwrap_or_default()
    );
    let cmd = format!(
        "{} --quiet {} --eval {}",
        sh_quote(shell),
        auth.shell_auth,
        sh_quote(&eval_js)
    );
    let res = exec_command(mgr, session_id, &cmd, 10 * 60 * 1000).await?;
    if res.code != 0 {
        return Err(if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        });
    }
    let text = res.stdout.trim();
    let start = text.rfind('{').unwrap_or(0);
    let end = text.rfind('}').unwrap_or(0);
    if end < start {
        return Err(format!(
            "无法解析集合统计：{}",
            text.chars().take(200).collect::<String>()
        ));
    }
    let raw: HashMap<String, Value> =
        serde_json::from_str(&text[start..=end]).map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for (name, item) in raw {
        out.insert(
            name,
            BackupMetaCollection {
                count: item.get("count").and_then(|v| v.as_u64()).unwrap_or(0),
                max_id: item
                    .get("maxId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                update_field: item
                    .get("updateField")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            },
        );
    }
    Ok(out)
}

async fn read_remote_backup_chain(
    mgr: &SessionManager,
    session_id: &str,
    backup_dir: &str,
    db: &str,
) -> Result<Option<MongoBackupChain>, String> {
    let path = format!(
        "{}/{}",
        backup_dir.trim_end_matches('/'),
        chain_file_name(db)
    );
    let res = exec_command(
        mgr,
        session_id,
        &format!("cat {} 2>/dev/null || true", sh_quote(&path)),
        30_000,
    )
    .await?;
    Ok(parse_backup_chain(&res.stdout))
}

async fn write_remote_backup_chain(
    mgr: &SessionManager,
    session_id: &str,
    backup_dir: &str,
    chain: &MongoBackupChain,
) -> Result<(), String> {
    let path = format!(
        "{}/{}",
        backup_dir.trim_end_matches('/'),
        chain_file_name(&chain.db)
    );
    let b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        serialize_backup_chain(chain).as_bytes(),
    );
    let res = exec_command(
        mgr,
        session_id,
        &format!(
            r#"
set -e
umask 027
printf '%s' {b64_q} | base64 -d > {path_q}
echo CHAIN_OK
"#,
            b64_q = sh_quote(&b64),
            path_q = sh_quote(&path)
        ),
        30_000,
    )
    .await?;
    if res.code != 0 || !res.stdout.contains("CHAIN_OK") {
        return Err(if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        });
    }
    Ok(())
}

pub async fn load_remote_backup_chain(
    mgr: &SessionManager,
    session_id: &str,
    db: &str,
    cfg_input: Option<BackupConfig>,
) -> Result<Option<MongoBackupChain>, String> {
    let cfg = cfg_input.unwrap_or_else(default_backup_config);
    let dir = cfg.mongo_backup_dir.trim_end_matches('/');
    let db_name = db.trim();
    if db_name.is_empty() {
        return Ok(None);
    }
    read_remote_backup_chain(mgr, session_id, dir, db_name).await
}

async fn append_chain_entry(
    mgr: &SessionManager,
    session_id: &str,
    backup_dir: &str,
    meta: &MongoBackupMeta,
) -> Result<(), String> {
    let existing = read_remote_backup_chain(mgr, session_id, backup_dir, &meta.db).await?;
    let entries = existing
        .map(|c| c.entries)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.backup_id != meta.backup_id)
        .chain(std::iter::once(meta.clone()))
        .collect();
    let chain = MongoBackupChain {
        version: 1,
        db: meta.db.clone(),
        entries,
    };
    write_remote_backup_chain(mgr, session_id, backup_dir, &chain).await
}

async fn prune_old_server_backups(
    mgr: &SessionManager,
    session_id: &str,
    dir: &str,
    retain_days: u32,
) {
    if retain_days == 0 {
        return;
    }
    let days = retain_days;
    let _ = exec_command(
        mgr,
        session_id,
        &format!(
            "find {} -maxdepth 1 -name '*_mc.zip' -mtime +{} -delete || true\nfind {} -maxdepth 1 -name '*_mongodb_data.zip' -mtime +{} -delete || true\nfind {} -maxdepth 1 -name 'mongodb_*.archive.gz' -mtime +{} -delete || true",
            sh_quote(dir),
            days,
            sh_quote(dir),
            days,
            sh_quote(dir),
            days
        ),
        60_000,
    )
    .await;
}

async fn upload_via_local_client(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    oss_key: &str,
    cfg: &BackupConfig,
    on_message: Option<Box<dyn Fn(&str, u32) + Send + Sync>>,
) -> Result<(), String> {
    let file_name = Path::new(remote_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "backup".into());
    let tmp = oss::temp_download_path("sshdesk-mongo", &file_name);
    let tmp_s = tmp.to_string_lossy().to_string();
    if let Some(f) = &on_message {
        f("经本机中转上传 OSS（服务器无 oss-put）…", 72);
    }
    let _ = download_remote_file_with_progress(
        app,
        mgr,
        session_id,
        remote_path,
        &tmp_s,
        None,
    )
    .await?;
    if let Some(f) = &on_message {
        f("本机上传 OSS…", 86);
    }
    let oss_cfg = to_oss_config(cfg);
    oss::upload_object(&oss_cfg, &tmp_s, oss_key, None).await?;
    if cfg.oss_mongodb_retain_days > 0 {
        let _ = oss::cleanup_retain_days(&oss_cfg, cfg.oss_mongodb_retain_days, |name| {
            name.to_ascii_lowercase().ends_with("_mc.zip")
                || name.to_ascii_lowercase().ends_with("_mongodb_data.zip")
                || name.to_ascii_lowercase().starts_with("mongodb_")
                    && name.to_ascii_lowercase().ends_with(".archive.gz")
        })
        .await;
    }
    let _ = tokio::fs::remove_file(&tmp).await;
    Ok(())
}

async fn upload_backup_to_oss(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    oss_key: &str,
    cfg: &BackupConfig,
    tools: &RemoteTools,
    progress: Option<Arc<dyn Fn(BackupProgress) + Send + Sync>>,
) -> Result<bool, String> {
    let report = |phase: &str, message: &str, percent: u32| {
        if let Some(p) = &progress {
            p(BackupProgress {
                phase: phase.to_string(),
                message: message.to_string(),
                percent,
            });
        }
    };
    if cfg.oss_access_key_id.is_empty()
        || cfg.oss_access_key_secret.is_empty()
        || cfg.oss_bucket.is_empty()
    {
        report("done", "本地备份已完成（未配置 OSS，跳过上传）", 100);
        return Ok(false);
    }
    report("upload", "正在上传到 OSS…", 70);
    let prefix = cfg
        .oss_mongodb_prefix
        .trim_end_matches('/')
        .to_string();
    if !tools.oss_put.is_empty() && !tools.node.is_empty() {
        let env_exports = build_mongo_oss_env_exports(cfg);
        report("upload", "服务器直传 OSS…", 72);
        let up = exec_command(
            mgr,
            session_id,
            &format!(
                r#"
{env_exports}
set -e
{node} {put} put {remote} {key}
echo UPLOAD_OK
"#,
                env_exports = env_exports,
                node = sh_quote(&tools.node),
                put = sh_quote(&tools.oss_put),
                remote = sh_quote(remote_path),
                key = sh_quote(oss_key)
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
        if cfg.oss_mongodb_retain_days > 0 {
            let _ = exec_command(
                mgr,
                session_id,
                &format!(
                    r#"
{env_exports}
{node} {put} cleanup {prefix} {days} --name-glob '*_mc.zip' || true
{node} {put} cleanup {prefix} {days} --name-glob '*_mongodb_data.zip' || true
"#,
                    env_exports = env_exports,
                    node = sh_quote(&tools.node),
                    put = sh_quote(&tools.oss_put),
                    prefix = sh_quote(&prefix),
                    days = cfg.oss_mongodb_retain_days
                ),
                10 * 60 * 1000,
            )
            .await;
        }
        return Ok(true);
    }
    upload_via_local_client(
        app,
        mgr,
        session_id,
        remote_path,
        oss_key,
        cfg,
        progress.clone().map(|p| {
            Box::new(move |msg: &str, pct: u32| {
                p(BackupProgress {
                    phase: "upload".into(),
                    message: msg.to_string(),
                    percent: pct,
                });
            }) as Box<dyn Fn(&str, u32) + Send + Sync>
        }),
    )
    .await?;
    Ok(true)
}

pub async fn backup_remote_database(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    db_name: &str,
    conn: Option<MongoConn>,
    cfg_input: BackupConfig,
    on_progress: Option<Box<dyn Fn(BackupProgress) + Send + Sync>>,
) -> Result<Value, String> {
    let db = db_name.trim();
    if db.is_empty() {
        return Err("请选择数据库".into());
    }
    let cfg = cfg_input;
    let progress: Option<Arc<dyn Fn(BackupProgress) + Send + Sync>> =
        on_progress.map(Arc::from);
    let emit = |phase: &str, message: &str, percent: u32| {
        if let Some(p) = &progress {
            p(BackupProgress {
                phase: phase.to_string(),
                message: message.to_string(),
                percent,
            });
        }
    };
    emit("prepare", "探测远程工具…", 5);
    let tools = resolve_remote_tools(mgr, session_id, &cfg.tuku_backend_root).await?;
    if tools.mongodump.is_empty() {
        return Err("远程未找到 mongodump（可装宝塔 MongoDB 或 mongodb-database-tools）".into());
    }
    if tools.zip.is_empty() {
        return Err("远程未找到 zip，请安装：yum/apt install zip".into());
    }
    let file_name = bt_mc_zip_name(db, "full");
    let dir = cfg.mongo_backup_dir.trim_end_matches('/');
    let work = format!("{}/.sshdesk_tmp_{}", dir, chrono::Utc::now().timestamp_millis());
    let dump_root = format!("{}/dump", work);
    let remote_path = format!("{}/{}", dir, file_name);
    let prefix = cfg.oss_mongodb_prefix.trim_end_matches('/');
    let oss_key = format!("{}/{}", prefix, file_name);

    emit("dump", &format!("正在 mongodump：{}", db), 18);
    let dump_auth = resolve_mongo_auth(&cfg, conn.as_ref()).restore_auth;
    let dump_cmd = format!(
        r#"
set -e
rm -rf {work_q}
mkdir -p {dump_q}
{dump} {auth} --db {db_q} --out {dump_q}
test -d {db_dir_q}
echo DUMP_OK
"#,
        work_q = sh_quote(&work),
        dump_q = sh_quote(&dump_root),
        dump = sh_quote(&tools.mongodump),
        auth = dump_auth,
        db_q = sh_quote(db),
        db_dir_q = sh_quote(&format!("{}/{}", dump_root, db))
    );
    let dump_res = exec_command(mgr, session_id, &dump_cmd, 60 * 60 * 1000).await?;
    if dump_res.code != 0 || !dump_res.stdout.contains("DUMP_OK") {
        let _ = exec_command(mgr, session_id, &format!("rm -rf {}", sh_quote(&work)), 60_000).await;
        return Err(if !dump_res.stderr.is_empty() {
            dump_res.stderr
        } else {
            dump_res.stdout
        });
    }

    emit("zip", "统计集合并打包…", 42);
    if tools.shell.is_empty() {
        return Err("远程未找到 mongosh/mongo，无法写入增量元数据".into());
    }
    let auth = resolve_mongo_auth(&cfg, conn.as_ref());
    let collections =
        collect_remote_collection_stats(mgr, session_id, db, &auth, &tools.shell).await?;
    let backup_id = parse_backup_id_from_file_name(&file_name)
        .ok_or_else(|| "无法解析备份 ID".to_string())?;
    let meta = MongoBackupMeta {
        version: 1,
        backup_type: "full".into(),
        db: db.to_string(),
        backup_id: backup_id.clone(),
        file_name: file_name.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        since: None,
        base_backup_id: backup_id,
        parent_backup_id: None,
        collections,
        changed_collections: None,
    };
    let meta_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        serde_json::to_string(&meta).unwrap_or_default(),
    );
    let zip_cmd = format!(
        r#"
set -e
mkdir -p {dir_q} {pack_q}
rm -f {remote_q}
printf '%s' {meta_b64_q} | base64 -d > {meta_path_q}
cd {dump_root_q}
{zip} -m -r {remote_q} {db_q}
cd {work_q}
{zip} -j {remote_q} {meta_name_q}
test -f {remote_q}
unzip -l {remote_q} | grep -E ' {db}/' >/dev/null
rm -rf {work_q}
echo ZIP_OK
"#,
        dir_q = sh_quote(dir),
        pack_q = sh_quote(&format!("{}/pack", work)),
        remote_q = sh_quote(&remote_path),
        meta_b64_q = sh_quote(&meta_b64),
        meta_path_q = sh_quote(&format!("{}/{}", work, BACKUP_META_NAME)),
        dump_root_q = sh_quote(&dump_root),
        zip = sh_quote(&tools.zip),
        db_q = sh_quote(db),
        work_q = sh_quote(&work),
        meta_name_q = sh_quote(BACKUP_META_NAME)
    );
    let zip_res = exec_command(mgr, session_id, &zip_cmd, 30 * 60 * 1000).await?;
    if zip_res.code != 0 || !zip_res.stdout.contains("ZIP_OK") {
        let _ = exec_command(
            mgr,
            session_id,
            &format!(
                "rm -rf {}; rm -f {}",
                sh_quote(&work),
                sh_quote(&remote_path)
            ),
            60_000,
        )
        .await;
        return Err(if !zip_res.stderr.is_empty() {
            zip_res.stderr
        } else {
            zip_res.stdout
        });
    }
    append_chain_entry(mgr, session_id, dir, &meta).await?;
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
    prune_old_server_backups(mgr, session_id, dir, cfg.mongo_backup_retain_days).await;
    let uploaded =
        upload_backup_to_oss(app, mgr, session_id, &remote_path, &oss_key, &cfg, &tools, progress.clone())
            .await?;
    emit(
        "done",
        if uploaded {
            "全量备份完成并已上传 OSS（服务器本地已保留）"
        } else {
            "全量备份完成"
        },
        100,
    );
    Ok(serde_json::json!({
        "fileName": file_name,
        "remotePath": remote_path,
        "ossKey": oss_key,
        "bytes": bytes,
        "uploaded": uploaded
    }))
}

// incremental + upload_server + oss + restore_remote — continued in same module
include!("remote_extra.rs");
