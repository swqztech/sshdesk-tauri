use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
struct DeskEvent {
    channel: String,
    payload: Value,
}

pub fn emit(app: &AppHandle, channel: &str, payload: impl Serialize) {
    let payload = serde_json::to_value(payload).unwrap_or(Value::Null);
    let _ = app.emit(
        "sshdesk-event",
        DeskEvent {
            channel: channel.to_string(),
            payload,
        },
    );
}

pub fn emit_json(app: &AppHandle, channel: &str, payload: Value) {
    let _ = app.emit(
        "sshdesk-event",
        DeskEvent {
            channel: channel.to_string(),
            payload,
        },
    );
}
