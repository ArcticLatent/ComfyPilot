#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "linux")]
mod app_linux;
#[cfg(target_os = "windows")]
mod app_windows;
mod bridge;
mod commands;
mod contracts;
mod host;
mod platform;
mod qt_app;
mod shared;

#[cfg(target_os = "linux")]
use app_linux as platform_app;
#[cfg(target_os = "windows")]
use app_windows as platform_app;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("ComfyPilot currently supports Linux and Windows targets");

use cxx_qt_lib::{QQmlApplicationEngine, QUrl};
use std::sync::Arc;

const MAIN_QML: &str = "qrc:/qt/qml/ArcticHelper/qml/Main.qml";

fn main() {
    let state = platform_app::init();
    let _handle = host::AppHandle::new(state, Arc::new(bridge::push_event));

    // The look comes from our own QML components; keep the platform style
    // out of the way.
    if std::env::var_os("QT_QUICK_CONTROLS_STYLE").is_none() {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Basic");
    }
    qt_app::create_application();
    if !qt_app::claim_single_instance() {
        log::info!("ComfyPilot is already running; showed its window instead.");
        return;
    }

    let mut engine = QQmlApplicationEngine::new();
    let loaded = match engine.as_mut() {
        Some(mut engine) => {
            qt_app::enable_network_disk_cache(engine.as_mut());
            engine.as_mut().load(&QUrl::from(MAIN_QML));
            qt_app::engine_loaded(engine.as_mut())
        }
        None => false,
    };
    // Without a window nothing would ever quit the app (it doesn't quit when
    // its last window closes; see cpp/arctic_qt.h).
    if !loaded {
        log::error!("The user interface failed to load; see the Qt messages above.");
        std::process::exit(1);
    }
    let code = qt_app::exec();
    drop(engine);
    std::process::exit(code);
}
