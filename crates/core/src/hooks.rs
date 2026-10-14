//! Pure handling of Claude Code hook / statusLine payloads. Only the documented fields are read;
//! `transcript_path` and message texts are never touched.
use crate::state::{SessionState, StateStore, Status, Window};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookKind {
    Statusline,
    StopFailure,
    Notification,
    /// `UserPromptSubmit` / `Stop` / `SessionEnd`; the event is taken from `hook_event_name`.
    Activity,
}

fn window_from(v: &Value) -> Option<Window> {
    let used = v.get("used_percentage")?.as_f64()?;
    let resets = v
        .get("resets_at")
        .and_then(|r| r.as_i64().or_else(|| r.as_f64().map(|f| f as i64)))?;
    Some(Window {
        used_percentage: used,
        resets_at: resets,
    })
}

/// StatusLine JSON: remember the latest limits. Windows that are absent keep their stored values.
pub fn apply_statusline(st: &mut SessionState, v: &Value, now: i64) {
    if let Some(cwd) = v
        .get("cwd")
        .and_then(Value::as_str)
        .or_else(|| v.pointer("/workspace/current_dir").and_then(Value::as_str))
    {
        st.cwd = cwd.to_string();
    }
    if let Some(w) = v.pointer("/rate_limits/five_hour").and_then(window_from) {
        st.five_hour = Some(w);
    }
    if let Some(w) = v.pointer("/rate_limits/seven_day").and_then(window_from) {
        st.seven_day = Some(w);
    }
    if let Some(r) = crate::schedule::exhausted_reset(st, now) {
        st.blocked_until = r;
    }
    st.updated_at = now;
}

/// Returns `true` when the event means the session is gone and its state should be deleted.
pub fn apply_event(st: &mut SessionState, v: &Value, now: i64) -> bool {
    if let Some(cwd) = v.get("cwd").and_then(Value::as_str) {
        st.cwd = cwd.to_string();
    }
    st.updated_at = now;
    let ev = v.get("hook_event_name").and_then(Value::as_str).unwrap_or("");
    match ev {
        "SessionEnd" => return true,
        "UserPromptSubmit" => {
            st.status = Status::Running;
            st.blocked_until = 0;
        }
        "Stop" => {
            st.status = Status::Idle;
            st.blocked_until = 0;
        }
        "StopFailure" => {
            if v.get("error").and_then(Value::as_str) == Some("rate_limit") {
                st.status = Status::RateLimited;
                st.blocked_until = crate::schedule::guess_blocked_until(st, now);
            } else {
                st.status = Status::Idle;
            }
        }
        "Notification" => match v.get("notification_type").and_then(Value::as_str).unwrap_or("") {
            "permission_prompt" | "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
                st.status = Status::NeedsInput
            }
            "idle_prompt" if st.status != Status::RateLimited => st.status = Status::Idle,
            // Claude Code's own auto-continue took over: we must not type on top of it.
            "quota_auto_resume_fired" => {
                st.status = Status::Running;
                st.blocked_until = 0;
            }
            "quota_auto_resume_stale" | "quota_auto_resume_disabled" => st.status = Status::RateLimited,
            _ => {}
        },
        _ => {}
    }
    false
}

pub struct HookEnv<'a> {
    pub now: i64,
    /// Finds `(pid, creation time)` of the Claude Code process above this hook process.
    pub detect_claude: &'a dyn Fn() -> Option<(u32, u64)>,
    pub is_alive: &'a dyn Fn(u32, Option<u64>) -> bool,
}

/// Applies one hook payload to the store. Returns the saved state (`None` when removed/ignored).
pub fn handle(kind: HookKind, payload: &str, store: &StateStore, env: &HookEnv) -> Option<SessionState> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let id = v.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let mut remove = false;
    let saved = store.update(id, |st| {
        match kind {
            HookKind::Statusline => apply_statusline(st, &v, env.now),
            _ => remove = apply_event(st, &v, env.now),
        }
        let known = st.claude_pid.is_some_and(|p| (env.is_alive)(p, st.claude_started));
        if !known && let Some((pid, started)) = (env.detect_claude)() {
            st.claude_pid = Some(pid);
            st.claude_started = Some(started);
        }
    });
    if remove {
        store.remove(id);
        return None;
    }
    saved.ok()
}

/// One short line for the status bar when there is no previous statusLine to chain to.
pub fn statusline_text(st: &SessionState) -> String {
    let fmt = |label: &str, w: &Option<Window>| w.map(|w| format!("{label} {:.0}%", w.used_percentage));
    [fmt("5h", &st.five_hour), fmt("7d", &st.seven_day)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env<'a>(detect: &'a dyn Fn() -> Option<(u32, u64)>) -> HookEnv<'a> {
        HookEnv {
            now: 1000,
            detect_claude: detect,
            is_alive: &|_, _| true,
        }
    }

    fn store() -> (tempfile::TempDir, StateStore) {
        let d = tempfile::tempdir().unwrap();
        let s = StateStore::new(d.path().join("s"));
        (d, s)
    }

    #[test]
    fn statusline_stores_limits_and_keeps_missing_window() {
        let (_d, s) = store();
        let p1 = json!({"session_id":"a","cwd":"C:\\p","rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},
            "seven_day":{"used_percentage":41.2,"resets_at":1738857600}}});
        let st = handle(HookKind::Statusline, &p1.to_string(), &s, &env(&|| Some((7, 70)))).unwrap();
        assert_eq!(st.five_hour.unwrap().resets_at, 1738425600);
        assert_eq!((st.claude_pid, st.claude_started), (Some(7), Some(70)));
        let p2 = json!({"session_id":"a","rate_limits":{"seven_day":{"used_percentage":50,"resets_at":1738857600}}});
        let st = handle(HookKind::Statusline, &p2.to_string(), &s, &env(&|| None)).unwrap();
        assert_eq!(
            st.five_hour.unwrap().used_percentage,
            23.5,
            "absent window must not erase stored one"
        );
        assert_eq!(st.seven_day.unwrap().used_percentage, 50.0);
        assert_eq!(st.cwd, "C:\\p");
    }

    #[test]
    fn event_transitions() {
        let (_d, s) = store();
        let h = |ev: Value| {
            handle(HookKind::Activity, &ev.to_string(), &s, &env(&|| None))
                .unwrap()
                .status
        };
        assert_eq!(
            h(json!({"session_id":"a","hook_event_name":"UserPromptSubmit"})),
            Status::Running
        );
        assert_eq!(h(json!({"session_id":"a","hook_event_name":"Stop"})), Status::Idle);
        assert_eq!(
            h(json!({"session_id":"a","hook_event_name":"StopFailure","error":"rate_limit"})),
            Status::RateLimited
        );
        assert_eq!(
            h(json!({"session_id":"a","hook_event_name":"Notification","notification_type":"idle_prompt"})),
            Status::RateLimited
        );
        assert_eq!(
            h(json!({"session_id":"a","hook_event_name":"Notification","notification_type":"quota_auto_resume_fired"})),
            Status::Running
        );
        assert_eq!(
            h(json!({"session_id":"a","hook_event_name":"Notification","notification_type":"permission_prompt"})),
            Status::NeedsInput
        );
        assert_eq!(
            h(json!({"session_id":"a","hook_event_name":"StopFailure","error":"overloaded"})),
            Status::Idle
        );
        assert_eq!(
            h(json!({"session_id":"a","hook_event_name":"Notification","notification_type":"auth_success"})),
            Status::Idle
        );
    }

    #[test]
    fn session_end_removes_state() {
        let (_d, s) = store();
        handle(
            HookKind::Activity,
            &json!({"session_id":"a","hook_event_name":"Stop"}).to_string(),
            &s,
            &env(&|| None),
        );
        assert!(s.load("a").is_some());
        let r = handle(
            HookKind::Activity,
            &json!({"session_id":"a","hook_event_name":"SessionEnd"}).to_string(),
            &s,
            &env(&|| None),
        );
        assert!(r.is_none() && s.load("a").is_none());
    }

    #[test]
    fn garbage_input_is_ignored() {
        let (_d, s) = store();
        assert!(handle(HookKind::Statusline, "not json", &s, &env(&|| None)).is_none());
        assert!(handle(HookKind::Statusline, "{}", &s, &env(&|| None)).is_none());
        assert!(s.list().is_empty());
    }

    #[test]
    fn dead_pid_is_redetected() {
        let (_d, s) = store();
        let dead = HookEnv {
            now: 1,
            detect_claude: &|| Some((9, 90)),
            is_alive: &|p, _| p == 9,
        };
        s.update("a", |st| st.claude_pid = Some(1)).unwrap();
        let st = handle(
            HookKind::Activity,
            &json!({"session_id":"a","hook_event_name":"Stop"}).to_string(),
            &s,
            &dead,
        )
        .unwrap();
        assert_eq!(st.claude_pid, Some(9));
    }

    #[test]
    fn statusline_text_formats() {
        let mut st = SessionState::new("a");
        assert_eq!(statusline_text(&st), "");
        st.five_hour = Some(Window {
            used_percentage: 23.4,
            resets_at: 1,
        });
        st.seven_day = Some(Window {
            used_percentage: 41.6,
            resets_at: 2,
        });
        assert_eq!(statusline_text(&st), "5h 23% · 7d 42%");
    }
}
