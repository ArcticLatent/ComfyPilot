//! The part of Tauri's API the backend was written against, provided by the
//! app itself now that the UI is Qt: an `AppHandle` that owns the
//! `AppState` and sends events to the UI, the `State` wrapper commands take,
//! and an `async_runtime` on a shared Tokio runtime.
//!
//! Keeping these shapes means the platform backend didn't need rewriting:
//! `app.emit(..)`, `app.state::<AppState>()`, `State<'_, AppState>` and
//! `async_runtime::spawn_blocking(..)` work as before.

use crate::shared::AppState;
use serde::Serialize;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

/// Receives every event the backend emits, as `(name, JSON payload)`.
pub type EventSink = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

struct Inner {
    state: AppState,
    sink: EventSink,
}

/// A cheap, cloneable handle to the app: its state and its event sink.
#[derive(Clone)]
pub struct AppHandle {
    inner: Arc<Inner>,
}

static HANDLE: OnceLock<AppHandle> = OnceLock::new();

impl AppHandle {
    /// Creates the app's handle; the first one becomes [`handle()`].
    pub fn new(state: AppState, sink: EventSink) -> Self {
        let handle = AppHandle {
            inner: Arc::new(Inner { state, sink }),
        };
        let _ = HANDLE.set(handle.clone());
        handle
    }

    /// Sends an event to the UI.
    pub fn emit<S: Serialize>(&self, event: &str, payload: S) -> Result<(), String> {
        let value = serde_json::to_value(payload).map_err(|err| err.to_string())?;
        (self.inner.sink)(event, value);
        Ok(())
    }

    /// The managed state (only `AppState` exists).
    pub fn state<T: Managed>(&self) -> State<'_, T> {
        State(T::from_handle(self))
    }

    /// Asks the UI to quit the application.
    pub fn exit(&self, code: i32) {
        let _ = self.emit("app-exit", code);
    }
}

/// The app's handle, once created.
pub fn handle() -> Option<&'static AppHandle> {
    HANDLE.get()
}

/// State types reachable through [`AppHandle::state`].
pub trait Managed: Sized + 'static {
    fn from_handle(handle: &AppHandle) -> &Self;
}

impl Managed for AppState {
    fn from_handle(handle: &AppHandle) -> &Self {
        &handle.inner.state
    }
}

/// A borrowed managed state, as commands receive it.
pub struct State<'a, T>(&'a T);

impl<T> Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

impl<T> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for State<'_, T> {}

/// Background work on one shared multi-threaded Tokio runtime.
pub mod async_runtime {
    use super::*;

    pub fn runtime() -> &'static tokio::runtime::Runtime {
        static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("arctic-worker")
                .build()
                .expect("failed to start the async runtime")
        })
    }

    /// A spawned task; awaiting it gives the task's result or why it didn't finish.
    pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);

    impl<T> JoinHandle<T> {
        pub fn inner(&self) -> &tokio::task::JoinHandle<T> {
            &self.0
        }
    }

    impl<T> Future for JoinHandle<T> {
        type Output = Result<T, tokio::task::JoinError>;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(&mut self.0).poll(cx)
        }
    }

    pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        JoinHandle(runtime().spawn(future))
    }

    pub fn spawn_blocking<F, R>(work: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        JoinHandle(runtime().spawn_blocking(work))
    }
}
