//! The QML side's door into the backend: the `Backend` singleton.
//!
//! `Backend.invoke(command, argsJson)` starts a command off the UI thread
//! and returns a request id; the result arrives later as
//! `replied(id, ok, json)`. Events the backend emits (download progress,
//! install steps, ComfyUI status...) arrive as `backendEvent(name, json)`.
//! `qml/Api.js` wraps both into promises and listeners.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Backend = super::BackendRust;

        /// Starts `command` with a JSON object of arguments; returns the request id.
        #[qinvokable]
        fn invoke(self: Pin<&mut Backend>, command: &QString, args_json: &QString) -> i32;

        /// Saves an image of the window, popups and dialogs included (the
        /// development `--screenshot=` option).
        #[qinvokable]
        #[cxx_name = "saveWindowImage"]
        fn save_window_image(self: &Backend, path: &QString) -> bool;

        /// The result of request `request_id`: JSON value, or a JSON string error.
        #[qsignal]
        fn replied(self: Pin<&mut Backend>, request_id: i32, ok: bool, json: QString);

        /// An event from the backend, with a JSON payload.
        #[qsignal]
        #[cxx_name = "backendEvent"]
        fn backend_event(self: Pin<&mut Backend>, name: QString, json: QString);
    }

    impl cxx_qt::Threading for Backend {}
    impl cxx_qt::Initialize for Backend {}
}

use crate::commands;
use crate::host::{self, async_runtime};
use core::pin::Pin;
use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde_json::Value;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct BackendRust {
    next_id: AtomicI32,
}

/// Where backend events go: the `Backend` object once QML has created it;
/// until then they're kept and delivered when it appears.
enum EventTarget {
    Waiting(Vec<(String, Value)>),
    Ready(CxxQtThread<qobject::Backend>),
}

static EVENT_TARGET: Mutex<EventTarget> = Mutex::new(EventTarget::Waiting(Vec::new()));
/// Events kept before the UI exists; more than this and the oldest go.
const MAX_EARLY_EVENTS: usize = 500;

fn deliver(thread: &CxxQtThread<qobject::Backend>, name: String, payload: Value) {
    let json = payload.to_string();
    let _ = thread.queue(move |backend| {
        backend.backend_event(QString::from(&name), QString::from(&json));
    });
}

/// The `host::EventSink` the app is created with.
pub fn push_event(name: &str, payload: Value) {
    let Ok(mut target) = EVENT_TARGET.lock() else {
        return;
    };
    match &mut *target {
        EventTarget::Ready(thread) => deliver(thread, name.to_string(), payload),
        EventTarget::Waiting(queue) => {
            if queue.len() >= MAX_EARLY_EVENTS {
                queue.remove(0);
            }
            queue.push((name.to_string(), payload));
        }
    }
}

impl cxx_qt::Initialize for qobject::Backend {
    fn initialize(self: Pin<&mut Self>) {
        let thread = self.qt_thread();
        if let Ok(mut target) = EVENT_TARGET.lock() {
            if let EventTarget::Waiting(queue) = &mut *target {
                for (name, payload) in queue.drain(..) {
                    deliver(&thread, name, payload);
                }
            }
            *target = EventTarget::Ready(thread);
        }
    }
}

impl qobject::Backend {
    pub fn save_window_image(&self, path: &QString) -> bool {
        crate::qt_app::save_main_window_image(path)
    }

    pub fn invoke(self: Pin<&mut Self>, command: &QString, args_json: &QString) -> i32 {
        let id = self.as_ref().rust().next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let thread = self.qt_thread();
        let command = command.to_string();
        let raw_args = args_json.to_string();
        let args = if raw_args.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(&raw_args).unwrap_or(Value::Object(Default::default()))
        };
        let pending = match host::handle() {
            Some(handle) => commands::dispatch(handle, &command, args),
            None => Box::pin(async { Err("The backend is not ready.".to_string()) }),
        };
        async_runtime::spawn(async move {
            let (ok, json) = match pending.await {
                Ok(value) => (true, value.to_string()),
                Err(message) => {
                    log::debug!("Command `{command}` failed: {message}");
                    (false, Value::String(message).to_string())
                }
            };
            let _ = thread.queue(move |backend| {
                backend.replied(id, ok, QString::from(&json));
            });
        });
        id
    }
}
