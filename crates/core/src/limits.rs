//! Account-level limits (the 5-hour and weekly windows are shared by all sessions).
//! Each window comes from the session that reported it most recently; windows whose reset time has
//! passed are dropped, so stale numbers are never shown.
use crate::config::Lang;
use crate::i18n::{duration, fmt, t};
use crate::state::{SessionState, Window};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AccountLimits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

/// A reading from Anthropic's usage endpoint (the hourly cross-check).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ApiSnapshot {
    pub fetched_at: i64,
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

fn freshest(
    states: &[SessionState],
    api: Option<(i64, Option<Window>)>,
    now: i64,
    pick: impl Fn(&SessionState) -> Option<Window>,
) -> Option<Window> {
    states
        .iter()
        .filter_map(|s| pick(s).map(|w| (s.updated_at, w)))
        .chain(api.and_then(|(at, w)| w.map(|w| (at, w))))
        .filter(|(_, w)| w.resets_at > now)
        .max_by_key(|(updated, _)| *updated)
        .map(|(_, w)| w)
}

/// The newest reading wins, whether it came from a session's status line or from the usage endpoint.
pub fn account_limits(states: &[SessionState], api: Option<&ApiSnapshot>, now: i64) -> AccountLimits {
    AccountLimits {
        five_hour: freshest(states, api.map(|a| (a.fetched_at, a.five_hour)), now, |s| s.five_hour),
        seven_day: freshest(states, api.map(|a| (a.fetched_at, a.seven_day)), now, |s| s.seven_day),
    }
}

/// Percent left, 0..=100.
pub fn remaining(w: &Window) -> f64 {
    (100.0 - w.used_percentage).clamp(0.0, 100.0)
}

fn part(lang: Lang, label: &str, w: &Option<Window>, now: i64) -> String {
    match w {
        Some(w) => format!(
            "{label}: {} {:.0}% · {} {}",
            t(lang, "limits.left"),
            remaining(w),
            t(lang, "limits.resets_in"),
            duration(lang, w.resets_at - now)
        ),
        None => format!("{label}: {}", t(lang, "limits.no_data")),
    }
}

/// "5 h: left 82% · resets in 2 h 43 m; week: left 97% · resets in 1 d 17 h"
pub fn describe(lang: Lang, l: &AccountLimits, now: i64) -> String {
    let (a, b) = if lang == Lang::Ru {
        ("5 ч", "неделя")
    } else {
        ("5 h", "week")
    };
    format!(
        "{}; {}",
        part(lang, a, &l.five_hour, now),
        part(lang, b, &l.seven_day, now)
    )
}

/// Short tray tooltip (Windows limits tooltips to 127 UTF-16 units).
pub fn tooltip(lang: Lang, l: &AccountLimits, now: i64) -> String {
    let (Some(f), Some(w)) = (l.five_hour, l.seven_day) else {
        return t(lang, "tray.tooltip.idle").to_string();
    };
    fmt(
        lang,
        "tray.limits",
        &[
            ("five", format!("{:.0}", remaining(&f))),
            ("five_in", duration(lang, f.resets_at - now)),
            ("week", format!("{:.0}", remaining(&w))),
            ("week_in", duration(lang, w.resets_at - now)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn st(id: &str, updated: i64, five: Option<Window>, week: Option<Window>) -> SessionState {
        SessionState {
            session_id: id.into(),
            updated_at: updated,
            five_hour: five,
            seven_day: week,
            ..Default::default()
        }
    }

    fn w(used: f64, resets_in: i64) -> Option<Window> {
        Some(Window {
            used_percentage: used,
            resets_at: NOW + resets_in,
        })
    }

    #[test]
    fn takes_the_freshest_session_per_window() {
        let a = st("a", NOW - 500, w(10.0, 3000), w(5.0, 90_000));
        let b = st("b", NOW - 10, w(40.0, 3000), None);
        let l = account_limits(&[a, b], None, NOW);
        assert_eq!(l.five_hour.unwrap().used_percentage, 40.0);
        assert_eq!(
            l.seven_day.unwrap().used_percentage,
            5.0,
            "week only known from the older session"
        );
    }

    #[test]
    fn expired_windows_are_dropped() {
        let a = st("a", NOW - 10, w(99.0, -5), w(5.0, 100));
        let l = account_limits(&[a], None, NOW);
        assert!(l.five_hour.is_none());
        assert!(l.seven_day.is_some());
    }

    #[test]
    fn newer_api_reading_beats_stale_session_data() {
        let a = st("a", NOW - 3000, w(18.0, 3000), w(3.0, 90_000));
        let api = ApiSnapshot {
            fetched_at: NOW - 5,
            five_hour: w(95.0, 3000),
            seven_day: None,
        };
        let l = account_limits(std::slice::from_ref(&a), Some(&api), NOW);
        assert_eq!(l.five_hour.unwrap().used_percentage, 95.0);
        assert_eq!(
            l.seven_day.unwrap().used_percentage,
            3.0,
            "api had no week: keep the session value"
        );
        let old_api = ApiSnapshot {
            fetched_at: NOW - 9000,
            ..api
        };
        let l = account_limits(&[a], Some(&old_api), NOW);
        assert_eq!(l.five_hour.unwrap().used_percentage, 18.0, "session reading is newer");
    }

    #[test]
    fn describes_remaining_not_used() {
        let l = AccountLimits {
            five_hour: w(18.0, 9780),
            seven_day: w(3.0, 148_000),
        };
        let s = describe(Lang::En, &l, NOW);
        assert!(s.contains("82%"), "{s}");
        assert!(s.contains("97%"), "{s}");
        assert!(describe(Lang::Ru, &AccountLimits::default(), NOW).contains("нет данных"));
    }

    #[test]
    fn tooltip_fits_windows_limit() {
        let l = AccountLimits {
            five_hour: w(18.0, 9780),
            seven_day: w(3.0, 148_000),
        };
        assert!(tooltip(Lang::Ru, &l, NOW).encode_utf16().count() <= 127);
        assert_eq!(
            tooltip(Lang::En, &AccountLimits::default(), NOW),
            "Resumer — watching limits"
        );
    }

    #[test]
    fn remaining_is_clamped() {
        assert_eq!(
            remaining(&Window {
                used_percentage: 120.0,
                resets_at: 1
            }),
            0.0
        );
        assert_eq!(
            remaining(&Window {
                used_percentage: -3.0,
                resets_at: 1
            }),
            100.0
        );
    }
}
