use crate::SendArgs;
use resumer_core::console::SendOptions;
use resumer_core::engine::{Injector, inject, inject_detached};
use resumer_core::logger::log;
use resumer_core::proc;
use resumer_core::state::StateStore;
use resumer_core::targets::state_for;
use std::io::Write;

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

pub fn list() -> i32 {
    let procs = proc::scan_claude();
    let states = StateStore::default_store().list();
    let own = proc::own_ancestors();
    if procs.is_empty() {
        println!("No Claude Code processes found.");
        return 0;
    }
    println!(
        "{:<7} {:<7} {:<11} {:<4} {:<12} {:<34} cmdline",
        "PID", "PPID", "image", "WT", "status", "cwd"
    );
    for p in &procs {
        let st = state_for(p, &states).map(|s| s.status.as_str()).unwrap_or("-");
        let mark = if own.contains(&p.pid) { " (this session)" } else { "" };
        println!(
            "{:<7} {:<7} {:<11} {:<4} {:<12} {:<34} {}{}",
            p.pid,
            p.ppid,
            p.name,
            if p.in_windows_terminal { "yes" } else { "no" },
            st,
            trunc(p.cwd.as_deref().unwrap_or("?"), 34),
            trunc(p.cmdline.as_deref().unwrap_or("?"), 90),
            mark
        );
    }
    0
}

pub fn send(a: SendArgs) -> i32 {
    let procs = proc::scan_claude();
    let own = proc::own_ancestors();
    if own.contains(&a.pid) {
        eprintln!("Refusing: PID {} is this very session (or its ancestor).", a.pid);
        return 2;
    }
    let target = procs.iter().find(|p| p.pid == a.pid);
    if target.is_none() && !a.any {
        eprintln!(
            "PID {} is not a recognised Claude Code process (see `probe list`). Use --any to override.",
            a.pid
        );
        return 2;
    }
    if !a.yes {
        let what = target
            .map(|t| format!("{} in {}", t.name, t.cwd.as_deref().unwrap_or("?")))
            .unwrap_or_default();
        print!(
            "Type {:?}{} into PID {} ({what})? [y/N] ",
            a.text,
            if a.no_enter { "" } else { " + Enter" },
            a.pid
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return 1;
        }
    }
    let opts = SendOptions {
        enter_delay_ms: a.enter_delay_ms,
        raw_vk: a.raw_vk,
        press_enter: !a.no_enter,
    };
    let exe = std::env::current_exe().unwrap_or_default();
    let t0 = std::time::Instant::now();
    match inject_detached(&exe, a.pid, &a.text, &opts) {
        Ok(()) => {
            log(&format!(
                "probe send: pid={} ok (enter_delay={}ms raw_vk={})",
                a.pid, a.enter_delay_ms, a.raw_vk
            ));
            println!(
                "Sent to PID {} in {} ms (enter delay {} ms{}).",
                a.pid,
                t0.elapsed().as_millis(),
                a.enter_delay_ms,
                if a.raw_vk { ", raw vk" } else { "" }
            );
            0
        }
        Err(e) => {
            log(&format!("probe send: pid={} failed: {e}", a.pid));
            eprintln!("Failed: {e}");
            1
        }
    }
}

/// Hidden `__inject <pid> <enter_delay_ms> <raw 0|1> <enter 0|1> <text>`: runs without a console of its own.
pub fn inject_entry(args: &[String]) -> i32 {
    let parse = || -> Option<(u32, SendOptions, String)> {
        let pid = args.first()?.parse().ok()?;
        let delay = args.get(1)?.parse().ok()?;
        Some((
            pid,
            SendOptions {
                enter_delay_ms: delay,
                raw_vk: args.get(2)? == "1",
                press_enter: args.get(3)? == "1",
            },
            args.get(4).cloned().unwrap_or_default(),
        ))
    };
    let Some((pid, opts, text)) = parse() else {
        eprintln!("bad __inject arguments");
        return 64;
    };
    match inject(&Injector::InProcess, pid, &text, &opts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
