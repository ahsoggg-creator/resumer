//! Safe merge of Resumer's hooks and statusLine into `~/.claude/settings.json`.
//! Existing hooks and the existing statusLine are preserved (the latter is chained, not replaced).
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Substring that identifies commands written by Resumer.
pub const MARKER: &str = "resumer-cli";

/// `(event, matcher, subcommand)`
pub const HOOKS: &[(&str, Option<&str>, &str)] = &[
    ("UserPromptSubmit", None, "activity"),
    ("Stop", None, "activity"),
    ("StopFailure", Some("rate_limit"), "stop-failure"),
    ("Notification", None, "notification"),
    ("SessionEnd", None, "activity"),
];

#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error("settings.json is not valid JSON ({0}); nothing was changed")]
    Invalid(String),
    #[error("settings.json root is not an object; nothing was changed")]
    NotObject,
    #[error("unexpected structure under \"{0}\"; nothing was changed")]
    BadShape(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn is_ours(command: &str) -> bool {
    command.contains(MARKER)
}

/// Shell-agnostic command line (Git Bash and PowerShell): forward slashes, quotes only if the path has spaces.
pub fn command_line(exe: &str, sub: &str) -> String {
    let p = exe.replace('\\', "/");
    if p.contains(' ') {
        format!("\"{p}\" hook {sub}")
    } else {
        format!("{p} hook {sub}")
    }
}

pub fn parse(text: &str) -> Result<Value, MergeError> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| MergeError::Invalid(e.to_string()))?;
    if v.is_object() {
        Ok(v)
    } else {
        Err(MergeError::NotObject)
    }
}

pub fn render(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    s.push('\n');
    s
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstallResult {
    pub settings: Value,
    /// The statusLine command found in the file before Resumer wrapped it.
    pub chained: Option<String>,
}

pub fn install(settings: &Value, exe: &str) -> Result<InstallResult, MergeError> {
    let mut root = settings.as_object().cloned().ok_or(MergeError::NotObject)?;

    // --- hooks ---
    let hooks_val = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks_val
        .as_object_mut()
        .ok_or_else(|| MergeError::BadShape("hooks".into()))?;
    for (event, matcher, sub) in HOOKS {
        let cmd = command_line(exe, sub);
        let groups = hooks.entry(*event).or_insert_with(|| json!([]));
        let groups = groups
            .as_array_mut()
            .ok_or_else(|| MergeError::BadShape(format!("hooks.{event}")))?;
        let mut done = false;
        for g in groups.iter_mut() {
            let Some(arr) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            for h in arr.iter_mut() {
                if let Some(c) = h.get("command").and_then(Value::as_str)
                    && is_ours(c)
                    && c.contains(&format!("hook {sub}"))
                {
                    h["command"] = json!(cmd);
                    done = true;
                }
            }
        }
        if done {
            continue;
        }
        let entry = json!({"type": "command", "command": cmd, "timeout": 5});
        let same_matcher = groups.iter_mut().find(|g| {
            g.get("matcher").and_then(Value::as_str) == *matcher && g.get("hooks").is_some_and(Value::is_array)
        });
        match same_matcher {
            Some(g) => g["hooks"].as_array_mut().expect("checked").push(entry),
            None => {
                let mut g = Map::new();
                if let Some(m) = matcher {
                    g.insert("matcher".into(), json!(m));
                }
                g.insert("hooks".into(), json!([entry]));
                groups.push(Value::Object(g));
            }
        }
    }

    // --- statusLine ---
    let ours = command_line(exe, "statusline");
    let mut chained = None;
    match root.get_mut("statusLine") {
        Some(Value::Object(sl)) => {
            let existing = sl.get("command").and_then(Value::as_str).map(str::to_string);
            match existing {
                Some(c) if is_ours(&c) => {
                    sl.insert("command".into(), json!(ours));
                }
                Some(c) => {
                    chained = Some(c);
                    sl.insert("command".into(), json!(ours));
                }
                None => return Err(MergeError::BadShape("statusLine".into())),
            }
        }
        Some(_) => return Err(MergeError::BadShape("statusLine".into())),
        None => {
            root.insert("statusLine".into(), json!({"type": "command", "command": ours}));
        }
    }
    Ok(InstallResult {
        settings: Value::Object(root),
        chained,
    })
}

/// Removes Resumer's entries; restores the chained statusLine command (or drops the statusLine if there was none).
pub fn uninstall(settings: &Value, chained: Option<&str>) -> Result<Value, MergeError> {
    let mut root = settings.as_object().cloned().ok_or(MergeError::NotObject)?;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for (_, groups) in hooks.iter_mut() {
            let Some(groups) = groups.as_array_mut() else { continue };
            for g in groups.iter_mut() {
                if let Some(arr) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    arr.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(is_ours));
                }
            }
            groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|a| !a.is_empty()));
        }
        hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    }
    if root.get("hooks").and_then(Value::as_object).is_some_and(Map::is_empty) {
        root.remove("hooks");
    }
    let ours = root
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
        .is_some_and(is_ours);
    if ours {
        match chained {
            Some(c) => {
                root.get_mut("statusLine")
                    .and_then(Value::as_object_mut)
                    .expect("object")
                    .insert("command".into(), json!(c));
            }
            None => {
                root.remove("statusLine");
            }
        }
    }
    Ok(Value::Object(root))
}

pub fn is_installed(settings: &Value) -> bool {
    settings
        .pointer("/hooks/StopFailure")
        .and_then(Value::as_array)
        .is_some_and(|g| g.iter().any(|g| g.to_string().contains(MARKER)))
}

/// Minimal unified-style line diff (LCS), `+`/`-` lines with two lines of context.
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || t[i][j + 1] >= t[i + 1][j]) {
            ops.push(('+', b[j]));
            j += 1;
        } else {
            ops.push(('-', a[i]));
            i += 1;
        }
    }
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, (c, _))| *c != ' ')
        .map(|(k, _)| k)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut keep = vec![false; ops.len()];
    for &k in &changed {
        let end = (k + 3).min(ops.len());
        keep[k.saturating_sub(2)..end].fill(true);
    }
    let mut out = String::new();
    let mut gap = false;
    for (k, (c, line)) in ops.iter().enumerate() {
        if keep[k] {
            if gap {
                out.push_str("  ...\n");
                gap = false;
            }
            out.push_str(&format!("{c} {line}\n"));
        } else {
            gap = true;
        }
    }
    out
}

// ---------- file operations ----------

pub struct Applied {
    pub backup: Option<PathBuf>,
}

fn backup_name(path: &Path, stamp: i64) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".resumer-backup-{stamp}"));
    PathBuf::from(s)
}

/// Writes `new_text` atomically after copying the original to `settings.json.resumer-backup-<epoch>`.
pub fn write_with_backup(path: &Path, new_text: &str, stamp: i64) -> Result<Applied, MergeError> {
    let backup = if path.exists() {
        let b = backup_name(path, stamp);
        std::fs::copy(path, &b)?;
        Some(b)
    } else {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        None
    };
    let tmp = path.with_extension("json.resumer-tmp");
    std::fs::write(&tmp, new_text)?;
    std::fs::rename(&tmp, path)?;
    Ok(Applied { backup })
}

/// Puts a backup made by `write_with_backup` back in place.
pub fn rollback(path: &Path, backup: &Path) -> Result<(), MergeError> {
    std::fs::copy(backup, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "C:\\Users\\me\\AppData\\Local\\Programs\\Resumer\\resumer-cli.exe";

    fn count_ours(v: &Value) -> usize {
        v.to_string().matches(MARKER).count()
    }

    #[test]
    fn empty_settings_get_everything() {
        let r = install(&json!({}), EXE).unwrap();
        assert!(r.chained.is_none());
        let s = &r.settings;
        assert_eq!(s["statusLine"]["type"], "command");
        assert!(
            s["statusLine"]["command"]
                .as_str()
                .unwrap()
                .ends_with("resumer-cli.exe hook statusline")
        );
        assert_eq!(s["hooks"]["StopFailure"][0]["matcher"], "rate_limit");
        for (ev, _, _) in HOOKS {
            assert_eq!(s["hooks"][*ev].as_array().unwrap().len(), 1, "{ev}");
        }
        assert!(!s["hooks"]["Stop"][0].as_object().unwrap().contains_key("matcher"));
        assert!(is_installed(s));
    }

    #[test]
    fn foreign_statusline_is_chained_and_extra_keys_kept() {
        let orig = json!({"statusLine": {"type": "command", "command": "python C:/x/statusline.py", "padding": 2}});
        let r = install(&orig, EXE).unwrap();
        assert_eq!(r.chained.as_deref(), Some("python C:/x/statusline.py"));
        assert_eq!(r.settings["statusLine"]["padding"], 2);
        assert!(
            r.settings["statusLine"]["command"]
                .as_str()
                .unwrap()
                .contains("hook statusline")
        );
        let back = uninstall(&r.settings, r.chained.as_deref()).unwrap();
        assert_eq!(back, orig, "uninstall must restore the exact original statusLine");
    }

    #[test]
    fn foreign_hooks_survive_install_and_uninstall() {
        let orig = json!({
            "model": "opusplan",
            "hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}],
                "Stop": [{"hooks": [{"type": "command", "command": "notify.sh"}]}],
                "StopFailure": [{"matcher": "rate_limit", "hooks": [{"type": "command", "command": "mine.sh"}]}]
            }
        });
        let r = install(&orig, EXE).unwrap();
        let h = &r.settings["hooks"];
        assert_eq!(h["PreToolUse"], orig["hooks"]["PreToolUse"]);
        assert_eq!(h["Stop"][0]["hooks"][0]["command"], "notify.sh");
        assert_eq!(
            h["Stop"][0]["hooks"].as_array().unwrap().len(),
            2,
            "joined the matcher-less group"
        );
        assert_eq!(h["StopFailure"][0]["hooks"].as_array().unwrap().len(), 2);
        assert_eq!(r.settings["model"], "opusplan");
        let back = uninstall(&r.settings, r.chained.as_deref()).unwrap();
        assert_eq!(back, orig);
    }

    #[test]
    fn install_is_idempotent_and_updates_path() {
        let once = install(&json!({}), EXE).unwrap();
        let again = install(&once.settings, "D:/new place/resumer-cli.exe").unwrap();
        assert_eq!(count_ours(&again.settings), count_ours(&once.settings), "no duplicates");
        assert!(
            again.settings["statusLine"]["command"]
                .as_str()
                .unwrap()
                .starts_with("\"D:/new place/resumer-cli.exe\" hook")
        );
        assert!(
            again.chained.is_none(),
            "own statusLine must never be chained to itself"
        );
    }

    #[test]
    fn uninstall_on_pristine_settings_is_a_noop() {
        let orig = json!({"theme": "dark"});
        assert_eq!(uninstall(&orig, None).unwrap(), orig);
        let r = install(&orig, EXE).unwrap();
        assert_eq!(uninstall(&r.settings, None).unwrap(), orig);
    }

    #[test]
    fn bad_shapes_are_refused() {
        assert!(matches!(
            install(&json!({"hooks": []}), EXE),
            Err(MergeError::BadShape(_))
        ));
        assert!(matches!(
            install(&json!({"hooks": {"Stop": {}}}), EXE),
            Err(MergeError::BadShape(_))
        ));
        assert!(matches!(
            install(&json!({"statusLine": "x"}), EXE),
            Err(MergeError::BadShape(_))
        ));
        assert!(matches!(parse("[1]"), Err(MergeError::NotObject)));
        assert!(matches!(parse("{oops"), Err(MergeError::Invalid(_))));
        assert_eq!(parse("  \n").unwrap(), json!({}));
    }

    #[test]
    fn diff_shows_only_changes() {
        let d = diff("a\nb\nc\n", "a\nb\nX\nc\n");
        assert_eq!(d, "  a\n  b\n+ X\n  c\n");
        assert_eq!(diff("same\n", "same\n"), "");
    }

    #[test]
    fn backup_write_and_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, "{\"a\": 1}").unwrap();
        let ap = write_with_backup(&p, "{\"a\": 2}\n", 77).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\": 2}\n");
        let b = ap.backup.unwrap();
        assert!(b.to_string_lossy().ends_with("settings.json.resumer-backup-77"));
        rollback(&p, &b).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\": 1}");
        let fresh = dir.path().join("new").join("settings.json");
        assert!(write_with_backup(&fresh, "{}\n", 1).unwrap().backup.is_none());
    }

    #[test]
    fn command_line_quotes_only_when_needed() {
        assert_eq!(
            command_line("C:\\a\\resumer-cli.exe", "x"),
            "C:/a/resumer-cli.exe hook x"
        );
        assert_eq!(
            command_line("C:\\a b\\resumer-cli.exe", "x"),
            "\"C:/a b/resumer-cli.exe\" hook x"
        );
    }
}
