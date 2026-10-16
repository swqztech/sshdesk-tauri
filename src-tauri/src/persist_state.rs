use crate::platform::user_data_dir;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn read_json_map(path: &PathBuf) -> serde_json::Map<String, Value> {
    if !path.exists() {
        return serde_json::Map::new();
    }
    let raw = fs::read_to_string(path).unwrap_or_default();
    serde_json::from_str::<serde_json::Map<String, Value>>(&raw).unwrap_or_default()
}

fn write_json_map(path: &PathBuf, map: &serde_json::Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, serde_json::to_string_pretty(map).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_terminal_state(host_id: &str) -> Value {
    let path = user_data_dir().join("terminal-state.json");
    let all = read_json_map(&path);
    all.get(host_id)
        .cloned()
        .unwrap_or_else(|| {
            let id = format!("t-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
            serde_json::json!({
                "tabs": [{ "id": id, "label": "终端 1" }],
                "activeTabId": id
            })
        })
}

pub fn save_terminal_state(host_id: &str, state: Value) -> Result<(), String> {
    let path = user_data_dir().join("terminal-state.json");
    let mut all = read_json_map(&path);
    all.insert(host_id.to_string(), state);
    write_json_map(&path, &all)
}

pub fn load_file_browser_state(host_id: &str, fallback_home: &str) -> Value {
    let path = user_data_dir().join("file-browser-state.json");
    let all = read_json_map(&path);
    if let Some(st) = all.get(host_id) {
        if st.get("tabs").and_then(|t| t.as_array()).map(|a| !a.is_empty()).unwrap_or(false) {
            return st.clone();
        }
    }
    let id = format!("t-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
    serde_json::json!({
        "tabs": [{ "id": id, "path": fallback_home }],
        "activeTabId": id
    })
}

pub fn save_file_browser_state(host_id: &str, state: Value) -> Result<(), String> {
    let path = user_data_dir().join("file-browser-state.json");
    let mut all = read_json_map(&path);
    all.insert(host_id.to_string(), state);
    write_json_map(&path, &all)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub host_id: String,
    pub title: String,
    pub connected: bool,
}
