//! Resumer GUI: session list, account limits, manual wake-up, settings, hook installation and diagnostics.
#![windows_subsystem = "windows"]

mod diag;
mod theme;

use eframe::egui::{self, RichText};
use resumer_core::config::{Config, Lang, TargetMode};
use resumer_core::engine::{Injector, execute_force, prepare_force};
use resumer_core::i18n::{duration, fmt, status_label, t};
use resumer_core::limits::{self, AccountLimits};
use resumer_core::proc::ProcessInfo;
use resumer_core::scheduler::{self, SchedulerHandle};
use resumer_core::settings_merge as sm;
use resumer_core::state::{SessionState, StateStore, Status, Window};
use resumer_core::targets::{ForcePlan, RunSummary, state_for};
use resumer_core::{localtime, logger, now_epoch, paths, proc, schedule};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the list of live Claude Code processes is rescanned (in the background).
const SCAN_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Sessions,
    Settings,
}

/// One line of the session table: a live process, with its hook data if the hooks have seen it.
struct Row {
    pid: Option<u32>,
    cwd: String,
    status: Option<Status>,
    own: bool,
    waiting_until: Option<i64>,
}

#[derive(Default)]
struct RunUi {
    running: bool,
    /// (finished at, summary, was a dry run)
    last: Option<(i64, RunSummary, bool)>,
}

struct App {
    cfg: Config,
    store: StateStore,
    sessions: Vec<SessionState>,
    procs: Arc<Mutex<Vec<ProcessInfo>>>,
    rows: Vec<Row>,
    limits: AccountLimits,
    notice: String,
    last_refresh: i64,
    tab: Tab,
    whitelist_text: String,
    excluded_text: String,
    scheduler: SchedulerHandle,
    run: Arc<Mutex<RunUi>>,
    cancel: Arc<AtomicBool>,
    pending: Option<ForcePlan>,
    own: HashSet<u32>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let cfg = Config::load();
        let procs = Arc::new(Mutex::new(Vec::new()));
        {
            let procs = procs.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::Builder::new()
                .name("resumer-scan".into())
                .spawn(move || {
                    loop {
                        let found = proc::scan_claude();
                        if let Ok(mut p) = procs.lock() {
                            *p = found;
                        }
                        ctx.request_repaint();
                        std::thread::sleep(SCAN_EVERY);
                    }
                })
                .ok();
        }
        let ctx = cc.egui_ctx.clone();
        let scheduler = scheduler::spawn(Injector::InProcess, move |_| ctx.request_repaint());
        App {
            whitelist_text: cfg.whitelist.join("\n"),
            excluded_text: cfg.excluded.join("\n"),
            cfg,
            store: StateStore::default_store(),
            sessions: Vec::new(),
            procs,
            rows: Vec::new(),
            limits: AccountLimits::default(),
            notice: String::new(),
            last_refresh: 0,
            tab: Tab::Sessions,
            scheduler,
            run: Arc::new(Mutex::new(RunUi::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            pending: None,
            own: proc::own_ancestors(),
        }
    }

    fn lang(&self) -> Lang {
        self.cfg.language
    }

    fn refresh(&mut self) {
        let now = now_epoch();
        self.store.prune_dead(proc::is_alive);
        self.sessions = self.store.list();
        self.limits = limits::account_limits(&self.sessions, now);
        self.rows = self.build_rows(now);
        self.last_refresh = now;
    }

    /// Every live Claude Code process, plus sessions known only from hooks (the process scan missed them).
    fn build_rows(&self, now: i64) -> Vec<Row> {
        let procs = self.procs.lock().map(|p| p.clone()).unwrap_or_default();
        let mut used: HashSet<String> = HashSet::new();
        let mut rows = Vec::new();
        for p in &procs {
            let st = state_for(p, &self.sessions);
            if let Some(s) = st {
                used.insert(s.session_id.clone());
            }
            rows.push(Row {
                pid: Some(p.pid),
                cwd: st
                    .map(|s| s.cwd.clone())
                    .filter(|c| !c.is_empty())
                    .or_else(|| p.cwd.clone())
                    .unwrap_or_default(),
                status: st.map(|s| s.status),
                own: self.own.contains(&p.pid),
                waiting_until: st.and_then(|s| schedule::pending_reset(s, now)).filter(|&r| r > now),
            });
        }
        for s in self.sessions.iter().filter(|s| !used.contains(&s.session_id)) {
            rows.push(Row {
                pid: s.claude_pid,
                cwd: s.cwd.clone(),
                status: Some(s.status),
                own: false,
                waiting_until: schedule::pending_reset(s, now).filter(|&r| r > now),
            });
        }
        rows.sort_by_key(|r| r.cwd.to_lowercase());
        rows
    }

    fn save_cfg(&mut self) {
        self.cfg.whitelist = lines(&self.whitelist_text);
        self.cfg.excluded = lines(&self.excluded_text);
        if let Err(e) = self.cfg.save() {
            self.notice = format!("config: {e}");
        }
        self.scheduler.poke();
    }

    fn start_wake(&mut self) {
        let plan = prepare_force(&self.cfg, &self.store, self.cfg.include_waiting);
        if plan.send.is_empty() {
            self.notice = t(self.lang(), "run.nothing").to_string();
        } else {
            self.pending = Some(plan);
        }
    }

    fn confirm_wake(&mut self, ctx: &egui::Context) {
        let Some(plan) = self.pending.take() else { return };
        self.cancel.store(false, Ordering::SeqCst);
        if let Ok(mut r) = self.run.lock() {
            r.running = true;
        }
        let (cfg, run, cancel, ctx) = (self.cfg.clone(), self.run.clone(), self.cancel.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("resumer-wake".into())
            .spawn(move || {
                let store = StateStore::default_store();
                let sum = execute_force(
                    &plan,
                    &cfg,
                    &store,
                    Injector::InProcess,
                    cfg.include_waiting,
                    cfg.dry_run,
                    &cancel,
                );
                if let Ok(mut r) = run.lock() {
                    r.running = false;
                    r.last = Some((now_epoch(), sum, cfg.dry_run));
                }
                ctx.request_repaint();
            })
            .ok();
    }

    fn hooks_installed(&self) -> bool {
        std::fs::read_to_string(paths::claude_settings_file())
            .ok()
            .and_then(|t| sm::parse(&t).ok())
            .is_some_and(|v| sm::is_installed(&v))
    }

    fn install_hooks(&mut self) {
        // Hooks must call the console CLI: the GUI exe has no `hook` mode and would open a window per event.
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("resumer-cli.exe")))
            .filter(|p| p.is_file());
        let Some(exe) = exe else {
            self.notice = "resumer-cli.exe not found next to Resumer.exe".into();
            return;
        };
        let exe = exe.to_string_lossy().to_string();
        let path = paths::claude_settings_file();
        let old_text = std::fs::read_to_string(&path).unwrap_or_default();
        let result = sm::parse(&old_text)
            .map_err(|e| e.to_string())
            .and_then(|old| sm::install(&old, &exe).map_err(|e| e.to_string()));
        match result {
            Ok(r) => {
                let stamp = now_epoch();
                match sm::write_with_backup(&path, &sm::render(&r.settings), stamp) {
                    Ok(ap) => {
                        if r.chained.is_some() {
                            let mut cfg = self.cfg.clone();
                            cfg.chained_statusline = r.chained.clone();
                            let _ = cfg.save();
                            self.cfg = cfg;
                        }
                        logger::log("hooks installed from GUI");
                        self.notice = format!(
                            "{} {}",
                            t(self.lang(), "hooks.installed"),
                            ap.backup
                                .map(|b| format!("(backup: {})", b.display()))
                                .unwrap_or_default()
                        );
                    }
                    Err(e) => self.notice = format!("write failed: {e}"),
                }
            }
            Err(e) => self.notice = e,
        }
    }

    fn remove_hooks(&mut self) {
        let path = paths::claude_settings_file();
        let old_text = std::fs::read_to_string(&path).unwrap_or_default();
        let result = sm::parse(&old_text)
            .map_err(|e| e.to_string())
            .and_then(|old| sm::uninstall(&old, self.cfg.chained_statusline.as_deref()).map_err(|e| e.to_string()));
        match result {
            Ok(new) => match sm::write_with_backup(&path, &sm::render(&new), now_epoch()) {
                Ok(_) => {
                    logger::log("hooks removed from GUI");
                    self.notice = t(self.lang(), "hooks.missing").to_string();
                }
                Err(e) => self.notice = format!("write failed: {e}"),
            },
            Err(e) => self.notice = e,
        }
    }

    fn next_wake_text(&self, now: i64) -> String {
        let lang = self.lang();
        if self.cfg.paused || !self.cfg.auto_send {
            return t(lang, "set.paused").to_string();
        }
        match schedule::plan(&self.sessions, &self.cfg, now).next_wake {
            Some(t_wake) => format!(
                "{}: {} ({} {})",
                t(lang, "next.at"),
                localtime::short(t_wake, now),
                t(lang, "next.in"),
                duration(lang, t_wake - now)
            ),
            None => t(lang, "next.none").to_string(),
        }
    }

    fn limit_bar(&self, ui: &mut egui::Ui, label: &str, w: Option<Window>, now: i64) {
        let lang = self.lang();
        ui.horizontal(|ui| {
            ui.add_sized([60.0, 18.0], egui::Label::new(RichText::new(label).strong()));
            match w {
                Some(w) => {
                    let left = limits::remaining(&w);
                    let color = if left <= 10.0 {
                        theme::RED
                    } else if left <= 30.0 {
                        theme::YELLOW
                    } else {
                        theme::GREEN
                    };
                    ui.add(
                        egui::ProgressBar::new((left / 100.0) as f32)
                            .desired_width(220.0)
                            .fill(color)
                            .text(RichText::new(format!("{} {:.0}%", t(lang, "limits.left"), left)).color(theme::BASE)),
                    );
                    ui.label(format!(
                        "{} {}",
                        t(lang, "limits.resets_in"),
                        duration(lang, w.resets_at - now)
                    ));
                }
                None => {
                    ui.label(RichText::new(t(lang, "limits.no_data")).color(theme::SUBTEXT));
                }
            }
        });
    }

    fn sessions_tab(&mut self, ui: &mut egui::Ui, now: i64) {
        let lang = self.lang();
        let (running, last) = self.run.lock().map(|r| (r.running, r.last.clone())).unwrap_or_default();

        ui.add_space(4.0);
        let (five, week) = (self.limits.five_hour, self.limits.seven_day);
        self.limit_bar(ui, t(lang, "col.five"), five, now);
        self.limit_bar(ui, t(lang, "col.week"), week, now);
        if let Some(upd) = self.sessions.iter().map(|s| s.updated_at).max() {
            ui.label(
                RichText::new(format!(
                    "{} {} {}",
                    t(lang, "limits.updated"),
                    duration(lang, now - upd),
                    t(lang, "limits.ago")
                ))
                .small()
                .color(theme::SUBTEXT),
            );
        }
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if running {
                ui.spinner();
                ui.label(t(lang, "run.running"));
                if ui.button(t(lang, "btn.stop_run")).clicked() {
                    self.cancel.store(true, Ordering::SeqCst);
                }
            } else if let Some(n) = self.pending.as_ref().map(|p| p.send.len()) {
                ui.label(fmt(lang, "confirm.wake", &[("n", n.to_string())]));
                if ui.button(t(lang, "btn.confirm")).clicked() {
                    let ctx = ui.ctx().clone();
                    self.confirm_wake(&ctx);
                }
                if ui.button(t(lang, "btn.cancel")).clicked() {
                    self.pending = None;
                }
            } else if ui
                .add(egui::Button::new(RichText::new(t(lang, "btn.wake_now")).strong()).fill(theme::SURFACE1))
                .clicked()
            {
                self.start_wake();
            }
            let label = if self.cfg.paused { "btn.resume" } else { "btn.pause" };
            if ui.button(t(lang, label)).clicked() {
                self.cfg.paused = !self.cfg.paused;
                self.save_cfg();
            }
        });
        if let Some((at, sum, dry)) = last {
            let n = if dry { sum.would_send.len() } else { sum.sent.len() };
            let failed = if sum.failed.is_empty() {
                String::new()
            } else {
                format!(", ✗ {}", sum.failed.len())
            };
            let dry_note = if dry {
                format!(" ({})", t(lang, "run.dry"))
            } else {
                String::new()
            };
            ui.label(
                RichText::new(format!(
                    "{} {}: {n}{failed}{dry_note}",
                    t(lang, "run.last"),
                    localtime::hm(at)
                ))
                .color(theme::SUBTEXT),
            );
        }
        ui.add_space(6.0);

        if self.rows.is_empty() {
            ui.label(RichText::new(t(lang, "sessions.none")).color(theme::SUBTEXT));
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sessions")
                .striped(true)
                .num_columns(4)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for h in ["col.status", "col.project", "col.pid", "col.blocked"] {
                        ui.strong(t(lang, h));
                    }
                    ui.end_row();
                    for r in &self.rows {
                        let (text, color) = match r.status {
                            Some(Status::RateLimited) => (status_label(lang, Status::RateLimited), theme::RED),
                            Some(Status::NeedsInput) => (status_label(lang, Status::NeedsInput), theme::YELLOW),
                            Some(Status::Running) => (status_label(lang, Status::Running), theme::BLUE),
                            Some(Status::Idle) => (status_label(lang, Status::Idle), theme::GREEN),
                            None => (t(lang, "status.unknown"), theme::SUBTEXT),
                        };
                        ui.colored_label(color, text);
                        if r.own {
                            ui.label(format!("{}  ({})", r.cwd, t(lang, "status.you")));
                        } else {
                            ui.label(&r.cwd);
                        }
                        ui.label(r.pid.map(|p| p.to_string()).unwrap_or_default());
                        ui.label(
                            r.waiting_until
                                .map(|u| localtime::short(u, now))
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            ui.label(RichText::new(t(lang, "hint.unknown")).small().color(theme::SUBTEXT));
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let mut changed = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            changed |= ui.checkbox(&mut self.cfg.auto_send, t(lang, "set.auto_send")).changed();
            ui.label(
                RichText::new(t(lang, "set.auto_send.hint"))
                    .small()
                    .color(theme::SUBTEXT),
            );
            ui.label(RichText::new(t(lang, "set.refresh")).small().color(theme::SUBTEXT));
            ui.add_space(6.0);
            changed |= ui.checkbox(&mut self.cfg.paused, t(lang, "set.paused")).changed();
            changed |= ui.checkbox(&mut self.cfg.dry_run, t(lang, "set.dry_run")).changed();
            changed |= ui
                .checkbox(&mut self.cfg.include_waiting, t(lang, "set.include_waiting"))
                .changed();
            ui.add_space(6.0);
            ui.label(RichText::new(t(lang, "set.mode")).strong());
            changed |= ui
                .radio_value(
                    &mut self.cfg.target_mode,
                    TargetMode::Blocked,
                    t(lang, "set.mode.blocked"),
                )
                .changed();
            changed |= ui
                .radio_value(&mut self.cfg.target_mode, TargetMode::AllKnown, t(lang, "set.mode.all"))
                .changed();
            ui.add_space(6.0);
            ui.label(RichText::new(t(lang, "set.message")).strong());
            changed |= ui
                .add(
                    egui::TextEdit::multiline(&mut self.cfg.message)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                )
                .changed();
            ui.add_space(6.0);
            egui::Grid::new("numbers")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    for (key, v, max) in [
                        ("set.margin", &mut self.cfg.margin_secs, 3600),
                        ("set.gap_min", &mut self.cfg.gap_min_secs, 600),
                        ("set.gap_max", &mut self.cfg.gap_max_secs, 600),
                        ("set.enter_delay", &mut self.cfg.enter_delay_ms, 2000),
                    ] {
                        ui.label(t(lang, key));
                        changed |= ui.add(egui::DragValue::new(v).range(0..=max)).changed();
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "{} ({})",
                    t(lang, "set.whitelist"),
                    t(lang, "set.one_per_line")
                ))
                .strong(),
            );
            ui.label(
                RichText::new(t(lang, "set.whitelist.hint"))
                    .small()
                    .color(theme::SUBTEXT),
            );
            changed |= ui
                .add(
                    egui::TextEdit::multiline(&mut self.whitelist_text)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                )
                .changed();
            ui.label(RichText::new(format!("{} ({})", t(lang, "set.excluded"), t(lang, "set.one_per_line"))).strong());
            changed |= ui
                .add(
                    egui::TextEdit::multiline(&mut self.excluded_text)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                )
                .changed();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(t(lang, "set.language"));
                changed |= ui.radio_value(&mut self.cfg.language, Lang::Ru, "Русский").changed();
                changed |= ui.radio_value(&mut self.cfg.language, Lang::En, "English").changed();
            });
        });
        if changed {
            self.cfg.gap_max_secs = self.cfg.gap_max_secs.max(self.cfg.gap_min_secs);
            self.save_cfg();
        }
    }
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = now_epoch();
        if now - self.last_refresh >= 2 {
            self.refresh();
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        let lang = self.lang();
        let installed = self.hooks_installed();

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new(t(lang, "app.title")).color(theme::MAUVE));
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Sessions, t(lang, "tab.sessions"));
                ui.selectable_value(&mut self.tab, Tab::Settings, t(lang, "tab.settings"));
                ui.separator();
                ui.label(self.next_wake_text(now));
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("bottom").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if installed {
                    ui.colored_label(theme::GREEN, t(lang, "hooks.installed"));
                    if ui.button(t(lang, "btn.remove_hooks")).clicked() {
                        self.remove_hooks();
                    }
                } else {
                    ui.colored_label(theme::YELLOW, t(lang, "hooks.missing"));
                    if ui.button(t(lang, "btn.install_hooks")).clicked() {
                        self.install_hooks();
                    }
                }
                if ui.button(t(lang, "btn.open_logs")).clicked() {
                    let _ = std::process::Command::new("explorer").arg(paths::logs_dir()).spawn();
                }
                if ui.button(t(lang, "btn.copy_diag")).clicked() {
                    ui.ctx().copy_text(diag::diagnostics_text());
                    self.notice = t(lang, "btn.copy_diag").to_string();
                }
            });
            if !self.notice.is_empty() {
                ui.label(RichText::new(&self.notice).color(theme::SUBTEXT));
            }
            ui.label(RichText::new(t(lang, "about.privacy")).small().color(theme::SUBTEXT));
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Sessions => self.sessions_tab(ui, now),
            Tab::Settings => self.settings_tab(ui),
        });
    }
}

fn main() -> eframe::Result {
    diag::install_panic_hook();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 600.0])
            .with_title("Resumer"),
        ..Default::default()
    };
    eframe::run_native("Resumer", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
