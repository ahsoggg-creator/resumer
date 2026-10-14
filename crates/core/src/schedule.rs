//! Wake-up planning. Pure functions of `(states, config, now)`: no polling, no clocks inside.
//! The daemon sleeps until `Schedule::next_wake`, then asks again with the real wall-clock time, so a
//! suspend/hibernate in between only makes the answer "due now".
use crate::config::{Config, TargetMode};
use crate::state::{SessionState, Status};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    NotWhitelisted,
    Excluded,
    NeedsInput,
    Running,
    AlreadyNudged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    pub session_id: String,
    /// The reset moment this wake-up answers; stored into `last_nudged_epoch` after sending.
    pub reset_epoch: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schedule {
    /// Sessions to type into right now.
    pub due: Vec<Due>,
    pub skipped: Vec<(String, SkipReason)>,
    /// Unix seconds of the next moment worth waking up at (includes the safety margin). `None` = nothing pending.
    pub next_wake: Option<i64>,
    /// Limit is still active for a session whose time had come (fresh data says so): re-planned to `next_wake`.
    pub replanned: bool,
}

/// Reset time of the exhausted window(s) that are still in the future: the later one wins.
pub fn exhausted_reset(st: &SessionState, now: i64) -> Option<i64> {
    [st.five_hour, st.seven_day]
        .into_iter()
        .flatten()
        .filter(|w| w.exhausted() && w.resets_at > now)
        .map(|w| w.resets_at)
        .max()
}

/// At the moment a limit error arrives: which reset are we waiting for?
pub fn guess_blocked_until(st: &SessionState, now: i64) -> i64 {
    if let Some(r) = exhausted_reset(st, now) {
        return r;
    }
    // Neither window shows 100% (e.g. a per-model limit): the nearest future reset is the best guess.
    [st.five_hour, st.seven_day]
        .into_iter()
        .flatten()
        .map(|w| w.resets_at)
        .filter(|&r| r > now)
        .min()
        .unwrap_or(0)
}

/// The reset moment this session is waiting for, if any.
pub fn pending_reset(st: &SessionState, now: i64) -> Option<i64> {
    if st.status == Status::RateLimited {
        if st.blocked_until > 0 {
            return Some(st.blocked_until);
        }
        // Percentages only (older state files / hooks installed after the limit hit).
        return [st.five_hour, st.seven_day]
            .into_iter()
            .flatten()
            .filter(|w| w.exhausted() && w.resets_at > 0)
            .map(|w| w.resets_at)
            .max();
    }
    exhausted_reset(st, now)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ready,
    StillLimited { until: i64 },
}

/// Is the limit that ended at `reset` really over? Fresh data (written after the reset) can veto.
pub fn verify_reset(st: &SessionState, reset: i64, now: i64) -> Verdict {
    if now < reset {
        return Verdict::StillLimited { until: reset };
    }
    if st.updated_at >= reset
        && let Some(r2) = exhausted_reset(st, now)
    {
        return Verdict::StillLimited { until: r2 };
    }
    Verdict::Ready
}

pub fn plan(states: &[SessionState], cfg: &Config, now: i64) -> Schedule {
    let mut out = Schedule::default();
    if cfg.paused {
        return out;
    }
    let margin = cfg.margin_secs as i64;

    // Triggers: sessions that are waiting for a limit reset and were not nudged for it yet.
    let mut ripe: Vec<(&SessionState, i64)> = Vec::new();
    for s in states {
        let Some(r) = pending_reset(s, now) else { continue };
        if cfg.target_mode == TargetMode::Blocked && s.status != Status::RateLimited {
            continue;
        }
        if s.last_nudged_epoch == r || s.status == Status::Running || s.status == Status::NeedsInput {
            continue;
        }
        if cfg.is_excluded(&s.cwd) || !cfg.is_whitelisted(&s.cwd) {
            continue;
        }
        let at = r + margin;
        if now < at {
            out.next_wake = min_opt(out.next_wake, at);
            continue;
        }
        match verify_reset(s, r, now) {
            Verdict::Ready => ripe.push((s, r)),
            Verdict::StillLimited { until } => {
                out.next_wake = min_opt(out.next_wake, until + margin);
                out.replanned = true;
            }
        }
    }
    if ripe.is_empty() {
        return out;
    }

    // The reset that fired; in all-known mode it is the key under which every target gets marked.
    let fired = ripe.iter().map(|(_, r)| *r).max().unwrap_or(0);
    for s in states {
        let key = ripe.iter().find(|(t, _)| t.session_id == s.session_id).map(|(_, r)| *r);
        let is_trigger = key.is_some();
        if !is_trigger && cfg.target_mode == TargetMode::Blocked {
            continue;
        }
        let reset_epoch = key.unwrap_or(fired);
        let skip = if cfg.is_excluded(&s.cwd) {
            Some(SkipReason::Excluded)
        } else if !cfg.is_whitelisted(&s.cwd) {
            Some(SkipReason::NotWhitelisted)
        } else if s.status == Status::NeedsInput {
            Some(SkipReason::NeedsInput)
        } else if s.status == Status::Running {
            Some(SkipReason::Running)
        } else if s.last_nudged_epoch == reset_epoch {
            Some(SkipReason::AlreadyNudged)
        } else {
            None
        };
        match skip {
            Some(r) => out.skipped.push((s.session_id.clone(), r)),
            None => out.due.push(Due {
                session_id: s.session_id.clone(),
                reset_epoch,
            }),
        }
    }
    out
}

fn min_opt(a: Option<i64>, b: i64) -> Option<i64> {
    Some(a.map_or(b, |a| a.min(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Window;

    const NOW: i64 = 1_000_000;

    fn w(used: f64, resets_at: i64) -> Option<Window> {
        Some(Window {
            used_percentage: used,
            resets_at,
        })
    }

    fn sess(id: &str, status: Status, five: Option<Window>, week: Option<Window>) -> SessionState {
        SessionState {
            session_id: id.into(),
            cwd: format!("C:\\proj\\{id}"),
            status,
            five_hour: five,
            seven_day: week,
            updated_at: NOW - 100,
            ..Default::default()
        }
    }

    fn cfg() -> Config {
        Config {
            whitelist: vec!["C:\\proj".into()],
            margin_secs: 10,
            ..Default::default()
        }
    }

    #[test]
    fn nothing_exhausted_means_no_wake() {
        let s = sess("a", Status::Idle, w(20.0, NOW + 100), w(5.0, NOW + 9000));
        assert_eq!(plan(&[s], &cfg(), NOW), Schedule::default());
    }

    #[test]
    fn five_hour_exhausted_wakes_after_its_reset() {
        let s = sess("a", Status::RateLimited, w(100.0, NOW + 3600), w(30.0, NOW + 90_000));
        let p = plan(&[s], &cfg(), NOW);
        assert_eq!(p.next_wake, Some(NOW + 3610));
        assert!(p.due.is_empty());
    }

    #[test]
    fn weekly_exhausted_waits_for_the_week() {
        let s = sess("a", Status::RateLimited, w(40.0, NOW + 3600), w(100.0, NOW + 200_000));
        assert_eq!(plan(&[s], &cfg(), NOW).next_wake, Some(NOW + 200_010));
    }

    #[test]
    fn both_exhausted_waits_for_the_later_reset() {
        let s = sess("a", Status::RateLimited, w(100.0, NOW + 3600), w(100.0, NOW + 200_000));
        assert_eq!(plan(&[s], &cfg(), NOW).next_wake, Some(NOW + 200_010));
    }

    #[test]
    fn due_exactly_after_margin_and_only_once() {
        let mut s = sess("a", Status::RateLimited, w(100.0, NOW + 60), None);
        assert!(plan(std::slice::from_ref(&s), &cfg(), NOW + 69).due.is_empty());
        let p = plan(std::slice::from_ref(&s), &cfg(), NOW + 70);
        assert_eq!(
            p.due,
            vec![Due {
                session_id: "a".into(),
                reset_epoch: NOW + 60
            }]
        );
        s.last_nudged_epoch = NOW + 60;
        let p = plan(&[s], &cfg(), NOW + 500);
        assert!(
            p.due.is_empty() && p.next_wake.is_none(),
            "never nudge twice for the same reset"
        );
    }

    #[test]
    fn survives_sleep_jump() {
        let s = sess("a", Status::RateLimited, w(100.0, NOW + 3600), None);
        let before = plan(std::slice::from_ref(&s), &cfg(), NOW);
        assert_eq!(before.next_wake, Some(NOW + 3610));
        // computer slept 5 hours: the very next plan() is simply "due"
        let after = plan(&[s], &cfg(), NOW + 5 * 3600);
        assert_eq!(after.due.len(), 1);
    }

    #[test]
    fn blocked_until_survives_post_reset_statusline_data() {
        let mut s = sess("a", Status::RateLimited, w(3.0, NOW + 18_000), None); // fresh window after reset
        s.blocked_until = NOW - 100;
        s.updated_at = NOW;
        assert_eq!(plan(&[s], &cfg(), NOW).due.len(), 1);
    }

    #[test]
    fn fresh_data_showing_limit_replans() {
        let mut s = sess("a", Status::RateLimited, w(100.0, NOW + 900), None);
        s.blocked_until = NOW - 60;
        s.updated_at = NOW;
        let p = plan(&[s], &cfg(), NOW);
        assert!(p.due.is_empty() && p.replanned);
        assert_eq!(p.next_wake, Some(NOW + 910));
    }

    #[test]
    fn running_and_needs_input_never_receive_anything() {
        for st in [Status::Running, Status::NeedsInput] {
            let mut s = sess("a", st, w(100.0, NOW - 100), None);
            s.blocked_until = NOW - 100;
            let p = plan(&[s], &cfg(), NOW);
            assert!(p.due.is_empty());
        }
        // all_known mode: a rate-limited trigger fires, but waiting / running neighbours are skipped and reported
        let trig = {
            let mut s = sess("t", Status::RateLimited, w(100.0, NOW - 100), None);
            s.blocked_until = NOW - 100;
            s
        };
        let waiting = sess("w", Status::NeedsInput, None, None);
        let busy = sess("r", Status::Running, None, None);
        let idle = sess("i", Status::Idle, None, None);
        let mut c = cfg();
        c.target_mode = TargetMode::AllKnown;
        let p = plan(&[trig, waiting, busy, idle], &c, NOW);
        let ids: Vec<&str> = p.due.iter().map(|d| d.session_id.as_str()).collect();
        assert_eq!(ids, vec!["t", "i"]);
        assert!(p.skipped.contains(&("w".into(), SkipReason::NeedsInput)));
        assert!(p.skipped.contains(&("r".into(), SkipReason::Running)));
        assert!(p.due.iter().all(|d| d.reset_epoch == NOW - 100));
    }

    #[test]
    fn blocked_mode_ignores_unmarked_sessions() {
        let blocked = {
            let mut s = sess("b", Status::RateLimited, w(100.0, NOW - 100), None);
            s.blocked_until = NOW - 100;
            s
        };
        let idle = sess("i", Status::Idle, w(10.0, NOW + 1), None);
        let p = plan(&[blocked, idle], &cfg(), NOW);
        assert_eq!(p.due.len(), 1);
        assert_eq!(p.due[0].session_id, "b");
    }

    #[test]
    fn whitelist_and_exclusions_apply() {
        let mut s = sess("a", Status::RateLimited, w(100.0, NOW - 100), None);
        s.blocked_until = NOW - 100;
        let mut c = cfg();
        c.whitelist.clear();
        assert!(
            plan(std::slice::from_ref(&s), &c, NOW).due.is_empty(),
            "empty whitelist = nobody is touched"
        );
        c.whitelist.push("C:\\proj".into());
        c.excluded.push("C:\\proj\\a".into());
        assert!(plan(std::slice::from_ref(&s), &c, NOW).due.is_empty());
        c.excluded.clear();
        assert_eq!(plan(&[s], &c, NOW).due.len(), 1);
    }

    #[test]
    fn paused_plans_nothing() {
        let mut s = sess("a", Status::RateLimited, w(100.0, NOW - 100), None);
        s.blocked_until = NOW - 100;
        let mut c = cfg();
        c.paused = true;
        assert_eq!(plan(&[s], &c, NOW), Schedule::default());
    }

    #[test]
    fn next_wake_is_the_earliest_of_several() {
        let a = sess("a", Status::RateLimited, w(100.0, NOW + 5000), None);
        let b = sess("b", Status::RateLimited, w(100.0, NOW + 700), None);
        assert_eq!(plan(&[a, b], &cfg(), NOW).next_wake, Some(NOW + 710));
    }

    #[test]
    fn guess_for_unknown_limit_uses_nearest_reset() {
        let s = sess("a", Status::Idle, w(60.0, NOW + 100), w(10.0, NOW + 9000));
        assert_eq!(guess_blocked_until(&s, NOW), NOW + 100);
        let s2 = sess("a", Status::Idle, None, None);
        assert_eq!(guess_blocked_until(&s2, NOW), 0);
    }
}
