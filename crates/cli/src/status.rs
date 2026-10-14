use crate::{NudgeArgs, REAL_NUDGE_ENABLED};
use resumer_core::config::{Config, Lang};
use resumer_core::engine::{Injector, execute_force, prepare_force};
use resumer_core::i18n::duration;
use resumer_core::state::StateStore;
use resumer_core::{localtime, now_epoch, proc, schedule};
use std::sync::atomic::AtomicBool;

fn win(w: &Option<resumer_core::state::Window>) -> String {
    w.map(|w| format!("{:.0}%", w.used_percentage))
        .unwrap_or_else(|| "-".into())
}

pub fn run() -> i32 {
    let store = StateStore::default_store();
    let pruned = store.prune_dead(proc::is_alive);
    store.sweep_stale_files(std::time::Duration::from_secs(60));
    let states = store.list();
    let cfg = Config::load();
    let now = now_epoch();
    if pruned > 0 {
        println!("(removed {pruned} dead session(s))");
    }
    if states.is_empty() {
        println!("No known sessions. Run `resumer-cli install-hooks`, then start Claude Code.");
    } else {
        println!(
            "{:<10} {:<13} {:<7} {:<5} {:<5} {:<12} project",
            "session", "status", "pid", "5h", "7d", "blocked till"
        );
        for s in &states {
            println!(
                "{:<10} {:<13} {:<7} {:<5} {:<5} {:<12} {}",
                s.session_id.chars().take(8).collect::<String>(),
                s.status.as_str(),
                s.claude_pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                win(&s.five_hour),
                win(&s.seven_day),
                if s.blocked_until > 0 {
                    localtime::short(s.blocked_until, now)
                } else {
                    "-".into()
                },
                s.cwd
            );
        }
    }
    let sched = schedule::plan(&states, &cfg, now);
    match sched.next_wake {
        Some(t) => println!(
            "\nNext wake-up: {} (in {})",
            localtime::short(t, now),
            duration(Lang::En, t - now)
        ),
        None => println!(
            "\nNext wake-up: none scheduled{}",
            if cfg.paused { " (paused)" } else { "" }
        ),
    }
    if cfg.whitelist.is_empty() {
        println!(
            "Note: the automatic wake-up whitelist is empty, so nothing is woken automatically (manual `nudge --all` still works)."
        );
    }
    0
}

pub fn nudge(a: NudgeArgs) -> i32 {
    if !a.all {
        eprintln!("Only `nudge --all` is supported.");
        return 2;
    }
    if !a.dry_run && !REAL_NUDGE_ENABLED {
        eprintln!("Real typing from the CLI is disabled in this version (0.1.0). Use --dry-run.");
        return 2;
    }
    let mut cfg = Config::load();
    if let Some(t) = a.text {
        cfg.message = t;
    }
    let store = StateStore::default_store();
    let include_waiting = a.include_waiting || cfg.include_waiting;
    let plan = prepare_force(&cfg, &store, include_waiting);
    let now = now_epoch();
    if let Some(until) = plan.limit_until
        && !a.force
        && !a.dry_run
    {
        eprintln!(
            "The limit is not reset until {}. Use --force to send anyway.",
            localtime::short(until, now)
        );
        return 3;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let sum = execute_force(
        &plan,
        &cfg,
        &store,
        Injector::Detached(exe),
        include_waiting,
        a.dry_run,
        &AtomicBool::new(false),
    );
    println!("Windows found: {}", plan.found);
    for l in &sum.would_send {
        println!("  would send: {l}");
    }
    for l in &sum.sent {
        println!("  sent: {l}");
    }
    for (t, why) in &plan.skipped {
        let r = match why {
            resumer_core::targets::ForceSkip::NeedsInput => "waiting for an answer - left alone",
            resumer_core::targets::ForceSkip::OwnSession => "this session",
            resumer_core::targets::ForceSkip::Excluded => "excluded folder",
        };
        println!("  skipped: {} - {r}", resumer_core::engine::label(&t.cwd, t.pid));
    }
    for (l, why) in &sum.skipped {
        println!("  skipped: {l} - {why}");
    }
    for (l, e) in &sum.failed {
        println!("  failed: {l} - {e}");
    }
    if sum.failed.is_empty() { 0 } else { 1 }
}
