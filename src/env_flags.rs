fn parse_env_bool(var: &str) -> Option<bool> {
    std::env::var(var).ok().and_then(|value| {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "1" | "true" | "yes" | "on" | "enable" | "enabled" => Some(true),
            "0" | "false" | "no" | "off" | "disable" | "disabled" => Some(false),
            _ => None,
        }
    })
}

pub fn remote_refresh_enabled() -> bool {
    parse_env_bool("ARCTIC_SKIP_REMOTE_REFRESH")
        .map(|skip| !skip)
        .unwrap_or(true)
}

pub fn prefer_local_catalog() -> bool {
    parse_env_bool("ARCTIC_USE_LOCAL_CATALOG").unwrap_or_else(|| !remote_refresh_enabled())
}

pub fn auto_update_enabled() -> bool {
    if let Some(skip) = parse_env_bool("ARCTIC_SKIP_AUTO_UPDATE") {
        return !skip;
    }

    parse_env_bool("ARCTIC_AUTO_UPDATE").unwrap_or(true)
}

pub fn external_package_manager() -> Option<String> {
    std::env::var("ARCTIC_PACKAGE_MANAGER")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}
