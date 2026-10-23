//! Command calls from the UI: a name plus a JSON object of arguments in, a
//! JSON result (or an error message) out. The per-platform tables live in
//! `app_linux/commands.rs` and `app_windows/commands.rs`.

use crate::host::async_runtime;
use crate::host::AppHandle;
use crate::platform_app;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// A command in progress.
pub type Pending = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

/// Starts command `name`; unknown names fail with an error message.
pub fn dispatch(handle: &AppHandle, name: &str, args: Value) -> Pending {
    match platform_app::commands::dispatch(handle, name, args) {
        Some(pending) => pending,
        None => {
            let message = format!("Unknown command `{name}`.");
            Box::pin(async move { Err(message) })
        }
    }
}

/// Reads argument `key` (camelCase, or the snake_case name) from `args`. A
/// missing argument reads as `null`, so `Option` parameters may be left out.
pub fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    let value = args
        .get(key)
        .or_else(|| args.get(snake_case(key)))
        .cloned()
        .unwrap_or(Value::Null);
    serde_json::from_value(value).map_err(|err| format!("Invalid argument `{key}`: {err}"))
}

fn snake_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Runs a synchronous command on the blocking pool.
pub async fn blocking<F>(work: F) -> Result<Value, String>
where
    F: FnOnce() -> Result<Value, String> + Send + 'static,
{
    async_runtime::spawn_blocking(work)
        .await
        .map_err(|err| format!("Command task failed: {err}"))?
}

pub fn reply<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| err.to_string())
}

pub fn reply_result<T: Serialize>(result: Result<T, String>) -> Result<Value, String> {
    result.and_then(reply)
}

pub fn reply_unit(_: ()) -> Result<Value, String> {
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arguments_accept_camel_and_snake_case_and_missing_options() {
        let args = json!({ "comfyuiRoot": "/x", "model_id": 7 });
        assert_eq!(arg::<String>(&args, "comfyuiRoot").unwrap(), "/x");
        assert_eq!(arg::<u64>(&args, "modelId").unwrap(), 7);
        assert_eq!(arg::<Option<String>>(&args, "basePath").unwrap(), None);
        assert!(arg::<String>(&args, "path").is_err());
    }

    #[test]
    fn every_command_name_is_unique() {
        let mut names = platform_app::commands::COMMANDS.to_vec();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
    }
}
