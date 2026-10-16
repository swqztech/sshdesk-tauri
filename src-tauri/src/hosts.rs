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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
