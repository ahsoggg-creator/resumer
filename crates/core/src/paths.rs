use std::path::PathBuf;

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `%APPDATA%\Resumer` (override with `RESUMER_HOME`, used by tests and demo mode).
pub fn data_dir() -> PathBuf {
    if let Some(p) = env_path("RESUMER_HOME") {
        return p;
    }
    env_path("APPDATA")
        .or_else(|| env_path("HOME").map(|h| h.join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join("Resumer")
}

pub fn state_dir() -> PathBuf {
    data_dir().join("state")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn crash_dir() -> PathBuf {
    logs_dir().join("crashes")
}

pub fn log_file() -> PathBuf {
    logs_dir().join("resumer.log")
}

pub fn config_file() -> PathBuf {
    data_dir().join("config.json")
}

pub fn runtime_file() -> PathBuf {
    data_dir().join("runtime.json")
}

/// `~/.claude/settings.json`
pub fn claude_settings_file() -> PathBuf {
    env_path("USERPROFILE")
        .or_else(|| env_path("HOME"))
        .unwrap_or_default()
        .join(".claude")
        .join("settings.json")
}

pub fn install_dir() -> PathBuf {
    env_path("LOCALAPPDATA")
        .unwrap_or_else(std::env::temp_dir)
        .join("Programs")
        .join("Resumer")
}
