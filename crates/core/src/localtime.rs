//! Local-time formatting for the UI (the logs stay in UTC).

#[cfg(windows)]
fn local_parts(epoch: i64) -> Option<(u16, u16, u16, u16, u16, u16)> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = (epoch.checked_add(11_644_473_600)?).checked_mul(10_000_000)? as u64;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut local: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0
        {
            return None;
        }
        Some((
            local.wYear,
            local.wMonth,
            local.wDay,
            local.wHour,
            local.wMinute,
            local.wSecond,
        ))
    }
}

#[cfg(not(windows))]
fn local_parts(epoch: i64) -> Option<(u16, u16, u16, u16, u16, u16)> {
    let s = crate::logger::format_time(epoch);
    let n: Vec<u16> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect();
    (n.len() >= 6).then(|| (n[0], n[1], n[2], n[3], n[4], n[5]))
}

/// `HH:MM`
pub fn hm(epoch: i64) -> String {
    local_parts(epoch)
        .map(|(_, _, _, h, m, _)| format!("{h:02}:{m:02}"))
        .unwrap_or_else(|| "--:--".into())
}

/// `YYYY-MM-DD HH:MM`
pub fn ymd_hm(epoch: i64) -> String {
    local_parts(epoch)
        .map(|(y, mo, d, h, m, _)| format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}"))
        .unwrap_or_else(|| "-".into())
}

/// `HH:MM` for today, `MM-DD HH:MM` otherwise.
pub fn short(epoch: i64, now: i64) -> String {
    match (local_parts(epoch), local_parts(now)) {
        (Some(a), Some(b)) if (a.0, a.1, a.2) == (b.0, b.1, b.2) => format!("{:02}:{:02}", a.3, a.4),
        (Some(a), _) => format!("{:02}-{:02} {:02}:{:02}", a.1, a.2, a.3, a.4),
        _ => "-".into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_without_panicking() {
        let now = crate::now_epoch();
        assert_eq!(super::hm(now).len(), 5);
        assert_eq!(super::ymd_hm(now).len(), 16);
        assert_eq!(super::short(now, now).len(), 5);
        assert_eq!(super::short(now + 3 * 86_400, now).len(), 11);
    }
}
