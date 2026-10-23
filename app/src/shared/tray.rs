//! The system tray's backend side. The tray itself is drawn by QML
//! (`SystemTrayIcon`); the backend tells it whether ComfyUI is running
//! (`tray-status` event) and carries out its Start/Stop/Restart menu items.

use super::{
    comfyui_runtime_running, emit_comfyui_runtime_event, resolve_comfyui_instance_name,
    start_comfyui_root_background, AppState,
};
use crate::host::{AppHandle, State};
use crate::platform::stop_comfyui_root_impl;
use serde::Serialize;
use std::time::{Duration, Instant};

/// How long a restart waits for the stopped ComfyUI to let go of its port.
const RESTART_STOP_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize)]
pub struct TrayStatus {
    pub enabled: bool,
    pub running: bool,
    pub tooltip: String,
}

/// Whether to show a tray icon. On Linux it can be turned off with
/// `ARCTIC_ENABLE_TRAY=0` (some desktops have no tray).
pub fn tray_enabled_for_platform() -> bool {
    if !cfg!(feature = "desktop-tray") {
        return false;
    }
    if cfg!(target_os = "linux") {
        return std::env::var("ARCTIC_ENABLE_TRAY").map_or(true, |v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        });
    }
    true
}

fn tray_status(state: &AppState, running: bool) -> TrayStatus {
    let tooltip = if running {
        // The running install's name (not the selected one's).
        let root = super::comfyui_running_root(state)
            .1
            .map(|root| root.to_string_lossy().to_string());
        let name = resolve_comfyui_instance_name(&state.context, root);
        format!("ComfyPilot - Running: {name}")
    } else {
        "ComfyPilot - ComfyUI: Stopped".to_string()
    };
    TrayStatus {
        enabled: tray_enabled_for_platform(),
        running,
        tooltip,
    }
}

/// Tells the tray whether ComfyUI is running.
pub fn update_tray_comfy_status(app: &AppHandle, running: bool) {
    let state = app.state::<AppState>();
    let _ = app.emit("tray-status", tray_status(&state, running));
}

/// The tray's current state, for when the UI starts.
pub fn get_tray_status(state: State<'_, AppState>) -> TrayStatus {
    let running = comfyui_runtime_running(&state);
    tray_status(&state, running)
}

/// The tray's "Start ComfyUI" item.
pub fn tray_start_comfyui(app: AppHandle) {
    let state = app.state::<AppState>();
    if comfyui_runtime_running(&state) {
        let name = resolve_comfyui_instance_name(&state.context, None);
        update_tray_comfy_status(&app, true);
        emit_comfyui_runtime_event(&app, "started", format!("{name} is already running."));
    } else {
        start_comfyui_root_background(&app, None);
    }
}

/// The tray's "Stop ComfyUI" item.
pub fn tray_stop_comfyui(app: AppHandle) {
    let state = app.state::<AppState>();
    let name = resolve_comfyui_instance_name(&state.context, None);
    emit_comfyui_runtime_event(&app, "stopping", format!("Stopping {name}..."));
    if let Err(err) = stop_comfyui_root_impl(&state) {
        log::warn!("Tray stop ComfyUI failed: {err}");
        emit_comfyui_runtime_event(&app, "stop_failed", format!("{name} stop failed: {err}"));
        return;
    }
    let running = comfyui_runtime_running(&state);
    update_tray_comfy_status(&app, running);
    if running {
        emit_comfyui_runtime_event(
            &app,
            "stop_failed",
            format!("{name} stop did not fully complete."),
        );
    } else {
        emit_comfyui_runtime_event(&app, "stopped", format!("{name} stopped."));
    }
}

/// The tray's "Restart ComfyUI" item: stops the running ComfyUI and, once it
/// has let go of its port, starts it again (or just starts it if it wasn't
/// running). Runs in the background.
pub fn tray_restart_comfyui(app: AppHandle) {
    let state = app.state::<AppState>();
    if !comfyui_runtime_running(&state) {
        start_comfyui_root_background(&app, None);
        return;
    }
    let name = resolve_comfyui_instance_name(&state.context, None);
    emit_comfyui_runtime_event(&app, "restarting", format!("Restarting {name}..."));
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        if let Err(err) = stop_comfyui_root_impl(&state) {
            log::warn!("Tray restart: stopping ComfyUI failed: {err}");
            update_tray_comfy_status(&app, comfyui_runtime_running(&state));
            emit_comfyui_runtime_event(
                &app,
                "restart_failed",
                format!("{name} restart failed: {err}"),
            );
            return;
        }
        let deadline = Instant::now() + RESTART_STOP_WAIT;
        while comfyui_runtime_running(&state) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
        }
        if comfyui_runtime_running(&state) {
            update_tray_comfy_status(&app, true);
            emit_comfyui_runtime_event(
                &app,
                "restart_failed",
                format!("{name} did not stop, so it wasn't restarted."),
            );
            return;
        }
        update_tray_comfy_status(&app, false);
        start_comfyui_root_background(&app, None);
    });
}

/// Marks the app as quitting (so closing the window really quits).
pub fn set_app_quitting(state: State<'_, AppState>) {
    if let Ok(mut quitting) = state.quitting.lock() {
        *quitting = true;
    }
}
