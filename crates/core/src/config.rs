//! User settings in `%APPDATA%\Resumer\config.json`. Unknown/missing fields fall back to defaults.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_MESSAGE: &str = "Если тебя оборвали лимиты — продолжай работу с того места, где остановился. Если ты уже закончил — ничего не делай.";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    #[default]
    Ru,
    En,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TargetMode {
    /// Only sessions marked `rate_limited` (or with an exhausted window).
    #[default]
    Blocked,
    /// Every known live session.
    AllKnown,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub language: Lang,
    pub message: String,
    /// Seconds to wait after `resets_at` before typing.
    pub margin_secs: u64,
    pub gap_min_secs: u64,
    pub gap_max_secs: u64,
    pub enter_delay_ms: u64,
    pub target_mode: TargetMode,
    /// Auto-wake only touches sessions whose cwd is under one of these folders. Empty = none.
    pub whitelist: Vec<String>,
    /// Never touched by anything, including the manual "wake all".
    pub excluded: Vec<String>,
    /// Manual run also types into sessions that wait for an answer.
    pub include_waiting: bool,
    pub paused: bool,
    pub dry_run: bool,
    pub autostart: bool,
    pub auto_update: bool,
    pub notifications: bool,
    pub show_limits_in_tray: bool,
    /// Global hotkey for "wake all now", e.g. `Ctrl+Alt+R`. Empty = not assigned.
    pub force_hotkey: String,
    /// The `statusLine` command that was configured before Resumer wrapped it.
    pub chained_statusline: Option<String>,
    pub last_update_check: i64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            language: Lang::Ru,
            message: DEFAULT_MESSAGE.to_string(),
            margin_secs: 10,
            gap_min_secs: 20,
            gap_max_secs: 30,
            enter_delay_ms: 80,
            target_mode: TargetMode::Blocked,
            whitelist: Vec::new(),
            excluded: Vec::new(),
            include_waiting: false,
            paused: false,
            dry_run: false,
            autostart: false,
            auto_update: true,
            notifications: true,
            show_limits_in_tray: false,
            force_hotkey: String::new(),
            chained_statusline: None,
            last_update_check: 0,
        }
    }
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(&crate::paths::config_file())
    }

    pub fn load_from(path: &Path) -> Config {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&crate::paths::config_file())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }
}

fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

/// `cwd` equals `folder` or lies inside it (case-insensitive, slash-agnostic).
pub fn path_within(cwd: &str, folder: &str) -> bool {
    let (c, f) = (norm(cwd), norm(folder));
    !f.is_empty() && (c == f || c.starts_with(&format!("{f}\\")))
}

impl Config {
    pub fn is_excluded(&self, cwd: &str) -> bool {
        self.excluded.iter().any(|f| path_within(cwd, f))
    }

    pub fn is_whitelisted(&self, cwd: &str) -> bool {
        self.whitelist.iter().any(|f| path_within(cwd, f))
    }
}

pub fn config_path_for_tests(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_files() {
        let d = tempfile::tempdir().unwrap();
        let p = config_path_for_tests(d.path());
        assert_eq!(Config::load_from(&p), Config::default());
        std::fs::write(&p, br#"{"margin_secs": 42, "language": "en"}"#).unwrap();
        let c = Config::load_from(&p);
        assert_eq!((c.margin_secs, c.language, c.gap_min_secs), (42, Lang::En, 20));
        std::fs::write(&p, b"{broken").unwrap();
        assert_eq!(Config::load_from(&p), Config::default());
    }

    #[test]
    fn save_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = config_path_for_tests(d.path());
        let mut c = Config::default();
        c.whitelist.push("C:\\Work".into());
        c.save_to(&p).unwrap();
        assert_eq!(Config::load_from(&p), c);
    }

    #[test]
    fn path_matching() {
        assert!(path_within("C:\\Work\\app", "c:/work"));
        assert!(path_within("C:\\Work", "C:\\Work\\"));
        assert!(!path_within("C:\\Workshop", "C:\\Work"));
        assert!(!path_within("C:\\x", ""));
        let mut c = Config::default();
        assert!(!c.is_whitelisted("C:\\x"));
        c.whitelist.push("C:\\x".into());
        c.excluded.push("C:\\x\\secret".into());
        assert!(c.is_whitelisted("C:\\x\\a") && c.is_excluded("C:\\x\\secret\\b") && !c.is_excluded("C:\\x\\a"));
    }
}
