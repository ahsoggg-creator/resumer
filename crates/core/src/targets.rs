//! Target selection for the manual "wake everything now" run, plus the shared send loop.
use crate::config::Config;
use crate::proc::ProcessInfo;
use crate::schedule::pending_reset;
use crate::state::{SessionState, Status};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub pid: u32,
    pub started: u64,
    pub cwd: String,
    pub session_id: Option<String>,
    pub status: Option<Status>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForceSkip {
    /// The session where Resumer (or the agent that launched it) runs.
    OwnSession,
    NeedsInput,
    Excluded,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForcePlan {
    pub found: usize,
    pub send: Vec<Target>,
    pub skipped: Vec<(Target, ForceSkip)>,
    /// Some live session is still limited until this moment (unix seconds).
    pub limit_until: Option<i64>,
}

/// The state file that belongs to this process (same pid and creation time), most recently updated first.
pub fn state_for<'a>(p: &ProcessInfo, states: &'a [SessionState]) -> Option<&'a SessionState> {
    states
        .iter()
        .filter(|s| s.claude_pid == Some(p.pid))
        .filter(|s| match (s.claude_started, p.started) {
            (Some(a), b) if a != 0 && b != 0 => a == b,
            _ => true,
        })
        .max_by_key(|s| s.updated_at)
}

/// Manual run: everything found by the process scan, independent of schedule, status and `last_nudged_epoch`.
pub fn plan_force(
    procs: &[ProcessInfo],
    states: &[SessionState],
    own: &HashSet<u32>,
    cfg: &Config,
    include_waiting: bool,
    now: i64,
) -> ForcePlan {
    let mut plan = ForcePlan {
        found: procs.len(),
        ..Default::default()
    };
    for p in procs {
        let st = state_for(p, states);
        let cwd = st
            .map(|s| s.cwd.clone())
            .filter(|c| !c.is_empty())
            .or_else(|| p.cwd.clone())
            .unwrap_or_default();
        let t = Target {
            pid: p.pid,
            started: p.started,
            cwd: cwd.clone(),
            session_id: st.map(|s| s.session_id.clone()),
            status: st.map(|s| s.status),
        };
        let proc_cwd_excluded = p.cwd.as_deref().is_some_and(|c| cfg.is_excluded(c));
        let skip = if own.contains(&p.pid) {
            Some(ForceSkip::OwnSession)
        } else if cfg.is_excluded(&cwd) || proc_cwd_excluded {
            Some(ForceSkip::Excluded)
        } else if t.status == Some(Status::NeedsInput) && !include_waiting {
            Some(ForceSkip::NeedsInput)
        } else {
            None
        };
        if skip.is_none()
            && let Some(r) = st.and_then(|s| pending_reset(s, now)).filter(|&r| r > now)
        {
            plan.limit_until = Some(plan.limit_until.map_or(r, |x| x.max(r)));
        }
        match skip {
            Some(s) => plan.skipped.push((t, s)),
            None => plan.send.push(t),
        }
    }
    plan
}

/// The live process a state refers to, or `None` if it is gone or the PID was reused.
pub fn find_live_process<'a>(st: &SessionState, live: &'a [ProcessInfo]) -> Option<&'a ProcessInfo> {
    let pid = st.claude_pid?;
    live.iter().find(|p| {
        p.pid == pid
            && (st
                .claude_started
                .is_none_or(|s| s == 0 || p.started == 0 || p.started == s))
    })
}

/// Pseudo-random pause between windows, `gap_min..=gap_max` seconds.
pub fn random_gap(cfg: &Config, seed: u64) -> u64 {
    let (lo, hi) = (
        cfg.gap_min_secs.min(cfg.gap_max_secs),
        cfg.gap_min_secs.max(cfg.gap_max_secs),
    );
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    lo + x % (hi - lo + 1)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub pid: u32,
    /// Process creation time (0 = unknown); a different value means the PID was reused.
    pub started: u64,
    pub label: String,
    pub session_id: Option<String>,
    /// For scheduled wake-ups: value to store into `last_nudged_epoch`.
    pub reset_epoch: Option<i64>,
}

pub trait Backend {
    fn send(&self, job: &Job, text: &str, enter_delay_ms: u64) -> Result<(), String>;
    fn sleep(&self, secs: u64);
    /// Last-moment safety check; `Some(reason)` cancels this job.
    fn recheck(&self, job: &Job) -> Option<String>;
    fn mark_nudged(&self, job: &Job);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub sent: Vec<String>,
    /// What would have been sent (dry run).
    pub would_send: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
    pub cancelled: bool,
}

pub fn run_jobs(jobs: &[Job], cfg: &Config, dry_run: bool, backend: &dyn Backend, cancel: &AtomicBool) -> RunSummary {
    let mut sum = RunSummary::default();
    let seed = crate::now_epoch() as u64;
    let mut first = true;
    for (i, job) in jobs.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            sum.cancelled = true;
            break;
        }
        if let Some(why) = backend.recheck(job) {
            sum.skipped.push((job.label.clone(), why));
            continue;
        }
        if dry_run {
            sum.would_send.push(job.label.clone());
            continue;
        }
        if !first {
            backend.sleep(random_gap(cfg, seed.wrapping_add(i as u64)));
            if cancel.load(Ordering::SeqCst) {
                sum.cancelled = true;
                break;
            }
        }
        first = false;
        match backend.send(job, &cfg.message, cfg.enter_delay_ms) {
            Ok(()) => {
                backend.mark_nudged(job);
                sum.sent.push(job.label.clone());
            }
            Err(e) => sum.failed.push((job.label.clone(), e)),
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Window;
    use std::cell::RefCell;

    const NOW: i64 = 5_000_000;

    fn proc(pid: u32, cwd: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: 1,
            name: "claude.exe".into(),
            started: pid as u64 * 10,
            cwd: Some(cwd.into()),
            ..Default::default()
        }
    }

    fn st(id: &str, pid: u32, status: Status, cwd: &str) -> SessionState {
        SessionState {
            session_id: id.into(),
            cwd: cwd.into(),
            claude_pid: Some(pid),
            claude_started: Some(pid as u64 * 10),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn force_covers_unknown_processes_ignores_schedule_and_skips_waiting_and_self() {
        let procs = vec![
            proc(1, "C:\\a"),
            proc(2, "C:\\b"),
            proc(3, "C:\\c"),
            proc(4, "C:\\d"),
            proc(5, "C:\\secret\\x"),
        ];
        let mut idle = st("s1", 1, Status::Idle, "C:\\a");
        idle.last_nudged_epoch = NOW + 1; // must not matter
        let states = vec![
            idle,
            st("s2", 2, Status::NeedsInput, "C:\\b"),
            st("s3", 3, Status::Running, "C:\\c"),
        ];
        let own: HashSet<u32> = [4].into_iter().collect();
        let mut cfg = Config::default();
        cfg.excluded.push("C:\\secret".into());
        let plan = plan_force(&procs, &states, &own, &cfg, false, NOW);
        assert_eq!(plan.found, 5);
        let sent: Vec<u32> = plan.send.iter().map(|t| t.pid).collect();
        assert_eq!(
            sent,
            vec![1, 3],
            "idle and running (queued) are sent; waiting, own session and excluded are skipped"
        );
        let skipped: Vec<(u32, ForceSkip)> = plan.skipped.iter().map(|(t, s)| (t.pid, s.clone())).collect();
        assert_eq!(
            skipped,
            vec![
                (2, ForceSkip::NeedsInput),
                (4, ForceSkip::OwnSession),
                (5, ForceSkip::Excluded)
            ]
        );
    }

    #[test]
    fn unknown_process_without_state_is_targeted() {
        let plan = plan_force(
            &[proc(9, "C:\\new")],
            &[],
            &HashSet::new(),
            &Config::default(),
            false,
            NOW,
        );
        assert_eq!(plan.send.len(), 1);
        assert_eq!(plan.send[0].session_id, None);
        assert_eq!(plan.send[0].cwd, "C:\\new");
    }

    #[test]
    fn include_waiting_setting_lets_waiting_sessions_in() {
        let states = vec![st("s2", 2, Status::NeedsInput, "C:\\b")];
        let plan = plan_force(
            &[proc(2, "C:\\b")],
            &states,
            &HashSet::new(),
            &Config::default(),
            true,
            NOW,
        );
        assert_eq!(plan.send.len(), 1);
    }

    #[test]
    fn whitelist_does_not_apply_to_manual_runs() {
        let mut cfg = Config::default();
        cfg.whitelist.push("C:\\only".into());
        let plan = plan_force(&[proc(1, "C:\\other")], &[], &HashSet::new(), &cfg, false, NOW);
        assert_eq!(plan.send.len(), 1);
    }

    #[test]
    fn reports_limit_that_is_still_active() {
        let mut s = st("s1", 1, Status::RateLimited, "C:\\a");
        s.five_hour = Some(Window {
            used_percentage: 100.0,
            resets_at: NOW + 600,
        });
        s.blocked_until = NOW + 600;
        let plan = plan_force(
            &[proc(1, "C:\\a")],
            &[s],
            &HashSet::new(),
            &Config::default(),
            false,
            NOW,
        );
        assert_eq!(plan.limit_until, Some(NOW + 600));
        let plan = plan_force(
            &[proc(1, "C:\\a")],
            &[],
            &HashSet::new(),
            &Config::default(),
            false,
            NOW,
        );
        assert_eq!(plan.limit_until, None);
    }

    #[test]
    fn pid_reuse_is_detected() {
        let s = st("s1", 7, Status::Idle, "C:\\a");
        let mut reused = proc(7, "C:\\zzz");
        reused.started = 999;
        assert!(find_live_process(&s, &[reused]).is_none());
        assert!(find_live_process(&s, &[proc(7, "C:\\a")]).is_some());
        assert!(find_live_process(&s, &[proc(8, "C:\\a")]).is_none());
    }

    #[test]
    fn gap_stays_in_range() {
        let cfg = Config {
            gap_min_secs: 20,
            gap_max_secs: 30,
            ..Default::default()
        };
        for seed in 0..500 {
            let g = random_gap(&cfg, seed);
            assert!((20..=30).contains(&g), "{g}");
        }
        let fixed = Config {
            gap_min_secs: 5,
            gap_max_secs: 5,
            ..Default::default()
        };
        assert_eq!(random_gap(&fixed, 3), 5);
    }

    struct Mock {
        log: RefCell<Vec<String>>,
        deny_recheck: Option<u32>,
        fail_pid: Option<u32>,
    }

    impl Backend for Mock {
        fn send(&self, job: &Job, text: &str, _d: u64) -> Result<(), String> {
            if self.fail_pid == Some(job.pid) {
                return Err("denied".into());
            }
            self.log
                .borrow_mut()
                .push(format!("send {} {}", job.pid, text.len() > 5));
            Ok(())
        }
        fn sleep(&self, secs: u64) {
            self.log.borrow_mut().push(format!("sleep {}", secs >= 20));
        }
        fn recheck(&self, job: &Job) -> Option<String> {
            (self.deny_recheck == Some(job.pid)).then(|| "needs_input".to_string())
        }
        fn mark_nudged(&self, job: &Job) {
            self.log.borrow_mut().push(format!("mark {}", job.pid));
        }
    }

    fn job(pid: u32) -> Job {
        Job {
            pid,
            started: 0,
            label: format!("p{pid}"),
            session_id: None,
            reset_epoch: Some(1),
        }
    }

    #[test]
    fn runner_sends_in_order_with_gaps_and_marks() {
        let m = Mock {
            log: RefCell::new(vec![]),
            deny_recheck: Some(2),
            fail_pid: Some(3),
        };
        let sum = run_jobs(
            &[job(1), job(2), job(3), job(4)],
            &Config::default(),
            false,
            &m,
            &AtomicBool::new(false),
        );
        assert_eq!(sum.sent, vec!["p1", "p4"]);
        assert_eq!(sum.skipped, vec![("p2".to_string(), "needs_input".to_string())]);
        assert_eq!(sum.failed, vec![("p3".to_string(), "denied".to_string())]);
        assert_eq!(
            m.log.borrow().as_slice(),
            [
                "send 1 true",
                "mark 1",
                "sleep true",
                "sleep true",
                "send 4 true",
                "mark 4"
            ]
        );
    }

    #[test]
    fn dry_run_types_nothing() {
        let m = Mock {
            log: RefCell::new(vec![]),
            deny_recheck: None,
            fail_pid: None,
        };
        let sum = run_jobs(&[job(1), job(2)], &Config::default(), true, &m, &AtomicBool::new(false));
        assert_eq!(sum.would_send, vec!["p1", "p2"]);
        assert!(sum.sent.is_empty());
        assert!(m.log.borrow().is_empty(), "no send, no mark, no sleep");
    }

    #[test]
    fn cancel_stops_the_run() {
        let m = Mock {
            log: RefCell::new(vec![]),
            deny_recheck: None,
            fail_pid: None,
        };
        let sum = run_jobs(&[job(1)], &Config::default(), false, &m, &AtomicBool::new(true));
        assert!(sum.cancelled && sum.sent.is_empty());
    }
}
