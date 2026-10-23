use crate::host::AppHandle;
use serde::Serialize;

/// A desktop notification for the UI to show.
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub title: &'static str,
    pub body: String,
}

/// Payload for the `comfyui-runtime` frontend event.
#[derive(Debug, Clone, Serialize)]
pub struct ComfyRuntimeEvent {
    pub phase: String,
    pub message: String,
}

pub fn emit_comfyui_runtime_event(app: &AppHandle, phase: &str, message: impl Into<String>) {
    let msg = message.into();
    if phase.ends_with("failed") {
        log::warn!("[comfyui {phase}] {msg}");
    } else {
        log::info!("[comfyui {phase}] {msg}");
    }
    let _ = app.emit(
        "comfyui-runtime",
        ComfyRuntimeEvent {
            phase: phase.to_string(),
            message: msg.clone(),
        },
    );

    if matches!(
        phase,
        "starting"
            | "started"
            | "stopping"
            | "stopped"
            | "start_failed"
            | "stop_failed"
            | "restart_failed"
    ) {
        let _ = app.emit(
            "notify",
            Notification {
                title: "ComfyPilot",
                body: msg,
            },
        );
    }
}

/// Payload for streamed stdout/stderr from a running ComfyUI process.
#[derive(Debug, Clone, Serialize)]
pub struct ComfyRuntimeLogEvent {
    pub stream: String,
    pub text: String,
}

pub fn emit_comfyui_runtime_log_event(app: &AppHandle, stream: &str, text: impl Into<String>) {
    let _ = app.emit(
        "comfyui-runtime-log",
        ComfyRuntimeLogEvent {
            stream: stream.to_string(),
            text: text.into(),
        },
    );
}

/// Progress payload shared by installs and model/LoRA/workflow downloads.
#[derive(Clone, Debug, Serialize)]
pub struct DownloadProgressEvent {
    pub kind: String,
    pub phase: String,
    pub artifact: Option<String>,
    pub index: Option<usize>,
    pub total: Option<usize>,
    pub received: Option<u64>,
    pub size: Option<u64>,
    pub folder: Option<String>,
    pub message: Option<String>,
}

pub fn emit_install_event(app: &AppHandle, phase: &str, message: &str) {
    // Also in the log file: the install log users attach to bug reports.
    match phase {
        "warn" | "error" | "failed" => log::warn!("[install {phase}] {message}"),
        _ => log::info!("[install {phase}] {message}"),
    }
    let _ = app.emit(
        "comfyui-install-progress",
        DownloadProgressEvent {
            kind: "comfyui_install".to_string(),
            phase: phase.to_string(),
            artifact: None,
            index: None,
            total: None,
            received: None,
            size: None,
            folder: None,
            message: Some(message.to_string()),
        },
    );
}
