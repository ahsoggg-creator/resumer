//! Crash reports and the "copy diagnostics" text. Neither ever contains session content.
use resumer_core::state::StateStore;
use resumer_core::{VERSION, logger, now_epoch, paths};
use std::fmt::Write as _;

pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "?".into());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let report = format!(
            "Resumer {VERSION} crash\ntime: {}\nlocation: {loc}\nmessage: {msg}\n\nstack:\n{}\n\nlog tail:\n{}\n",
            logger::format_time(now_epoch()),
            std::backtrace::Backtrace::force_capture(),
            logger::tail(40)
        );
        let dir = paths::crash_dir();
        let _ = std::fs::create_dir_all(&dir);
        let name = format!(
            "crash-{}.txt",
            logger::format_time(now_epoch()).replace([':', ' '], "-")
        );
        let _ = std::fs::write(dir.join(name), report);
        logger::log(&format!("PANIC at {loc}"));
        default(info);
    }));
}

pub fn diagnostics_text() -> String {
    let store = StateStore::default_store();
    let mut s = String::new();
    let _ = writeln!(s, "Resumer {VERSION}");
    let _ = writeln!(s, "data: {}", paths::data_dir().display());
    let settings_text = std::fs::read_to_string(paths::claude_settings_file()).unwrap_or_default();
    let hooks = resumer_core::settings_merge::parse(&settings_text)
        .map(|v| resumer_core::settings_merge::is_installed(&v))
        .unwrap_or(false);
    let _ = writeln!(s, "hooks installed: {hooks}");
    let sessions = store.list();
    let _ = writeln!(s, "sessions: {}", sessions.len());
    for st in &sessions {
        let _ = writeln!(
            s,
            "  {} status={} pid={:?} blocked_until={} last_nudged={}",
            st.session_id.chars().take(8).collect::<String>(),
            st.status.as_str(),
            st.claude_pid,
            st.blocked_until,
            st.last_nudged_epoch
        );
    }
    let _ = writeln!(s, "\nlog tail:\n{}", logger::tail(30));
    s
}
