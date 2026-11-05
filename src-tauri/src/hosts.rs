use crate::platform::{decrypt_secret, encrypt_secret, user_data_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRecord {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: AuthType,
    pub password: Option<String>,
    pub private_key_path: Option<String>,
    pub passphrase: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthType {
    #[serde(rename = "password")]
    Password,
    #[serde(rename = "privateKey")]
    PrivateKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInput {
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub auth_type: AuthType,
    pub password: Option<String>,
    pub private_key_path: Option<String>,
    pub passphrase: Option<String>,
    pub keep_password: Option<bool>,
    pub keep_passphrase: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: AuthType,
    pub private_key_path: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub has_password: bool,
    pub has_passphrase: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoreFile {
    version: u8,
    hosts: Vec<HostRecord>,
}

fn store_path() -> PathBuf {
    user_data_dir().join("hosts.json")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn uid() -> String {
    format!(
        "{}-{}",
        now_ms(),
        uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect::<String>()
    )
}

fn read_store() -> StoreFile {
    let path = store_path();
    if !path.exists() {
        return StoreFile {
            version: 1,
            hosts: vec![],
        };
    }
    let raw = fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str(&raw).unwrap_or(StoreFile {
        version: 1,
        hosts: vec![],
    })
}

fn write_store(store: &StoreFile) -> Result<(), String> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, serde_json::to_string_pretty(store).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn to_view(h: &HostRecord) -> HostView {
    HostView {
        id: h.id.clone(),
        name: h.name.clone(),
        host: h.host.clone(),
        port: h.port,
        username: h.username.clone(),
        auth_type: h.auth_type,
        private_key_path: h.private_key_path.clone(),
        created_at: h.created_at,
        updated_at: h.updated_at,
        has_password: h.password.is_some(),
        has_passphrase: h.passphrase.is_some(),
    }
}

pub fn list_hosts() -> Vec<HostView> {
    let mut hosts = read_store().hosts;
    hosts.sort_by(|a, b| a.name.cmp(&b.name));
    hosts.iter().map(to_view).collect()
}

pub struct HostSecret {
    pub record: HostRecord,
    pub password: String,
    pub passphrase: String,
}

pub fn get_host_secret(id: &str) -> Option<HostSecret> {
    let h = read_store().hosts.into_iter().find(|x| x.id == id)?;
    Some(HostSecret {
        record: h.clone(),
        password: decrypt_secret(h.password.as_deref()),
        passphrase: decrypt_secret(h.passphrase.as_deref()),
    })
}

pub fn create_host(input: HostInput) -> Result<HostView, String> {
    let now = now_ms();
    let record = HostRecord {
        id: uid(),
        name: input.name.trim().to_string(),
        host: input.host.trim().to_string(),
        port: if input.port.unwrap_or(0) > 0 {
            input.port.unwrap()
        } else {
            22
        },
        username: input.username.trim().to_string(),
        auth_type: input.auth_type,
        password: input
            .password
            .filter(|p| !p.is_empty())
            .map(|p| encrypt_secret(&p)),
        private_key_path: input
            .private_key_path
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        passphrase: input
            .passphrase
            .filter(|p| !p.is_empty())
            .map(|p| encrypt_secret(&p)),
        created_at: now,
        updated_at: now,
    };
    if record.name.is_empty() || record.host.is_empty() || record.username.is_empty() {
        return Err("名称、主机、用户名不能为空".into());
    }
    let mut store = read_store();
    store.hosts.push(record);
    write_store(&store)?;
    Ok(to_view(store.hosts.last().unwrap()))
}

pub fn update_host(id: &str, input: HostInput) -> Result<HostView, String> {
    let mut store = read_store();
    let idx = store.hosts.iter().position(|h| h.id == id);
    let idx = idx.ok_or_else(|| "主机不存在".to_string())?;
    let prev = store.hosts[idx].clone();
    let mut next = HostRecord {
        id: prev.id.clone(),
        name: input.name.trim().to_string(),
        host: input.host.trim().to_string(),
        port: if input.port.unwrap_or(0) > 0 {
            input.port.unwrap()
        } else {
            22
        },
        username: input.username.trim().to_string(),
        auth_type: input.auth_type,
        private_key_path: input
            .private_key_path
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        password: prev.password.clone(),
        passphrase: prev.passphrase.clone(),
        created_at: prev.created_at,
        updated_at: now_ms(),
    };
    if next.name.is_empty() || next.host.is_empty() || next.username.is_empty() {
        return Err("名称、主机、用户名不能为空".into());
    }
    if let Some(p) = input.password.filter(|p| !p.is_empty()) {
        next.password = Some(encrypt_secret(&p));
    } else if input.keep_password == Some(true) {
        next.password = prev.password;
    } else {
        next.password = None;
    }
    if let Some(p) = input.passphrase.filter(|p| !p.is_empty()) {
        next.passphrase = Some(encrypt_secret(&p));
    } else if input.keep_passphrase == Some(true) {
        next.passphrase = prev.passphrase;
    } else {
        next.passphrase = None;
    }
    store.hosts[idx] = next;
    write_store(&store)?;
    Ok(to_view(&store.hosts[idx]))
}

pub fn delete_host(id: &str) -> Result<bool, String> {
    let mut store = read_store();
    let before = store.hosts.len();
    store.hosts.retain(|h| h.id != id);
    if store.hosts.len() == before {
        return Ok(false);
    }
    write_store(&store)?;
    Ok(true)
}

/// 换电脑用的主机包。密码和私钥正文都在文件里，不依赖原来那台机器的路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortableHost {
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_type: AuthType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_key_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passphrase: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortableFile {
    kind: String,
    version: u8,
    exported_at: u64,
    hosts: Vec<PortableHost>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReport {
    pub path: String,
    pub count: usize,
    pub added: usize,
    pub updated: usize,
    pub names: Vec<String>,
    pub warnings: Vec<String>,
}

const PORTABLE_KIND: &str = "sshdesk-hosts";

fn nonempty(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn read_portable(path: &str) -> Result<PortableFile, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读不了这个文件: {}", e))?;
    if raw.len() > 20 * 1024 * 1024 {
        return Err("文件太大，不像连枢的主机导出".into());
    }
    let file: PortableFile = serde_json::from_str(&raw).map_err(|_| {
        "这不是连枢的主机导出文件".to_string()
    })?;
    if file.kind != PORTABLE_KIND || file.version != 1 {
        return Err("这不是连枢的主机导出文件".into());
    }
    if file.hosts.is_empty() {
        return Err("文件里没有主机".into());
    }
    Ok(file)
}

fn host_key(host: &str, port: u16, username: &str) -> String {
    format!(
        "{}|{}|{}",
        host.trim().to_ascii_lowercase(),
        port,
        username.trim()
    )
}

fn store_private_key(content: &str) -> Result<String, String> {
    let dir = user_data_dir().join("keys");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.pem", uid()));
    fs::write(&path, content).map_err(|e| format!("私钥写到本机失败: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

pub fn export_hosts(path: &str) -> Result<TransferReport, String> {
    let store = read_store();
    let mut warnings = Vec::new();
    let mut hosts = Vec::new();
    for record in &store.hosts {
        let password = nonempty(Some(decrypt_secret(record.password.as_deref())));
        let passphrase = nonempty(Some(decrypt_secret(record.passphrase.as_deref())));
        let mut private_key = None;
        if let Some(key_path) = record.private_key_path.as_deref() {
            match fs::read_to_string(key_path) {
                Ok(text) if !text.trim().is_empty() => private_key = Some(text),
                Ok(_) => warnings.push(format!("「{}」的私钥文件是空的", record.name)),
                Err(_) => warnings.push(format!(
                    "「{}」的私钥读不到（{}），新电脑上需要重新选文件",
                    record.name, key_path
                )),
            }
        }
        hosts.push(PortableHost {
            name: record.name.clone(),
            host: record.host.clone(),
            port: record.port,
            username: record.username.clone(),
            auth_type: record.auth_type,
            password,
            private_key_path: record.private_key_path.clone(),
            private_key,
            passphrase,
        });
    }
    let file = PortableFile {
        kind: PORTABLE_KIND.to_string(),
        version: 1,
        exported_at: now_ms(),
        hosts,
    };
    let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    if let Some(parent) = PathBuf::from(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    fs::write(path, text).map_err(|e| format!("导出失败: {}", e))?;
    let count = file.hosts.len();
    let names = file.hosts.into_iter().map(|h| h.name).collect();
    Ok(TransferReport {
        path: path.to_string(),
        count,
        added: count,
        updated: 0,
        names,
        warnings,
    })
}

pub fn preview_import(path: &str) -> Result<TransferReport, String> {
    let file = read_portable(path)?;
    let count = file.hosts.len();
    let names = file.hosts.into_iter().map(|h| h.name).collect();
    Ok(TransferReport {
        path: path.to_string(),
        count,
        added: 0,
        updated: 0,
        names,
        warnings: vec![],
    })
}

pub fn import_hosts(path: &str, replace: bool) -> Result<TransferReport, String> {
    let file = read_portable(path)?;
    let mut warnings = Vec::new();
    let mut incoming: Vec<HostRecord> = Vec::new();
    for item in file.hosts {
        let name = item.name.trim().to_string();
        let host = item.host.trim().to_string();
        let username = item.username.trim().to_string();
        if name.is_empty() || host.is_empty() || username.is_empty() {
            warnings.push("跳过一台缺少名称、主机或用户名的记录".into());
            continue;
        }
        let port = if item.port == 0 { 22 } else { item.port };
        let mut private_key_path = nonempty(item.private_key_path);
        if let Some(body) = nonempty(item.private_key) {
            match store_private_key(&body) {
                Ok(saved) => private_key_path = Some(saved),
                Err(err) => warnings.push(format!("「{}」{}", name, err)),
            }
        } else if item.auth_type == AuthType::PrivateKey {
            let missing = private_key_path
                .as_deref()
                .map(|p| !PathBuf::from(p).is_file())
                .unwrap_or(true);
            if missing {
                warnings.push(format!(
                    "「{}」没有带上私钥内容，导入后还要自己选一次私钥",
                    name
                ));
            }
        }
        let now = now_ms();
        incoming.push(HostRecord {
            id: uid(),
            name,
            host,
            port,
            username,
            auth_type: item.auth_type,
            password: nonempty(item.password).map(|p| encrypt_secret(&p)),
            private_key_path,
            passphrase: nonempty(item.passphrase).map(|p| encrypt_secret(&p)),
            created_at: now,
            updated_at: now,
        });
    }
    if incoming.is_empty() {
        return Err("没有可以导入的主机".into());
    }

    let mut store = if replace {
        StoreFile {
            version: 1,
            hosts: vec![],
        }
    } else {
        read_store()
    };
    let mut added = 0usize;
    let mut updated = 0usize;
    for record in incoming {
        if let Some(idx) = store.hosts.iter().position(|h| {
            host_key(&h.host, h.port, &h.username)
                == host_key(&record.host, record.port, &record.username)
        }) {
            let prev_id = store.hosts[idx].id.clone();
            let prev_created = store.hosts[idx].created_at;
            store.hosts[idx] = HostRecord {
                id: prev_id,
                created_at: prev_created,
                updated_at: now_ms(),
                ..record
            };
            updated += 1;
        } else {
            store.hosts.push(record);
            added += 1;
        }
    }
    let count = added + updated;
    let names: Vec<String> = store.hosts.iter().map(|h| h.name.clone()).collect();
    write_store(&store)?;
    Ok(TransferReport {
        path: path.to_string(),
        count,
        added,
        updated,
        names,
        warnings,
    })
}
