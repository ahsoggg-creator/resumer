//! `resumer-cli`: hooks, probes, hook installation, status and the manual wake-up.
mod hook;
mod probe;
mod settings_cmd;
mod status;

use clap::{Args, Parser, Subcommand};

/// 0.1.0 only checks what a run would do; real typing from the CLI arrives together with the daemon (0.2.0).
const REAL_NUDGE_ENABLED: bool = false;

#[derive(Parser)]
#[command(
    name = "resumer-cli",
    version,
    about = "Resumer: wake Claude Code sessions after a usage limit reset"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Probe tools: find sessions and test typing into a console.
    #[command(subcommand)]
    Probe(ProbeCmd),
    /// Claude Code hook / statusLine entry points (always exit 0).
    #[command(subcommand)]
    Hook(HookCmd),
    /// Add Resumer's hooks and statusLine to ~/.claude/settings.json (backup, diff, confirmation).
    InstallHooks(SettingsArgs),
    /// Remove them again and restore the previous statusLine.
    UninstallHooks(SettingsArgs),
    /// Known sessions and the next wake-up.
    Status,
    /// Wake every Claude Code window now.
    Nudge(NudgeArgs),
}

#[derive(Subcommand)]
enum ProbeCmd {
    /// List Claude Code processes found on this computer.
    List,
    /// Type text and Enter into the console of a process.
    Send(SendArgs),
}

#[derive(Args)]
struct SendArgs {
    pid: u32,
    text: String,
    /// Pause between the text and Enter.
    #[arg(long, default_value_t = 80)]
    enter_delay_ms: u64,
    /// Type characters with virtual-key 0 (no keyboard layout lookup).
    #[arg(long)]
    raw_vk: bool,
    /// Do not press Enter.
    #[arg(long)]
    no_enter: bool,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    yes: bool,
    /// Allow a process that is not recognised as Claude Code.
    #[arg(long)]
    any: bool,
}

#[derive(Subcommand)]
enum HookCmd {
    Statusline,
    StopFailure,
    Notification,
    Activity,
}

#[derive(Args)]
struct SettingsArgs {
    /// Path to resumer-cli.exe to write into the hooks (default: this executable).
    #[arg(long)]
    exe: Option<std::path::PathBuf>,
    /// settings.json to edit (default: ~/.claude/settings.json).
    #[arg(long)]
    settings: Option<std::path::PathBuf>,
    /// Show the diff and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    yes: bool,
}

#[derive(Args)]
struct NudgeArgs {
    /// Required: every found Claude Code window.
    #[arg(long)]
    all: bool,
    #[arg(long)]
    text: Option<String>,
    /// Only report what would be sent.
    #[arg(long)]
    dry_run: bool,
    /// Send even if a limit is still active.
    #[arg(long)]
    force: bool,
    /// Also type into sessions that wait for an answer.
    #[arg(long)]
    include_waiting: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("__inject") {
        std::process::exit(probe::inject_entry(&args[2..]));
    }
    if args.get(1).map(String::as_str) == Some("hook") {
        // Hooks must never fail, whatever clap or anything else thinks.
        let kind = match args.get(2).map(String::as_str) {
            Some("statusline") => resumer_core::hooks::HookKind::Statusline,
            Some("stop-failure") => resumer_core::hooks::HookKind::StopFailure,
            Some("notification") => resumer_core::hooks::HookKind::Notification,
            _ => resumer_core::hooks::HookKind::Activity,
        };
        hook::run(kind);
        std::process::exit(0);
    }
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Probe(ProbeCmd::List) => probe::list(),
        Cmd::Probe(ProbeCmd::Send(a)) => probe::send(a),
        Cmd::Hook(h) => {
            hook::run(match h {
                HookCmd::Statusline => resumer_core::hooks::HookKind::Statusline,
                HookCmd::StopFailure => resumer_core::hooks::HookKind::StopFailure,
                HookCmd::Notification => resumer_core::hooks::HookKind::Notification,
                HookCmd::Activity => resumer_core::hooks::HookKind::Activity,
            });
            0
        }
        Cmd::InstallHooks(a) => settings_cmd::install(a),
        Cmd::UninstallHooks(a) => settings_cmd::uninstall(a),
        Cmd::Status => status::run(),
        Cmd::Nudge(a) => status::nudge(a),
    };
    std::process::exit(code);
}
