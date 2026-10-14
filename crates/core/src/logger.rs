//! Tiny file logger with size-based rotation. Never logs session content, only events and counts.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_LOG_BYTES: u64 = 512 * 1024;
static LOCK: Mutex<()> = Mutex::new(());

pub fn log(msg: &str) {
    log_to(&crate::paths::log_file(), msg);
}

pub fn log_to(path: &Path, msg: &str) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if fs::metadata(path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = fs::rename(path, rotated(path));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            f,
            "{} {}",
            format_time(crate::now_epoch()),
            msg.replace(['\r', '\n'], " ")
        );
    }
}

fn rotated(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".1");
    PathBuf::from(s)
}

/// Last `n` lines of the current log (for crash reports and diagnostics).
pub fn tail(n: usize) -> String {
    let text = fs::read_to_string(crate::paths::log_file()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `YYYY-MM-DD HH:MM:SS` in UTC (no timezone database needed).
pub fn format_time(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_epoch() {
        assert_eq!(format_time(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_time(1_760_000_000), "2025-10-09 08:53:20Z");
    }

    #[test]
    fn rotates_when_large() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.log");
        fs::write(&p, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        log_to(&p, "hello\nworld");
        assert!(dir.path().join("a.log.1").exists());
        let t = fs::read_to_string(&p).unwrap();
        assert!(t.contains("hello world") && t.lines().count() == 1);
    }
}
