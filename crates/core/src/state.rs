//! Per-session state files in `%APPDATA%\Resumer\state\<session_id>.json`.
//! Writes are atomic (temp file + rename), reads tolerate partial/garbage files.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Running,
    Idle,
    RateLimited,
    NeedsInput,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Idle => "idle",
            Status::RateLimited => "rate_limited",
            Status::NeedsInput => "needs_input",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct Window {
    #[serde(default)]
    pub used_percentage: f64,
    /// Unix seconds.
    #[serde(default)]
    pub resets_at: i64,
}

impl Window {
    pub fn exhausted(&self) -> bool {
        self.used_percentage >= 100.0
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SessionState {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub claude_pid: Option<u32>,
    /// Process creation time (FILETIME ticks) of `claude_pid`; guards against PID reuse.
    #[serde(default)]
    pub claude_started: Option<u64>,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub five_hour: Option<Window>,
    #[serde(default)]
    pub seven_day: Option<Window>,
    #[serde(default)]
    pub updated_at: i64,
    /// `resets_at` of the limit window we already nudged this session for.
    #[serde(default)]
    pub last_nudged_epoch: i64,
    /// Unix seconds when the limit that stopped this session resets (0 = not blocked / unknown).
    /// Captured when the limit hits, so later post-reset statusLine data cannot hide the original reset time.
    #[serde(default)]
    pub blocked_until: i64,
}

impl SessionState {
    pub fn new(session_id: &str) -> Self {
        SessionState {
            session_id: session_id.to_string(),
            ..Default::default()
        }
    }
}

/// Session ids come from Claude Code, but they end up in file names: keep them harmless.
pub fn sanitize_id(id: &str) -> String {
    let s: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(100)
        .collect();
    if s.is_empty() { "unknown".into() } else { s }
}

#[derive(Clone, Debug)]
pub struct StateStore {
    dir: PathBuf,
}

impl StateStore {
    pub fn new(dir: PathBuf) -> Self {
        StateStore { dir }
    }

    pub fn default_store() -> Self {
        Self::new(crate::paths::state_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", sanitize_id(id)))
    }

    pub fn load(&self, id: &str) -> Option<SessionState> {
        read_state_file(&self.path(id))
    }

    pub fn save(&self, st: &SessionState) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path(&st.session_id);
        let tmp = self
            .dir
            .join(format!("{}.tmp{}", sanitize_id(&st.session_id), std::process::id()));
        let data = serde_json::to_vec_pretty(st).map_err(std::io::Error::other)?;
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all().ok();
        }
        fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    /// Read-modify-write under a short per-session lock so concurrent hooks do not lose updates.
    pub fn update<F: FnOnce(&mut SessionState)>(&self, id: &str, f: F) -> std::io::Result<SessionState> {
        fs::create_dir_all(&self.dir)?;
        let lock = self.dir.join(format!("{}.lock", sanitize_id(id)));
        let _guard = Lock::acquire(&lock);
        let mut st = self.load(id).unwrap_or_else(|| SessionState::new(id));
        st.session_id = id.to_string();
        f(&mut st);
        self.save(&st)?;
        Ok(st)
    }

    pub fn remove(&self, id: &str) {
        let _ = fs::remove_file(self.path(id));
    }

    /// All readable states; unreadable/foreign files are skipped.
    pub fn list(&self) -> Vec<SessionState> {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut v: Vec<SessionState> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| read_state_file(&p))
            .filter(|s| !s.session_id.is_empty())
            .collect();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.session_id.cmp(&b.session_id)));
        v
    }

    /// Removes states whose Claude process is gone (or whose PID was reused). Returns removed count.
    pub fn prune_dead(&self, is_alive: impl Fn(u32, Option<u64>) -> bool) -> usize {
        let mut n = 0;
        for s in self.list() {
            if let Some(pid) = s.claude_pid
                && !is_alive(pid, s.claude_started)
            {
                self.remove(&s.session_id);
                n += 1;
            }
        }
        n
    }

    /// Leftover temp/lock files from crashed hooks.
    pub fn sweep_stale_files(&self, max_age: Duration) {
        let Ok(rd) = fs::read_dir(&self.dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            if (ext == "lock" || ext.starts_with("tmp"))
                && fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|a| a > max_age)
            {
                let _ = fs::remove_file(p);
            }
        }
    }
}

fn read_state_file(path: &Path) -> Option<SessionState> {
    for attempt in 0..2 {
        match fs::read(path) {
            Ok(b) => {
                if let Ok(s) = serde_json::from_slice::<SessionState>(&b) {
                    return Some(s);
                }
            }
            Err(_) => return None,
        }
        if attempt == 0 {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    None
}

struct Lock(Option<PathBuf>);

impl Lock {
    fn acquire(path: &Path) -> Lock {
        let start = Instant::now();
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => return Lock(Some(path.to_path_buf())),
                Err(_) => {
                    let stale = fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|a| a > Duration::from_secs(2));
                    if stale {
                        let _ = fs::remove_file(path);
                    } else if start.elapsed() > Duration::from_millis(300) {
                        return Lock(None); // proceed without the lock rather than block a hook
                    } else {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, StateStore) {
        let d = tempfile::tempdir().unwrap();
        let s = StateStore::new(d.path().join("state"));
        (d, s)
    }

    #[test]
    fn roundtrip_and_update() {
        let (_d, s) = store();
        let st = s
            .update("abc-1", |st| {
                st.cwd = "C:\\p".into();
                st.status = Status::RateLimited;
                st.five_hour = Some(Window {
                    used_percentage: 100.0,
                    resets_at: 5,
                });
            })
            .unwrap();
        assert_eq!(s.load("abc-1").unwrap(), st);
        assert_eq!(s.list().len(), 1);
        assert!(!s.dir().join("abc-1.lock").exists());
    }

    #[test]
    fn tolerates_partial_files_and_foreign_json() {
        let (_d, s) = store();
        fs::create_dir_all(s.dir()).unwrap();
        fs::write(s.dir().join("bad.json"), b"{\"session_id\": \"x\", \"stat").unwrap();
        fs::write(s.dir().join("other.json"), b"[1,2]").unwrap();
        s.update("good", |_| {}).unwrap();
        let l = s.list();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].session_id, "good");
        assert!(s.load("bad").is_none());
    }

    #[test]
    fn id_is_sanitized() {
        assert_eq!(sanitize_id("../../evil"), "evil");
        assert_eq!(sanitize_id(""), "unknown");
        let (_d, s) = store();
        s.update("../x", |_| {}).unwrap();
        assert!(s.dir().join("x.json").exists());
    }

    #[test]
    fn prunes_dead_pids_only() {
        let (_d, s) = store();
        s.update("live", |st| st.claude_pid = Some(1)).unwrap();
        s.update("dead", |st| st.claude_pid = Some(2)).unwrap();
        s.update("nopid", |_| {}).unwrap();
        let n = s.prune_dead(|pid, _| pid == 1);
        assert_eq!(n, 1);
        assert!(s.load("dead").is_none() && s.load("live").is_some() && s.load("nopid").is_some());
    }

    #[test]
    fn parallel_updates_do_not_lose_writes() {
        let (_d, s) = store();
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let s = s.clone();
                std::thread::spawn(move || {
                    s.update("p", |st| st.last_nudged_epoch += 1).unwrap();
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(s.load("p").unwrap().last_nudged_epoch, 8);
    }
}
