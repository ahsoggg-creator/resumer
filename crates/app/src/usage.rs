//! Hourly cross-check of the account limits against Anthropic's usage endpoint (the one Claude Code itself uses).
//! Reads the OAuth access token Claude Code stores on disk; the token is only sent to api.anthropic.com
//! and is never logged. An expired token is not refreshed here: Claude Code does that while it runs.
use resumer_core::limits::ApiSnapshot;
use resumer_core::now_epoch;
use resumer_core::state::Window;
use serde_json::Value;
use std::time::Duration;

const URL: &str = "https://api.anthropic.com/api/oauth/usage";

pub fn credentials_file() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join(".claude")
        .join(".credentials.json")
}

/// `None` when there is no (valid) login or the request failed; the caller keeps the session data then.
pub fn fetch() -> Result<ApiSnapshot, String> {
    let text = std::fs::read_to_string(credentials_file()).map_err(|_| "no credentials file".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "bad credentials file".to_string())?;
    let oauth = v.get("claudeAiOauth").ok_or("no OAuth login")?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or("no access token")?;
    if let Some(exp_ms) = oauth.get("expiresAt").and_then(Value::as_i64)
        && exp_ms / 1000 <= now_epoch()
    {
        return Err("access token expired (open Claude Code to refresh it)".into());
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .new_agent();
    let mut resp = agent
        .get(URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "resumer")
        .call()
        .map_err(|e| format!("usage request failed: {e}"))?;
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("usage response unreadable: {e}"))?;
    let snap = parse(&body, now_epoch())?;
    Ok(snap)
}

pub fn parse(body: &str, now: i64) -> Result<ApiSnapshot, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "usage response is not JSON".to_string())?;
    let window = |key: &str| -> Option<Window> {
        let w = v.get(key)?;
        Some(Window {
            used_percentage: w.get("utilization")?.as_f64()?,
            resets_at: parse_iso(w.get("resets_at")?.as_str()?)?,
        })
    };
    let (five_hour, seven_day) = (window("five_hour"), window("seven_day"));
    if five_hour.is_none() && seven_day.is_none() {
        return Err("usage response has no limits".into());
    }
    Ok(ApiSnapshot {
        fetched_at: now,
        five_hour,
        seven_day,
    })
}

/// `2026-10-14T18:40:00.143015+00:00` -> unix seconds.
pub fn parse_iso(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time_end = rest.find(['+', '-', 'Z']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(time_end);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t.next()?.split('.').next()?.parse().ok()?;
    let offset = match zone.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (oh, om) = zone[1..].split_once(':')?;
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
        _ => 0,
    };
    // days since 1970-01-01 (Howard Hinnant's civil-from-days, inverted)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times() {
        assert_eq!(parse_iso("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(parse_iso("2026-10-14T18:40:00.143015+00:00"), Some(1_792_003_200));
        assert_eq!(parse_iso("2026-10-14T20:40:00+02:00"), Some(1_792_003_200));
        assert_eq!(parse_iso("2026-10-14T18:40:00Z"), Some(1_792_003_200));
        assert_eq!(parse_iso("garbage"), None);
    }

    #[test]
    fn parses_the_real_shape() {
        let body = r#"{"five_hour":{"utilization":95.0,"resets_at":"2026-10-14T18:40:00.143015+00:00"},
            "seven_day":{"utilization":13.0,"resets_at":"2026-10-16T09:00:00.143033+00:00"},"seven_day_opus":null}"#;
        let s = parse(body, 5).unwrap();
        assert_eq!(s.fetched_at, 5);
        assert_eq!(s.five_hour.unwrap().used_percentage, 95.0);
        assert_eq!(s.seven_day.unwrap().resets_at, 1_792_141_200);
        assert!(parse("{}", 0).is_err());
    }
}
