use super::*;
use crate::platform::downloads_dir;

struct IncrPlan {
    mode: String,
    query: Option<Value>,
}

fn plan_incremental_collections(
    prev: &HashMap<String, BackupMetaCollection>,
    current: &HashMap<String, BackupMetaCollection>,
    since: &str,
) -> HashMap<String, IncrPlan> {
    let mut plan = HashMap::new();
    for (name, cur) in current {
        let old = prev.get(name);
        if old.is_none() {
            plan.insert(name.clone(), IncrPlan { mode: "full".into(), query: None });
            continue;
        }
        let old = old.unwrap();
        if cur.count == old.count && cur.max_id == old.max_id {
            continue;
        }
        if cur.count < old.count {
            plan.insert(name.clone(), IncrPlan { mode: "full".into(), query: None });
            continue;
        }
        let mut or = Vec::new();
        if let (Some(om), Some(cm)) = (&old.max_id, &cur.max_id) {
            if om != cm {
                or.push(serde_json::json!({ "_id": { "$gt": { "$oid": om } } }));
            }
        }
        let uf = cur.update_field.as_ref().or(old.update_field.as_ref());
        if let Some(f) = uf {
            let mut obj = serde_json::Map::new();
            obj.insert(
                f.clone(),
                serde_json::json!({ "$gt": { "$date": since } }),
            );
            or.push(Value::Object(obj));
        }
        if or.is_empty() {
            plan.insert(name.clone(), IncrPlan { mode: "full".into(), query: None });
        } else if or.len() == 1 {
            plan.insert(name.clone(), IncrPlan {
                mode: "query".into(),
                query: Some(or[0].clone()),
            });
        } else {
            plan.insert(name.clone(), IncrPlan {
                mode: "query".into(),
                query: Some(serde_json::json!({ "$or": or })),
            });
        }
    }
    plan
}

pub async fn incremental_backup_remote_database(
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
    if tools.mongodump.is_empty() || tools.zip.is_empty() || tools.shell.is_empty() {
        return Err("远程未找到 mongodump/zip/mongosh".into());
    }
    let dir = cfg.mongo_backup_dir.trim_end_matches('/');
    let chain = read_remote_backup_chain(mgr, session_id, dir, db).await?;
    let latest = latest_chain_entry(&chain);
    let latest = latest.ok_or_else(|| "请先做一次全量备份，再执行增量备份".to_string())?;
    let auth = resolve_mongo_auth(&cfg, conn.as_ref());
    emit("dump", "对比集合变更…", 18);
    let current =
        collect_remote_collection_stats(mgr, session_id, db, &auth, &tools.shell).await?;
    let since = latest.created_at.clone();
    let plan = plan_incremental_collections(&latest.collections, &current, &since);
    let changed: Vec<String> = plan.keys().cloned().collect();
    if changed.is_empty() {
        return Err("自上次备份以来无数据变更，已跳过增量备份".into());
    }
    let file_name = bt_mc_zip_name(db, "incremental");
    let backup_id = parse_backup_id_from_file_name(&file_name)
        .ok_or_else(|| "无法解析备份 ID".to_string())?;
    let work = format!("{}/.sshdesk_tmp_{}", dir, chrono::Utc::now().timestamp_millis());
    let dump_root = format!("{}/dump", work);
    let remote_path = format!("{}/{}", dir, file_name);
    let prefix = cfg.oss_mongodb_prefix.trim_end_matches('/');
    let oss_key = format!("{}/{}", prefix, file_name);
    let dump_auth = auth.restore_auth.clone();
    let base_backup_id = if latest.base_backup_id.is_empty() {
        latest.backup_id.clone()
    } else {
        latest.base_backup_id.clone()
    };

    let mut dump_lines = vec![
        "set -e".to_string(),
        format!("rm -rf {}", sh_quote(&work)),
        format!("mkdir -p {}", sh_quote(&dump_root)),
    ];
    for (coll, item) in &plan {
        if item.mode == "full" {
            dump_lines.push(format!(
                "{} {} --db {} --collection {} --out {}",
                sh_quote(&tools.mongodump),
                dump_auth,
                sh_quote(db),
                sh_quote(coll),
                sh_quote(&dump_root)
            ));
        } else {
            let query = serde_json::to_string(&item.query.clone().unwrap_or(Value::Null))
                .unwrap_or("{}".into());
            let qpath = format!("{}/q_{}.json", work, coll);
            dump_lines.push(format!(
                "printf '%s' {} > {}",
                sh_quote(&query),
                sh_quote(&qpath)
            ));
            dump_lines.push(format!(
                "{} {} --db {} --collection {} --query \"$(cat {})\" --out {}",
                sh_quote(&tools.mongodump),
                dump_auth,
                sh_quote(db),
                sh_quote(coll),
                sh_quote(&qpath),
                sh_quote(&dump_root)
            ));
        }
    }
    dump_lines.push(format!("test -d {}", sh_quote(&format!("{}/{}", dump_root, db))));
    dump_lines.push("echo DUMP_OK".into());
    let dump_res = exec_command(mgr, session_id, &dump_lines.join("\n"), 60 * 60 * 1000).await?;
    if dump_res.code != 0 || !dump_res.stdout.contains("DUMP_OK") {
        let _ = exec_command(mgr, session_id, &format!("rm -rf {}", sh_quote(&work)), 60_000).await;
        return Err(if !dump_res.stderr.is_empty() {
            dump_res.stderr
        } else {
            dump_res.stdout
        });
    }

    let meta = MongoBackupMeta {
        version: 1,
        backup_type: "incremental".into(),
        db: db.to_string(),
        backup_id,
        file_name: file_name.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        since: Some(since),
        base_backup_id,
        parent_backup_id: Some(latest.backup_id.clone()),
        collections: current,
        changed_collections: Some(changed.clone()),
    };
    let meta_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        serde_json::to_string(&meta).unwrap_or_default(),
    );
    emit("zip", &format!("打包增量（{} 个集合）…", changed.len()), 48);
    let zip_cmd = format!(
        r#"
set -e
mkdir -p {dir_q}
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
    let done_msg = if uploaded {
        format!("增量备份完成（{} 个集合）并已上传 OSS", changed.len())
    } else {
        format!("增量备份完成（{} 个集合）", changed.len())
    };
    emit("done", &done_msg, 100);
    Ok(serde_json::json!({
        "fileName": file_name,
        "remotePath": remote_path,
        "ossKey": oss_key,
        "bytes": bytes,
        "uploaded": uploaded,
        "changedCollections": changed
    }))
}

async fn push_remote_mongo_to_oss(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    oss_key: &str,
    cfg: &BackupConfig,
    on_message: Option<Box<dyn Fn(&str, u32) + Send + Sync>>,
) -> Result<String, String> {
    let tools = resolve_remote_tools(mgr, session_id, &cfg.tuku_backend_root).await?;
    let prefix = cfg.oss_mongodb_prefix.trim_end_matches('/');
    if !tools.oss_put.is_empty() && !tools.node.is_empty() {
        let via = if cfg.oss_internal {
            "server-internal"
        } else {
            "server-public"
        };
        if let Some(f) = &on_message {
            f(
                if cfg.oss_internal {
                    "服务器内网直传 OSS…"
                } else {
                    "服务器直传 OSS…"
                },
                35,
            );
        }
        let env_exports = build_mongo_oss_env_exports(cfg);
        let up = exec_command(
            mgr,
            session_id,
            &format!(
                r#"
{env_exports}
set -e
test -f {remote}
{node} {put} put {remote} {key}
echo UPLOAD_OK
"#,
                env_exports = env_exports,
                remote = sh_quote(remote_path),
                node = sh_quote(&tools.node),
                put = sh_quote(&tools.oss_put),
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
        if let Some(f) = &on_message {
            f("上传完成", 92);
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
                    prefix = sh_quote(prefix),
                    days = cfg.oss_mongodb_retain_days
                ),
                10 * 60 * 1000,
            )
            .await;
        }
        return Ok(via.into());
    }
    upload_via_local_client(app, mgr, session_id, remote_path, oss_key, cfg, on_message).await?;
    Ok("local-relay".into())
}

pub async fn upload_server_backup_to_oss(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    remote_path: &str,
    cfg_input: BackupConfig,
    on_progress: Option<Box<dyn Fn(BackupProgress) + Send + Sync>>,
) -> Result<Value, String> {
    let p = remote_path.trim();
    if !p.starts_with('/') || p.contains("..") {
        return Err("非法备份路径".into());
    }
    let file_name = Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !is_mongo_backup_file_name(&file_name) {
        return Err("不是可上传的 Mongo 备份文件".into());
    }
    let cfg = cfg_input;
    if cfg.oss_access_key_id.is_empty()
        || cfg.oss_access_key_secret.is_empty()
        || cfg.oss_bucket.is_empty()
    {
        return Err("请先在设置里填写 OSS".into());
    }
    let emit = |phase: &str, message: &str, percent: u32| {
        if let Some(cb) = &on_progress {
            cb(BackupProgress {
                phase: phase.to_string(),
                message: message.to_string(),
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
    let prefix = cfg.oss_mongodb_prefix.trim_end_matches('/');
    let oss_key = format!("{}/{}", prefix, file_name);
    emit("upload", "准备上传 OSS…", 15);
    let via = push_remote_mongo_to_oss(app, mgr, session_id, p, &oss_key, &cfg, None).await?;
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

pub async fn list_oss_backups(cfg: &BackupConfig) -> Result<Vec<OssBackupItem>, String> {
    let oss_cfg = to_oss_config(cfg);
    oss::list_by_prefix(&oss_cfg, is_mongo_backup_file_name).await
}

pub async fn download_backup_from_oss(
    cfg: &BackupConfig,
    object_key: &str,
    local_dir: &str,
    on_progress: Option<Box<dyn Fn(BackupProgress) + Send + Sync>>,
) -> Result<Value, String> {
    tokio::fs::create_dir_all(local_dir)
        .await
        .map_err(|e| e.to_string())?;
    let file_name = oss::object_basename(object_key);
    let file_name_for_progress = file_name.clone();
    let local_path = crate::local_backups::unique_local_path(
        local_dir,
        &file_name,
        r"(\.archive\.gz|_mongodb_data\.zip|_mc\.zip)$",
    );
    let emit = |phase: &str, message: &str, percent: u32| {
        if let Some(cb) = &on_progress {
            cb(BackupProgress {
                phase: phase.to_string(),
                message: message.to_string(),
                percent,
            });
        }
    };
    emit("download", "正在从 OSS 下载…", 5);
    let oss_cfg = to_oss_config(cfg);
    let bytes = oss::download_object(
        &oss_cfg,
        object_key,
        &local_path.to_string_lossy(),
        None,
    )
    .await?;
    emit("done", "下载完成", 100);
    Ok(serde_json::json!({
        "fileName": local_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(file_name),
        "localPath": local_path.to_string_lossy().to_string(),
        "bytes": bytes
    }))
}

pub async fn remove_oss_backup(cfg: &BackupConfig, object_key: &str) -> Result<(), String> {
    oss::delete_object(&to_oss_config(cfg), object_key).await
}

pub fn default_download_dir() -> String {
    downloads_dir().to_string_lossy().to_string()
}

pub async fn restore_remote_database_from_local_file(
    app: &AppHandle,
    mgr: &SessionManager,
    session_id: &str,
    local_path: &str,
    target_db: &str,
    mode: &str,
    cfg_input: BackupConfig,
    conn: Option<MongoConn>,
    source_db_hint: Option<&str>,
    on_progress: Option<Box<dyn Fn(BackupProgress) + Send + Sync>>,
) -> Result<Value, String> {
    let db = target_db.trim();
    if db.is_empty() {
        return Err("请指定目标数据库名".into());
    }
    if !Path::new(local_path).is_file() {
        return Err(format!("备份文件不存在：{}", local_path));
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
    emit("prepare", "探测远程 mongorestore…", 5);
    let tools = resolve_remote_tools(mgr, session_id, &cfg.tuku_backend_root).await?;
    if tools.mongorestore.is_empty() {
        return Err("远程未找到 mongorestore".into());
    }
    let remote_file = format!(
        "/tmp/sshdesk_restore_{}_{}",
        chrono::Utc::now().timestamp_millis(),
        Path::new(local_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "backup".into())
    );
    emit("upload", "上传备份到服务器…", 12);
    let _ = upload_remote_file(
        app,
        mgr,
        session_id,
        local_path,
        &remote_file,
        progress.clone().map(|p| {
            Box::new(move |t: u64, total: u64| {
                let pct = if total > 0 {
                    12 + ((t * 38) / total.max(1)) as u32
                } else {
                    40
                };
                p(BackupProgress {
                    phase: "upload".into(),
                    message: format!("上传 {}%", (t * 100 / total.max(1)).min(99)),
                    percent: pct,
                });
            }) as Box<dyn Fn(u64, u64) + Send + Sync>
        }),
    )
    .await?;
    let mongo_auth = resolve_mongo_auth(&cfg, conn.as_ref());
    let restore_auth = mongo_auth.restore_auth;
    let hint = source_db_hint.unwrap_or("").trim();
    emit(
        "restore",
        &format!(
            "正在恢复到远程 {}（{}）…",
            db,
            if mode == "replace" { "覆盖" } else { "合并" }
        ),
        55,
    );
    let unzip = if tools.unzip.is_empty() {
        "unzip"
    } else {
        tools.unzip.as_str()
    };
    let restore_script = format!(
        r#"
set -e
ARCHIVE={arch_q}
TARGET={db_q}
MODE={mode_q}
HINT={hint_q}
MR={mr_q}
UNZIP={unzip_q}
WORK="/tmp/sshdesk_restore_work_$$"
mkdir -p "$WORK"
cleanup() {{ rm -rf "$WORK" "$ARCHIVE"; }}
trap cleanup EXIT

find_dump_root() {{
  local base="$1"
  if compgen -G "$base/*.bson" >/dev/null 2>&1; then echo "$base"; return 0; fi
  for d in "$base"/*; do
    [ -d "$d" ] || continue
    if compgen -G "$d/*.bson" >/dev/null 2>&1; then echo "$base"; return 0; fi
  done
  for d in "$base"/*; do
    [ -d "$d" ] || continue
    find_dump_root "$d"; return 0
  done
  echo "$base"
}}

pick_source_db() {{
  local root="$1"
  if [ -n "$HINT" ]; then echo "$HINT"; return 0; fi
  for d in "$root"/*; do
    [ -d "$d" ] || continue
    if compgen -G "$d/*.bson" >/dev/null 2>&1; then basename "$d"; return 0; fi
  done
  echo "$TARGET"
}}

MAGIC=$(head -c 2 "$ARCHIVE" | od -An -tx1 | tr -d ' \\n')
DROP_ARGS=""
[ "$MODE" = "replace" ] && DROP_ARGS="--drop"

if [ "$MAGIC" = "504b" ]; then
  EXTRACT="$WORK/extract"
  mkdir -p "$EXTRACT"
  "$UNZIP" -q -o "$ARCHIVE" -d "$EXTRACT"
  ROOT=$(find_dump_root "$EXTRACT")
  SOURCE=$(pick_source_db "$ROOT")
  if compgen -G "$ROOT"/*/ >/dev/null 2>&1; then
    $MR {restore_auth} $DROP_ARGS --dir "$ROOT" --nsFrom "${{SOURCE}}.*" --nsTo "${{TARGET}}.*"
  else
    $MR {restore_auth} $DROP_ARGS --db "$TARGET" --dir "$ROOT"
  fi
  echo RESTORE_OK zip
elif [ "$MAGIC" = "1f8b" ]; then
  SOURCE="$HINT"
  [ -z "$SOURCE" ] && SOURCE="$TARGET"
  if [ "$SOURCE" != "$TARGET" ]; then
    $MR {restore_auth} $DROP_ARGS --gzip --archive="$ARCHIVE" --nsFrom "${{SOURCE}}.*" --nsTo "${{TARGET}}.*"
  else
    $MR {restore_auth} $DROP_ARGS --gzip --archive="$ARCHIVE"
  fi
  echo RESTORE_OK gzip
else
  EXTRACT="$WORK/extract_raw"
  mkdir -p "$EXTRACT"
  if "$UNZIP" -q -o "$ARCHIVE" -d "$EXTRACT" 2>/dev/null; then
    ROOT=$(find_dump_root "$EXTRACT")
    SOURCE=$(pick_source_db "$ROOT")
    if compgen -G "$ROOT"/*/ >/dev/null 2>&1; then
      $MR {restore_auth} $DROP_ARGS --dir "$ROOT" --nsFrom "${{SOURCE}}.*" --nsTo "${{TARGET}}.*"
    else
      $MR {restore_auth} $DROP_ARGS --db "$TARGET" --dir "$ROOT"
    fi
    echo RESTORE_OK zip-raw
  else
    SOURCE="$HINT"
    [ -z "$SOURCE" ] && SOURCE="$TARGET"
    if [ "$SOURCE" != "$TARGET" ]; then
      $MR {restore_auth} $DROP_ARGS --gzip --archive="$ARCHIVE" --nsFrom "${{SOURCE}}.*" --nsTo "${{TARGET}}.*"
    else
      $MR {restore_auth} $DROP_ARGS --gzip --archive="$ARCHIVE"
    fi
    echo RESTORE_OK gzip-raw
  fi
fi
"#,
        arch_q = sh_quote(&remote_file),
        db_q = sh_quote(db),
        mode_q = sh_quote(mode),
        hint_q = sh_quote(hint),
        mr_q = sh_quote(&tools.mongorestore),
        unzip_q = sh_quote(unzip),
        restore_auth = restore_auth
    );
    let res = exec_command(mgr, session_id, &restore_script, 60 * 60 * 1000).await?;
    if res.code != 0 || !res.stdout.contains("RESTORE_OK") {
        return Err(if !res.stderr.is_empty() {
            res.stderr
        } else {
            res.stdout
        });
    }
    let format = res
        .stdout
        .lines()
        .find(|l| l.contains("RESTORE_OK"))
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("unknown");
    emit("done", &format!("已恢复到远程 {}", db), 100);
    Ok(serde_json::json!({
        "ok": true,
        "db": db,
        "mode": mode,
        "format": format,
        "connectionLabel": mongo_auth.label
    }))
}
