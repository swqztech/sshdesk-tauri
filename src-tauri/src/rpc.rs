use crate::events;
use crate::host_metrics::{fetch_host_pm2, fetch_host_stats};
use crate::hosts::{self, HostInput};
use crate::local_backups::{list_pc_mongo_backups, list_pc_qdrant_backups};
use crate::mongo::backup_meta::{read_backup_meta_from_zip, LocalBackupRef};
use crate::mongo::{
    backup_remote_database, default_backup_config, default_local_mongo_conn, delete_local_backup,
    download_backup_from_oss, incremental_backup_remote_database, list_local_backups,
    list_oss_backups, list_remote_databases, load_remote_backup_chain, load_remote_backup_config,
    remove_oss_backup, restore_local_backup_auto_chain, restore_local_database_from_file,
    save_remote_backup_config, upload_server_backup_to_oss, BackupConfig, BackupProgress, MongoConn,
    MongoLocalConn, MongoLocalRestoreProgress,
};
use crate::persist_state::{
    load_file_browser_state, load_terminal_state, save_file_browser_state, save_terminal_state,
};
use crate::platform::downloads_dir;
use crate::qdrant::{
    backup_remote_qdrant, default_qdrant_config, delete_local_qdrant_backup,
    detect_local_qdrant_exe, download_qdrant_from_oss, list_local_qdrant_backups,
    list_oss_qdrant, list_qdrant_collections, load_qdrant_local_settings,
    load_remote_qdrant_config, peek_qdrant_backup_meta, probe_local_qdrant_status,
    remove_oss_qdrant_backup, resolve_local_qdrant_data_dir, resolve_local_qdrant_exe,
    restore_qdrant_from_local_archive, save_qdrant_local_settings, save_remote_qdrant_config,
    start_local_qdrant_service, stop_local_qdrant_service, upload_server_qdrant_to_oss,
    QdrantConfig, QdrantLocalSettings, QdrantProgress,
};
use crate::sessions::SessionManager;
use crate::ssh_remote::{
    copy_remote_paths, download_remote_file, get_remote_home, list_remote_dir, move_remote_paths,
    resolve_remote_path, stat_remote_dir_size,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use tauri::State;

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    pub method: String,
    pub args: Vec<Value>,
}

fn str_arg(args: &[Value], i: usize) -> Result<String, String> {
    args.get(i)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("缺少参数 #{}", i + 1))
}

fn parse_host_input(v: &Value) -> Result<HostInput, String> {
    serde_json::from_value(v.clone()).map_err(|e| e.to_string())
}

fn parse_arg<T: serde::de::DeserializeOwned>(args: &[Value], i: usize) -> Result<T, String> {
    args.get(i)
        .ok_or_else(|| format!("缺少参数 #{}", i + 1))
        .and_then(|v| serde_json::from_value(v.clone()).map_err(|e| e.to_string()))
}

fn default_dl_dir() -> String {
    downloads_dir().to_string_lossy().to_string()
}

pub async fn dispatch(
    mgr: &SessionManager,
    method: &str,
    args: &[Value],
) -> Result<Value, String> {
    match method {
        "hosts:list" => Ok(serde_json::to_value(hosts::list_hosts()).map_err(|e| e.to_string())?),
        "hosts:create" => {
            let input = parse_host_input(args.first().ok_or("缺少主机参数")?)?;
            Ok(serde_json::to_value(hosts::create_host(input)?).map_err(|e| e.to_string())?)
        }
        "hosts:update" => {
            let id = str_arg(args, 0)?;
            let input = parse_host_input(args.get(1).ok_or("缺少主机参数")?)?;
            Ok(serde_json::to_value(hosts::update_host(&id, input)?).map_err(|e| e.to_string())?)
        }
        "hosts:delete" => Ok(Value::Bool(hosts::delete_host(&str_arg(args, 0)?)?)),

        "ssh:list" => Ok(serde_json::to_value(mgr.list()).map_err(|e| e.to_string())?),
        "ssh:connect" => {
            let host_id = str_arg(args, 0)?;
            let opts = args.get(1);
            let initial_command = opts
                .and_then(|o| o.get("initialCommand"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let initial_cwd = opts
                .and_then(|o| o.get("initialCwd"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            mgr.connect(&host_id, initial_command, initial_cwd).await
        }
        "ssh:openShell" => {
            let connection_id = str_arg(args, 0)?;
            let initial_command = args.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
            let initial_cwd = args.get(2).and_then(|v| v.as_str()).map(|s| s.to_string());
            mgr.open_shell(&connection_id, initial_command, initial_cwd)
                .await
        }
        "ssh:write" => Ok(Value::Bool(
            mgr.write_shell(&str_arg(args, 0)?, &str_arg(args, 1)?).await?,
        )),
        "ssh:resize" => {
            let cols = args.get(1).and_then(|v| v.as_u64()).unwrap_or(80) as u32;
            let rows = args.get(2).and_then(|v| v.as_u64()).unwrap_or(24) as u32;
            Ok(Value::Bool(
                mgr.resize_shell(&str_arg(args, 0)?, cols, rows).await?,
            ))
        }
        "ssh:closeShell" => Ok(Value::Bool(mgr.close_shell(&str_arg(args, 0)?).await?)),
        "ssh:closeConnection" => {
            Ok(Value::Bool(mgr.close_connection(&str_arg(args, 0)?).await?))
        }
        "ssh:cdShell" => Ok(Value::Bool(
            mgr.cd_shell(&str_arg(args, 0)?, &str_arg(args, 1)?).await?,
        )),
        "ssh:runShell" => Ok(Value::Bool(
            mgr.run_shell_command(&str_arg(args, 0)?, &str_arg(args, 1)?).await?,
        )),

        "files:loadState" => {
            let host_id = str_arg(args, 0)?;
            let session_id = args.get(1).and_then(|v| v.as_str());
            let mut home = "/".to_string();
            if let Some(sid) = session_id {
                if let Ok(h) = get_remote_home(mgr, sid).await {
                    home = h;
                }
            }
            let state = load_file_browser_state(&host_id, &home);
            if let Some(sid) = session_id {
                if let Some(tabs) = state.get("tabs").and_then(|t| t.as_array()) {
                    if !tabs.is_empty() {
                        let mut new_tabs = Vec::new();
                        for tab in tabs {
                            let path = tab
                                .get("path")
                                .and_then(|p| p.as_str())
                                .unwrap_or("/");
                            let resolved = resolve_remote_path(mgr, sid, path)
                                .await
                                .unwrap_or_else(|_| home.clone());
                            let mut t = tab.clone();
                            if let Some(obj) = t.as_object_mut() {
                                obj.insert("path".into(), Value::String(resolved));
                            }
                            new_tabs.push(t);
                        }
                        let mut st = state;
                        if let Some(obj) = st.as_object_mut() {
                            obj.insert("tabs".into(), Value::Array(new_tabs));
                        }
                        return Ok(st);
                    }
                }
            }
            Ok(state)
        }
        "files:saveState" => {
            save_file_browser_state(&str_arg(args, 0)?, args.get(1).cloned().unwrap_or(Value::Null))?;
            Ok(Value::Bool(true))
        }
        "terminal:loadState" => Ok(load_terminal_state(&str_arg(args, 0)?)),
        "terminal:saveState" => {
            save_terminal_state(&str_arg(args, 0)?, args.get(1).cloned().unwrap_or(Value::Null))?;
            Ok(Value::Bool(true))
        }
        "files:listDir" => {
            list_remote_dir(mgr, &str_arg(args, 0)?, &str_arg(args, 1)?).await
        }
        "files:dirSize" => {
            stat_remote_dir_size(mgr, &str_arg(args, 0)?, &str_arg(args, 1)?).await
        }
        "files:download" => {
            let session_id = str_arg(args, 0)?;
            let remote = str_arg(args, 1)?;
            let local = str_arg(args, 2)?;
            download_remote_file(&mgr.app_handle(), mgr, &session_id, &remote, &local).await
        }
        "files:copy" => {
            let sources: Vec<String> = args
                .get(1)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            copy_remote_paths(mgr, &str_arg(args, 0)?, sources, &str_arg(args, 2)?).await
        }
        "files:move" => {
            let sources: Vec<String> = args
                .get(1)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            move_remote_paths(mgr, &str_arg(args, 0)?, sources, &str_arg(args, 2)?).await
        }

        "host:stats" => fetch_host_stats(mgr, &str_arg(args, 0)?).await,
        "host:pm2" => fetch_host_pm2(mgr, &str_arg(args, 0)?).await,

        "mongo:defaultDownloadDir" | "qdrant:defaultDownloadDir" => {
            Ok(Value::String(default_dl_dir()))
        }

        "mongo:listDatabases" => {
            let sid = str_arg(args, 0)?;
            let conn = args.get(1).and_then(|v| serde_json::from_value::<MongoConn>(v.clone()).ok());
            let cfg = args
                .get(2)
                .and_then(|v| serde_json::from_value::<BackupConfig>(v.clone()).ok());
            Ok(serde_json::to_value(list_remote_databases(mgr, &sid, conn, cfg).await?)
                .map_err(|e| e.to_string())?)
        }
        "mongo:loadBackupConfig" => {
            Ok(serde_json::to_value(load_remote_backup_config(mgr, &str_arg(args, 0)?).await?)
                .map_err(|e| e.to_string())?)
        }
        "mongo:saveBackupConfig" => {
            let sid = str_arg(args, 0)?;
            let cfg = parse_arg::<BackupConfig>(args, 1)?;
            Ok(serde_json::to_value(save_remote_backup_config(mgr, &sid, cfg).await?)
                .map_err(|e| e.to_string())?)
        }
        "mongo:defaultBackupConfig" => {
            Ok(serde_json::to_value(default_backup_config()).map_err(|e| e.to_string())?)
        }
        "mongo:listPcBackups" => {
            let dir = args
                .get(0)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(default_dl_dir);
            Ok(serde_json::to_value(list_pc_mongo_backups(&dir)).map_err(|e| e.to_string())?)
        }
        "mongo:defaultLocalConn" => {
            Ok(serde_json::to_value(default_local_mongo_conn()).map_err(|e| e.to_string())?)
        }
        "mongo:uploadServerToOss" => {
            let sid = str_arg(args, 0)?;
            let remote = str_arg(args, 1)?;
            let cfg = parse_arg::<BackupConfig>(args, 2)?;
            let db_name = Path::new(&remote)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "backup".into());
            let app = mgr.app_handle();
            let app_cb = app.clone();
            let sid_cb = sid.clone();
            Ok(upload_server_backup_to_oss(
                &app,
                mgr,
                &sid,
                &remote,
                cfg,
                Some(Box::new(move |p: BackupProgress| {
                    events::emit(
                        &app_cb,
                        "mongo:backupProgress",
                        serde_json::json!({
                            "sessionId": sid_cb,
                            "dbName": db_name,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await?)
        }
        "mongo:listLocalBackups" => {
            let sid = str_arg(args, 0)?;
            let cfg = parse_arg::<BackupConfig>(args, 1)?;
            Ok(serde_json::to_value(list_local_backups(mgr, &sid, &cfg).await?)
                .map_err(|e| e.to_string())?)
        }
        "mongo:listOssBackups" => {
            let cfg = parse_arg::<BackupConfig>(args, 0)?;
            Ok(serde_json::to_value(list_oss_backups(&cfg).await?).map_err(|e| e.to_string())?)
        }
        "mongo:deleteLocalBackup" => {
            delete_local_backup(mgr, &str_arg(args, 0)?, &str_arg(args, 1)?).await?;
            Ok(Value::Bool(true))
        }
        "mongo:deleteOssBackup" => {
            let cfg = parse_arg::<BackupConfig>(args, 0)?;
            remove_oss_backup(&cfg, &str_arg(args, 1)?).await?;
            Ok(Value::Bool(true))
        }
        "mongo:backup" => {
            let sid = str_arg(args, 0)?;
            let db_name = str_arg(args, 1)?;
            let conn = args.get(2).and_then(|v| serde_json::from_value::<MongoConn>(v.clone()).ok());
            let cfg = parse_arg::<BackupConfig>(args, 3)?;
            let app = mgr.app_handle();
            let app_cb = app.clone();
            let sid_cb = sid.clone();
            let name = db_name.clone();
            backup_remote_database(
                &app,
                mgr,
                &sid,
                &db_name,
                conn,
                cfg,
                Some(Box::new(move |p: BackupProgress| {
                    events::emit(
                        &app_cb,
                        "mongo:backupProgress",
                        serde_json::json!({
                            "sessionId": sid_cb,
                            "dbName": name,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await
        }
        "mongo:backupIncremental" => {
            let sid = str_arg(args, 0)?;
            let db_name = str_arg(args, 1)?;
            let conn = args.get(2).and_then(|v| serde_json::from_value::<MongoConn>(v.clone()).ok());
            let cfg = parse_arg::<BackupConfig>(args, 3)?;
            let app = mgr.app_handle();
            let app_cb = app.clone();
            let sid_cb = sid.clone();
            let name = db_name.clone();
            incremental_backup_remote_database(
                &app,
                mgr,
                &sid,
                &db_name,
                conn,
                cfg,
                Some(Box::new(move |p: BackupProgress| {
                    events::emit(
                        &app_cb,
                        "mongo:backupProgress",
                        serde_json::json!({
                            "sessionId": sid_cb,
                            "dbName": name,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await
        }
        "mongo:loadBackupChain" => {
            let sid = str_arg(args, 0)?;
            let db = str_arg(args, 1)?;
            let cfg = args
                .get(2)
                .and_then(|v| serde_json::from_value::<BackupConfig>(v.clone()).ok());
            Ok(serde_json::to_value(load_remote_backup_chain(mgr, &sid, &db, cfg).await?)
                .map_err(|e| e.to_string())?)
        }
        "mongo:readBackupMeta" => {
            let meta = read_backup_meta_from_zip(&str_arg(args, 0)?);
            Ok(serde_json::to_value(meta).map_err(|e| e.to_string())?)
        }
        "mongo:restoreFromLocal" => {
            let sid = str_arg(args, 0)?;
            let local_path = str_arg(args, 1)?;
            let target_db = str_arg(args, 2)?;
            let mode = str_arg(args, 3)?;
            let local_conn = args
                .get(4)
                .and_then(|v| serde_json::from_value::<MongoLocalConn>(v.clone()).ok())
                .unwrap_or_else(default_local_mongo_conn);
            let source_hint = args.get(5).and_then(|v| v.as_str());
            let siblings: Option<Vec<LocalBackupRef>> = args
                .get(6)
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let app = mgr.app_handle();
            let db_name = target_db.clone();
            let on_progress = Some(Box::new(move |p: MongoLocalRestoreProgress| {
                events::emit(
                    &app,
                    "mongo:backupProgress",
                    serde_json::json!({
                        "sessionId": sid,
                        "dbName": db_name,
                        "phase": p.phase,
                        "message": p.message,
                        "percent": p.percent
                    }),
                );
            }) as Box<dyn Fn(MongoLocalRestoreProgress) + Send + Sync>);
            if let Some(sibs) = siblings {
                if !sibs.is_empty() {
                    return restore_local_backup_auto_chain(
                        &local_path,
                        &sibs,
                        &target_db,
                        &mode,
                        Some(&local_conn),
                        source_hint,
                        on_progress,
                    )
                    .await;
                }
            }
            restore_local_database_from_file(
                &local_path,
                &target_db,
                &mode,
                Some(&local_conn),
                source_hint,
                on_progress,
            )
            .await
        }
        "mongo:downloadOss" => {
            let sid = str_arg(args, 0)?;
            let cfg = parse_arg::<BackupConfig>(args, 1)?;
            let key = str_arg(args, 2)?;
            let dir = args
                .get(3)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(default_dl_dir);
            let db_name = Path::new(&key)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "backup".into());
            let app = mgr.app_handle();
            let app_cb = app.clone();
            let sid_cb = sid.clone();
            download_backup_from_oss(
                &cfg,
                &key,
                &dir,
                Some(Box::new(move |p: BackupProgress| {
                    events::emit(
                        &app_cb,
                        "mongo:backupProgress",
                        serde_json::json!({
                            "sessionId": sid_cb,
                            "dbName": db_name,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await
        }

        "qdrant:loadConfig" => {
            Ok(serde_json::to_value(load_remote_qdrant_config(mgr, &str_arg(args, 0)?).await?)
                .map_err(|e| e.to_string())?)
        }
        "qdrant:saveConfig" => {
            let sid = str_arg(args, 0)?;
            let cfg = parse_arg::<QdrantConfig>(args, 1)?;
            Ok(serde_json::to_value(save_remote_qdrant_config(mgr, &sid, cfg).await?)
                .map_err(|e| e.to_string())?)
        }
        "qdrant:defaultConfig" => {
            Ok(serde_json::to_value(default_qdrant_config()).map_err(|e| e.to_string())?)
        }
        "qdrant:loadLocalSettings" => {
            Ok(serde_json::to_value(load_qdrant_local_settings()).map_err(|e| e.to_string())?)
        }
        "qdrant:saveLocalSettings" => {
            let input = parse_arg::<QdrantLocalSettings>(args, 0)?;
            Ok(serde_json::to_value(save_qdrant_local_settings(input)).map_err(|e| e.to_string())?)
        }
        "qdrant:detectLocalQdrantExe" => Ok(Value::String(
            detect_local_qdrant_exe().unwrap_or_default(),
        )),
        "qdrant:listPcBackups" => {
            let dir = args
                .get(0)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(default_dl_dir);
            Ok(serde_json::to_value(list_pc_qdrant_backups(&dir)).map_err(|e| e.to_string())?)
        }
        "qdrant:peekBackupMeta" => {
            Ok(serde_json::to_value(peek_qdrant_backup_meta(
                &args.get(0).and_then(|v| v.as_str()).unwrap_or(""),
            ))
            .map_err(|e| e.to_string())?)
        }
        "qdrant:probeLocalStatus" => {
            let local = load_qdrant_local_settings();
            let collection = args
                .get(0)
                .and_then(|v| v.as_str())
                .unwrap_or("gallery_assets");
            Ok(serde_json::to_value(probe_local_qdrant_status(Some(collection)).await)
            .map_err(|e| e.to_string())?)
        }
        "qdrant:startLocal" => {
            let local = load_qdrant_local_settings();
            let collection = args
                .get(0)
                .and_then(|v| v.as_str())
                .unwrap_or("gallery_assets");
            let exe = resolve_local_qdrant_exe(Some(&local)).unwrap_or_else(|_| local.qdrant_exe.clone());
            Ok(serde_json::to_value(start_local_qdrant_service(Some(collection)).await?)
            .map_err(|e| e.to_string())?)
        }
        "qdrant:stopLocal" => {
            let local = load_qdrant_local_settings();
            Ok(serde_json::to_value(stop_local_qdrant_service().await)
            .map_err(|e| e.to_string())?)
        }
        "qdrant:restoreFromLocal" => {
            let sid = str_arg(args, 0)?;
            let local_path = str_arg(args, 1)?;
            let cfg = parse_arg::<QdrantConfig>(args, 2)?;
            let explicit = args.get(3).and_then(|v| v.as_str()).unwrap_or("").trim();
            let local = load_qdrant_local_settings();
            let progress_label = if !explicit.is_empty() {
                explicit.to_string()
            } else {
                peek_qdrant_backup_meta(&local_path)
                    .collection
                    .unwrap_or_else(|| cfg.qdrant_collection.clone())
            };
            let exe = resolve_local_qdrant_exe(Some(&local))?;
            let data_dir = resolve_local_qdrant_data_dir(Some(&local));
            let app = mgr.app_handle();
            let app_cb = app.clone();
            let label = progress_label.clone();
            let sid_emit = sid.clone();
            restore_qdrant_from_local_archive(
                &local_path,
                if explicit.is_empty() {
                    None
                } else {
                    Some(explicit)
                },
                Some(exe.as_str()),
                Some(data_dir.as_str()),
                Some(Box::new(move |p: QdrantProgress| {
                    events::emit(
                        &app_cb,
                        "qdrant:backupProgress",
                        serde_json::json!({
                            "sessionId": sid_emit,
                            "collection": label,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await
        }
        "qdrant:uploadServerToOss" => {
            let sid = str_arg(args, 0)?;
            let remote = str_arg(args, 1)?;
            let cfg = parse_arg::<QdrantConfig>(args, 2)?;
            let collection = Path::new(&remote)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "backup".into());
            let app = mgr.app_handle();
            let app_cb = app.clone();
            let sid_cb = sid.clone();
            upload_server_qdrant_to_oss(
                &app,
                mgr,
                &sid,
                &remote,
                &cfg,
                Some(Box::new(move |p: QdrantProgress| {
                    events::emit(
                        &app_cb,
                        "qdrant:backupProgress",
                        serde_json::json!({
                            "sessionId": sid_cb,
                            "collection": collection,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await
        }
        "qdrant:listCollections" => {
            let sid = str_arg(args, 0)?;
            let cfg = parse_arg::<QdrantConfig>(args, 1)?;
            Ok(serde_json::to_value(list_qdrant_collections(mgr, &sid, &cfg).await?)
                .map_err(|e| e.to_string())?)
        }
        "qdrant:listLocalBackups" => {
            let sid = str_arg(args, 0)?;
            let cfg = parse_arg::<QdrantConfig>(args, 1)?;
            Ok(serde_json::to_value(list_local_qdrant_backups(mgr, &sid, &cfg).await?)
                .map_err(|e| e.to_string())?)
        }
        "qdrant:listOssBackups" => {
            let cfg = parse_arg::<QdrantConfig>(args, 0)?;
            Ok(serde_json::to_value(list_oss_qdrant(&cfg).await?).map_err(|e| e.to_string())?)
        }
        "qdrant:deleteLocalBackup" => {
            delete_local_qdrant_backup(mgr, &str_arg(args, 0)?, &str_arg(args, 1)?).await?;
            Ok(Value::Bool(true))
        }
        "qdrant:deleteOssBackup" => {
            let cfg = parse_arg::<QdrantConfig>(args, 0)?;
            remove_oss_qdrant_backup(&cfg, &str_arg(args, 1)?).await?;
            Ok(Value::Bool(true))
        }
        "qdrant:backup" => {
            let sid = str_arg(args, 0)?;
            let collection = str_arg(args, 1)?;
            let cfg = parse_arg::<QdrantConfig>(args, 2)?;
            let app = mgr.app_handle();
            let app_cb = app.clone();
            let sid_cb = sid.clone();
            let col = collection.clone();
            backup_remote_qdrant(
                &app,
                mgr,
                &sid,
                &collection,
                &cfg,
                Some(Box::new(move |p: QdrantProgress| {
                    events::emit(
                        &app_cb,
                        "qdrant:backupProgress",
                        serde_json::json!({
                            "sessionId": sid_cb,
                            "collection": col,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await
        }
        "qdrant:downloadOss" => {
            let sid = str_arg(args, 0)?;
            let cfg = parse_arg::<QdrantConfig>(args, 1)?;
            let key = str_arg(args, 2)?;
            let dir = args
                .get(3)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(default_dl_dir);
            let collection = Path::new(&key)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "backup".into());
            let app = mgr.app_handle();
            download_qdrant_from_oss(
                &cfg,
                &key,
                &dir,
                Some(Box::new(move |p: QdrantProgress| {
                    events::emit(
                        &app,
                        "qdrant:backupProgress",
                        serde_json::json!({
                            "sessionId": sid,
                            "collection": collection,
                            "phase": p.phase,
                            "message": p.message,
                            "percent": p.percent
                        }),
                    );
                })),
            )
            .await
        }

        other => Err(format!("未知 RPC: {}", other)),
    }
}

#[tauri::command]
pub async fn sshdesk_rpc(
    state: State<'_, SessionManager>,
    request: RpcRequest,
) -> Result<String, String> {
    let result = match dispatch(&state, &request.method, &request.args).await {
        Ok(v) => serde_json::json!({ "ok": true, "result": v }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    };
    serde_json::to_string(&result).map_err(|e| e.to_string())
}
