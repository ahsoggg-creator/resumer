//! Hook entry points. They must be fast and must never fail: every error is swallowed (and logged).
use resumer_core::config::Config;
use resumer_core::hooks::{self, HookEnv, HookKind};
use resumer_core::state::StateStore;
use resumer_core::{logger, proc};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn read_stdin(limit: Duration) -> String {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::stdin().read_to_string(&mut s);
        let _ = tx.send(s);
    });
    rx.recv_timeout(limit).unwrap_or_default()
}

pub fn run(kind: HookKind) {
    let payload = read_stdin(Duration::from_millis(2000));
    let result = std::panic::catch_unwind(|| handle(kind, &payload));
    if result.is_err() {
        logger::log("hook panicked");
    }
    if kind == HookKind::Statusline {
        let out = result.unwrap_or_default();
        if !out.is_empty() {
            let _ = std::io::stdout().write_all(out.as_bytes());
        }
    }
}

/// Returns the statusLine text (empty for other hooks).
fn handle(kind: HookKind, payload: &str) -> String {
    let store = StateStore::default_store();
    let env = HookEnv {
        now: resumer_core::now_epoch(),
        detect_claude: &|| proc::claude_ancestor_of(proc::current_pid()),
        is_alive: &proc::is_alive,
    };
    let state = hooks::handle(kind, payload, &store, &env);
    if kind != HookKind::Statusline {
        return String::new();
    }
    let cfg = Config::load();
    if let Some(prev) = cfg.chained_statusline.as_deref().filter(|c| !c.is_empty())
        && let Some(out) = run_chained(prev, payload)
    {
        return out;
    }
    state.map(|s| hooks::statusline_text(&s)).unwrap_or_default()
}

/// Runs the statusLine command that was configured before Resumer, with the same stdin.
fn run_chained(command: &str, payload: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = match git_bash() {
        Some(bash) => {
            let mut c = Command::new(bash);
            c.arg("-c").arg(command);
            c
        }
        None => {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(command);
            c
        }
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(payload.as_bytes());
    }
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let start = Instant::now();
    loop {
        if let Ok(buf) = rx.try_recv() {
            let _ = child.wait();
            return Some(String::from_utf8_lossy(&buf).into_owned());
        }
        if start.elapsed() > Duration::from_secs(4) {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn git_bash() -> Option<std::path::PathBuf> {
    let mut cands: Vec<std::path::PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH") {
        cands.push(p.into());
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            let base = std::path::PathBuf::from(base);
            cands.push(base.join("Git").join("bin").join("bash.exe"));
            cands.push(base.join("Programs").join("Git").join("bin").join("bash.exe"));
        }
    }
    cands.into_iter().find(|p| p.is_file())
}
