use crate::events::emit;
use crate::hosts::{get_host_secret, AuthType};
use crate::persist_state::SessionInfo;
use crate::ssh_remote::sh_quote;
use async_trait::async_trait;
use parking_lot::Mutex;
use russh::client::{self, Config, Handler};
use russh::{Channel, ChannelMsg, Disconnect};
use russh_keys::key;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;

enum ShellIoCmd {
    Data(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Shutdown,
}

#[derive(Clone)]
pub(crate) struct SshHandler;

#[async_trait]
impl Handler for SshHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

struct Shell {
    id: String,
    connection_id: String,
    /// 写入 PTY 的队列；读循环在独立任务中持有 Channel，避免锁内 await 阻塞键盘输入
    io_tx: AsyncMutex<Option<UnboundedSender<ShellIoCmd>>>,
    cols: Mutex<u32>,
    rows: Mutex<u32>,
}

pub type SshClientHandle = client::Handle<SshHandler>;

pub struct Connection {
    pub id: String,
    pub host_id: String,
    pub title: String,
    pub handle: Arc<SshClientHandle>,
    shells: Mutex<HashMap<String, Arc<Shell>>>,
    pub exec_lock: AsyncMutex<()>,
}

pub struct SessionManager {
    connections: Mutex<HashMap<String, Arc<Connection>>>,
    shells: Mutex<HashMap<String, Arc<Shell>>>,
    app: AppHandle,
}

impl SessionManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            shells: Mutex::new(HashMap::new()),
            app,
        }
    }

    pub fn app_handle(&self) -> AppHandle {
        self.app.clone()
    }

    fn uid(prefix: &str) -> String {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let r = uuid::Uuid::new_v4().simple().to_string();
        format!("{}-{:x}-{}", prefix, t, &r[..4])
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        self.connections
            .lock()
            .values()
            .map(|c| SessionInfo {
                id: c.id.clone(),
                host_id: c.host_id.clone(),
                title: c.title.clone(),
                connected: true,
            })
            .collect()
    }

    async fn connect_client(host_id: &str) -> Result<(Arc<SshClientHandle>, String), String> {
        let secret = get_host_secret(host_id).ok_or_else(|| "主机不存在".to_string())?;
        let h = &secret.record;
        let title = if h.name.trim().is_empty() {
            format!("{}@{}", h.username, h.host)
        } else {
            h.name.trim().to_string()
        };

        let config = Arc::new(Config {
            inactivity_timeout: Some(Duration::from_secs(3600)),
            ..Default::default()
        });
        let addr = (h.host.as_str(), h.port);
        let mut session =
            client::connect(config, addr, SshHandler).await.map_err(|e| e.to_string())?;

        let auth_ok = match h.auth_type {
            AuthType::Password => {
                if secret.password.is_empty() {
                    return Err("未设置密码".into());
                }
                session
                    .authenticate_password(&h.username, &secret.password)
                    .await
                    .map_err(|e| e.to_string())?
            }
            AuthType::PrivateKey => {
                let key_path = h
                    .private_key_path
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "未配置私钥路径".to_string())?;
                if !Path::new(key_path).exists() {
                    return Err("私钥文件不存在，请检查路径".into());
                }
                let pass = if secret.passphrase.is_empty() {
                    None
                } else {
                    Some(secret.passphrase.as_str())
                };
                let key_pair = russh_keys::load_secret_key(key_path, pass)
                    .map_err(|e| format!("读取私钥失败: {}", e))?;
                session
                    .authenticate_publickey(&h.username, Arc::new(key_pair))
                    .await
                    .map_err(|e| e.to_string())?
            }
        };
        if !auth_ok {
            return Err("SSH 认证未成功".into());
        }
        Ok((Arc::new(session), title))
    }

    fn spawn_shell_io_loop(
        app: AppHandle,
        shell_id: String,
        mut channel: Channel<client::Msg>,
        mut cmd_rx: UnboundedReceiver<ShellIoCmd>,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { data }) => {
                                let text = String::from_utf8_lossy(&data).to_string();
                                emit(
                                    &app,
                                    "ssh:data",
                                    serde_json::json!({ "sessionId": shell_id, "data": text }),
                                );
                            }
                            Some(ChannelMsg::ExitStatus { .. }) | Some(ChannelMsg::Close) | None => {
                                break;
                            }
                            _ => {}
                        }
                    }
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(ShellIoCmd::Data(bytes)) => {
                                if channel.data(bytes.as_ref()).await.is_err() {
                                    break;
                                }
                            }
                            Some(ShellIoCmd::Resize { cols, rows }) => {
                                let _ = channel.window_change(cols, rows, 0, 0).await;
                            }
                            Some(ShellIoCmd::Shutdown) | None => break,
                        }
                    }
                }
            }
            let _ = channel.eof().await;
            let _ = channel.close().await;
            emit(
                &app,
                "ssh:closed",
                serde_json::json!({ "sessionId": shell_id }),
            );
        });
    }

    async fn open_shell_on_connection(
        &self,
        connection: &Arc<Connection>,
        shell: &Arc<Shell>,
    ) -> Result<(), String> {
        let mut channel = connection
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("无法打开 shell: {}", e))?;
        let cols = *shell.cols.lock();
        let rows = *shell.rows.lock();
        channel
            .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .map_err(|e| e.to_string())?;
        channel.request_shell(true).await.map_err(|e| e.to_string())?;
        let (io_tx, io_rx) = unbounded_channel();
        *shell.io_tx.lock().await = Some(io_tx);
        Self::spawn_shell_io_loop(self.app.clone(), shell.id.clone(), channel, io_rx);
        Ok(())
    }

    fn queue_shell_command(shell: Arc<Shell>, command: String) {
        let cmd = command.trim().to_string();
        if cmd.is_empty() {
            return;
        }
        tokio::spawn(async move {
            for _ in 0..50 {
                if shell.io_tx.lock().await.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
            let data = format!("{}\n", cmd);
            if let Some(tx) = shell.io_tx.lock().await.as_ref() {
                let _ = tx.send(ShellIoCmd::Data(data.into_bytes()));
            }
        });
    }

    pub async fn connect(
        &self,
        host_id: &str,
        initial_command: Option<String>,
        initial_cwd: Option<String>,
    ) -> Result<serde_json::Value, String> {
        let (handle, title) = Self::connect_client(host_id).await?;
        let connection_id = Self::uid("c");
        let shell_id = Self::uid("sh");

        let connection = Arc::new(Connection {
            id: connection_id.clone(),
            host_id: host_id.to_string(),
            title: title.clone(),
            handle,
            shells: Mutex::new(HashMap::new()),
            exec_lock: AsyncMutex::new(()),
        });

        let shell = Arc::new(Shell {
            id: shell_id.clone(),
            connection_id: connection_id.clone(),
            io_tx: AsyncMutex::new(None),
            cols: Mutex::new(120),
            rows: Mutex::new(36),
        });

        connection
            .shells
            .lock()
            .insert(shell_id.clone(), shell.clone());
        self.connections
            .lock()
            .insert(connection_id.clone(), connection.clone());
        self.shells.lock().insert(shell_id.clone(), shell.clone());

        self.open_shell_on_connection(&connection, &shell).await?;

        if let Some(cmd) = initial_command.filter(|s| !s.trim().is_empty()) {
            Self::queue_shell_command(shell.clone(), cmd);
        } else if let Some(cwd) = initial_cwd.filter(|s| !s.trim().is_empty()) {
            Self::queue_shell_command(shell.clone(), format!("cd {}", sh_quote(&cwd)));
        }

        Ok(serde_json::json!({
            "connectionId": connection_id,
            "shellId": shell_id,
            "title": title
        }))
    }

    pub fn connection_for_exec(&self, id: &str) -> Result<Arc<Connection>, String> {
        if let Some(sh) = self.shells.lock().get(id) {
            let cid = sh.connection_id.clone();
            if let Some(c) = self.connections.lock().get(&cid) {
                return Ok(c.clone());
            }
        }
        self.connections
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| "会话不存在或已断开".to_string())
    }

    fn require_shell(&self, shell_id: &str) -> Result<Arc<Shell>, String> {
        self.shells
            .lock()
            .get(shell_id)
            .cloned()
            .ok_or_else(|| "终端不存在或已关闭".to_string())
    }

    pub async fn open_shell(
        &self,
        connection_id: &str,
        initial_command: Option<String>,
        initial_cwd: Option<String>,
    ) -> Result<serde_json::Value, String> {
        let connection = self.connection_for_exec(connection_id)?;
        let shell_id = Self::uid("sh");
        let shell = Arc::new(Shell {
            id: shell_id.clone(),
            connection_id: connection.id.clone(),
            io_tx: AsyncMutex::new(None),
            cols: Mutex::new(120),
            rows: Mutex::new(36),
        });
        connection
            .shells
            .lock()
            .insert(shell_id.clone(), shell.clone());
        self.shells.lock().insert(shell_id.clone(), shell.clone());
        self.open_shell_on_connection(&connection, &shell).await?;
        if let Some(cmd) = initial_command.filter(|s| !s.trim().is_empty()) {
            Self::queue_shell_command(shell.clone(), cmd);
        } else if let Some(cwd) = initial_cwd.filter(|s| !s.trim().is_empty()) {
            Self::queue_shell_command(shell.clone(), format!("cd {}", sh_quote(&cwd)));
        }
        Ok(serde_json::json!({ "shellId": shell_id }))
    }

    pub async fn write_shell(&self, shell_id: &str, data: &str) -> Result<bool, String> {
        let shell = self.require_shell(shell_id)?;
        let guard = shell.io_tx.lock().await;
        let tx = guard
            .as_ref()
            .ok_or_else(|| "终端通道未就绪".to_string())?;
        tx.send(ShellIoCmd::Data(data.as_bytes().to_vec()))
            .map_err(|_| "终端已关闭".to_string())?;
        Ok(true)
    }

    pub async fn resize_shell(&self, shell_id: &str, cols: u32, rows: u32) -> Result<bool, String> {
        let shell = self.require_shell(shell_id)?;
        let cols = cols.max(20);
        let rows = rows.max(5);
        *shell.cols.lock() = cols;
        *shell.rows.lock() = rows;
        let guard = shell.io_tx.lock().await;
        let tx = guard
            .as_ref()
            .ok_or_else(|| "终端通道未就绪".to_string())?;
        tx.send(ShellIoCmd::Resize { cols, rows })
            .map_err(|_| "终端已关闭".to_string())?;
        Ok(true)
    }

    pub async fn close_shell(&self, shell_id: &str) -> Result<bool, String> {
        self.cleanup_shell(shell_id, true);
        Ok(true)
    }

    pub async fn close_connection(&self, connection_id: &str) -> Result<bool, String> {
        self.cleanup_connection(connection_id);
        Ok(true)
    }

    fn cleanup_shell(&self, shell_id: &str, end_stream: bool) {
        let shell = self.shells.lock().remove(shell_id);
        if let Some(shell) = shell {
            if end_stream {
                let s = Arc::clone(&shell);
                tokio::spawn(async move {
                    if let Some(tx) = s.io_tx.lock().await.take() {
                        let _ = tx.send(ShellIoCmd::Shutdown);
                    }
                });
            }
            if let Some(conn) = self.connections.lock().get(&shell.connection_id) {
                conn.shells.lock().remove(shell_id);
            }
        }
    }

    fn cleanup_connection(&self, connection_id: &str) {
        let conn = self.connections.lock().remove(connection_id);
        if let Some(conn) = conn {
            let ids: Vec<String> = conn.shells.lock().keys().cloned().collect();
            for id in ids {
                self.cleanup_shell(&id, true);
            }
            let handle = Arc::clone(&conn.handle);
            tokio::spawn(async move {
                let _ = handle
                    .disconnect(Disconnect::ByApplication, "", "")
                    .await;
            });
            emit(
                &self.app,
                "ssh:connectionClosed",
                serde_json::json!({ "connectionId": connection_id }),
            );
        }
    }

    pub async fn cd_shell(&self, shell_id: &str, remote_path: &str) -> Result<bool, String> {
        let p = remote_path.trim();
        if p.is_empty() {
            return Ok(false);
        }
        self.write_shell(shell_id, &format!("cd {}\n", sh_quote(p)))
            .await?;
        Ok(true)
    }

    pub async fn run_shell_command(&self, shell_id: &str, command: &str) -> Result<bool, String> {
        let cmd = command.trim();
        if cmd.is_empty() {
            return Ok(false);
        }
        self.write_shell(shell_id, &format!("{}\n", cmd)).await?;
        Ok(true)
    }

}
