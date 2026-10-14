//! Resumer GUI: session list, hook installation and diagnostics.
#![windows_subsystem = "windows"]

mod diag;
mod theme;

use eframe::egui::{self, RichText};
use resumer_core::config::Config;
use resumer_core::config::Lang;
use resumer_core::i18n::{fmt, status_label, t};
use resumer_core::settings_merge as sm;
use resumer_core::state::{SessionState, StateStore, Status};
use resumer_core::{localtime, logger, now_epoch, paths, proc, schedule};

struct App {
    cfg: Config,
    store: StateStore,
    sessions: Vec<SessionState>,
    notice: String,
    last_refresh: i64,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        App {
            cfg: Config::load(),
            store: StateStore::default_store(),
            sessions: Vec::new(),
            notice: String::new(),
            last_refresh: 0,
        }
    }

    fn lang(&self) -> Lang {
        self.cfg.language
    }

    fn refresh(&mut self) {
        let now = now_epoch();
        self.store.prune_dead(proc::is_alive);
        self.sessions = self.store.list();
        self.last_refresh = now;
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
        match schedule::plan(&self.sessions, &self.cfg, now).next_wake {
            Some(t_wake) => format!(
                "{}: {} ({} {})",
                t(lang, "next.at"),
                localtime::short(t_wake, now),
                t(lang, "next.in"),
                resumer_core::i18n::duration(lang, t_wake - now)
            ),
            None => t(lang, "next.none").to_string(),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = now_epoch();
        if now - self.last_refresh >= 2 {
            self.refresh();
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(2));
        let lang = self.lang();
        let installed = self.hooks_installed();

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new(t(lang, "app.title")).color(theme::MAUVE));
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
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if self.sessions.is_empty() {
                ui.label(RichText::new(t(lang, "sessions.none")).color(theme::SUBTEXT));
                return;
            }
            egui::Grid::new("sessions").striped(true).num_columns(5).show(ui, |ui| {
                for h in ["col.status", "col.project", "col.pid", "col.five", "col.week"] {
                    ui.strong(t(lang, h));
                }
                ui.end_row();
                for s in &self.sessions {
                    let color = match s.status {
                        Status::RateLimited => theme::RED,
                        Status::NeedsInput => theme::YELLOW,
                        Status::Running => theme::BLUE,
                        Status::Idle => theme::GREEN,
                    };
                    ui.colored_label(color, status_label(lang, s.status));
                    ui.label(&s.cwd);
                    ui.label(s.claude_pid.map(|p| p.to_string()).unwrap_or_default());
                    ui.label(
                        s.five_hour
                            .map(|w| format!("{:.0}%", w.used_percentage))
                            .unwrap_or_else(|| "-".into()),
                    );
                    ui.label(
                        s.seven_day
                            .map(|w| format!("{:.0}%", w.used_percentage))
                            .unwrap_or_else(|| "-".into()),
                    );
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new(fmt(lang, "about.privacy", &[]))
                    .small()
                    .color(theme::SUBTEXT),
            );
        });
    }
}

fn main() -> eframe::Result {
    diag::install_panic_hook();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 460.0])
            .with_title("Resumer"),
        ..Default::default()
    };
    eframe::run_native("Resumer", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
