//! Glue between the pure planners and the real system: process scan, state store, console injection.
use crate::config::Config;
use crate::console::{SendOptions, send_text};
use crate::logger::log;
use crate::proc::{self, ProcessInfo};
use crate::schedule::{Schedule, Verdict, verify_reset};
use crate::state::{StateStore, Status};
use crate::targets::{Backend, ForcePlan, Job, RunSummary, find_live_process, plan_force, run_jobs};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug)]
pub enum Injector {
    /// Call `AttachConsole` in this process (use only from a GUI-subsystem process).
    InProcess,
    /// Spawn `<exe> __inject …` without a console (use from console programs).
    Detached(PathBuf),
}

/// Folder name of the project plus the PID: enough to recognise a window, never any session content.
pub fn label(cwd: &str, pid: u32) -> String {
    let name = cwd
        .trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("?");
    format!("{name} (pid {pid})")
}

pub fn inject(injector: &Injector, pid: u32, text: &str, opts: &SendOptions) -> Result<(), String> {
    match injector {
        Injector::InProcess => send_text(pid, text, opts).map_err(|e| e.to_string()),
        Injector::Detached(exe) => inject_detached(exe, pid, text, opts),
    }
}

pub fn inject_detached(exe: &std::path::Path, pid: u32, text: &str, opts: &SendOptions) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let out = Command::new(exe)
        .arg("__inject")
        .arg(pid.to_string())
        .arg(opts.enter_delay_ms.to_string())
        .arg(if opts.raw_vk { "1" } else { "0" })
        .arg(if opts.press_enter { "1" } else { "0" })
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(DETACHED_PROCESS)
        .output()
        .map_err(|e| format!("cannot start injector: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub struct LiveBackend<'a> {
    pub store: &'a StateStore,
    pub injector: Injector,
    /// Manual run: ignores schedule rules (`running` gets the message queued; `needs_input` only if allowed).
    pub manual: bool,
    pub include_waiting: bool,
    pub own: HashSet<u32>,
}

impl Backend for LiveBackend<'_> {
    fn send(&self, job: &Job, text: &str, enter_delay_ms: u64) -> Result<(), String> {
        let opts = SendOptions {
            enter_delay_ms,
            ..Default::default()
        };
        inject(&self.injector, job.pid, text, &opts)
    }

    fn sleep(&self, secs: u64) {
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }

    fn recheck(&self, job: &Job) -> Option<String> {
        if self.own.contains(&job.pid) {
            return Some("own_session".into());
        }
        if !proc::still_claude(job.pid, job.started) {
            return Some("gone".into());
        }
        let st = job.session_id.as_deref().and_then(|id| self.store.load(id))?;
        match st.status {
            Status::NeedsInput if !(self.manual && self.include_waiting) => return Some("needs_input".into()),
            Status::Running if !self.manual => return Some("running".into()),
            _ => {}
        }
        if !self.manual
            && let Some(r) = job.reset_epoch
        {
            if st.last_nudged_epoch == r {
                return Some("already_nudged".into());
            }
            if let Verdict::StillLimited { .. } = verify_reset(&st, r, crate::now_epoch()) {
                return Some("limit_active".into());
            }
        }
        None
    }

    fn mark_nudged(&self, job: &Job) {
        if let (Some(id), Some(r)) = (&job.session_id, job.reset_epoch) {
            let _ = self.store.update(id, |s| s.last_nudged_epoch = r);
        }
    }
}

/// Scan processes and build the manual-run plan. Nothing is sent.
pub fn prepare_force(cfg: &Config, store: &StateStore, include_waiting: bool) -> ForcePlan {
    let procs = proc::scan_claude();
    plan_force(
        &procs,
        &store.list(),
        &proc::own_ancestors(),
        cfg,
        include_waiting,
        crate::now_epoch(),
    )
}

pub fn execute_force(
    plan: &ForcePlan,
    cfg: &Config,
    store: &StateStore,
    injector: Injector,
    include_waiting: bool,
    dry_run: bool,
    cancel: &AtomicBool,
) -> RunSummary {
    let jobs: Vec<Job> = plan
        .send
        .iter()
        .map(|t| Job {
            pid: t.pid,
            started: t.started,
            label: label(&t.cwd, t.pid),
            session_id: t.session_id.clone(),
            reset_epoch: None,
        })
        .collect();
    let backend = LiveBackend {
        store,
        injector,
        manual: true,
        include_waiting,
        own: proc::own_ancestors(),
    };
    log(&format!(
        "manual run: found={} targets={} skipped={} dry_run={dry_run}",
        plan.found,
        plan.send.len(),
        plan.skipped.len()
    ));
    let sum = run_jobs(&jobs, cfg, dry_run, &backend, cancel);
    log_summary("manual run", &sum);
    sum
}

/// Scheduled wake-up of the sessions in `sched.due`.
pub fn execute_due(
    sched: &Schedule,
    cfg: &Config,
    store: &StateStore,
    injector: Injector,
    cancel: &AtomicBool,
) -> RunSummary {
    let live: Vec<ProcessInfo> = proc::scan_claude();
    let mut sum_gone = Vec::new();
    let mut jobs = Vec::new();
    for d in &sched.due {
        let Some(st) = store.load(&d.session_id) else { continue };
        match find_live_process(&st, &live) {
            Some(p) => jobs.push(Job {
                pid: p.pid,
                started: p.started,
                label: label(&st.cwd, p.pid),
                session_id: Some(st.session_id.clone()),
                reset_epoch: Some(d.reset_epoch),
            }),
            None => sum_gone.push((label(&st.cwd, st.claude_pid.unwrap_or(0)), "gone".to_string())),
        }
    }
    let backend = LiveBackend {
        store,
        injector,
        manual: false,
        include_waiting: false,
        own: proc::own_ancestors(),
    };
    log(&format!(
        "scheduled wake-up: due={} dry_run={}",
        jobs.len(),
        cfg.dry_run
    ));
    let mut sum = run_jobs(&jobs, cfg, cfg.dry_run, &backend, cancel);
    sum.skipped.extend(sum_gone);
    log_summary("scheduled wake-up", &sum);
    sum
}

fn log_summary(what: &str, s: &RunSummary) {
    log(&format!(
        "{what}: sent={} would_send={} skipped={} failed={}{}",
        s.sent.len(),
        s.would_send.len(),
        s.skipped.len(),
        s.failed.len(),
        if s.cancelled { " (cancelled)" } else { "" }
    ));
    for l in &s.sent {
        log(&format!("  sent: {l}"));
    }
    for l in &s.would_send {
        log(&format!("  dry-run, would send: {l}"));
    }
    for (l, why) in &s.skipped {
        log(&format!("  skipped: {l}: {why}"));
    }
    for (l, e) in &s.failed {
        log(&format!("  failed: {l}: {e}"));
    }
}
