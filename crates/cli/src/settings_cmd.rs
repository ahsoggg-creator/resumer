use crate::SettingsArgs;
use resumer_core::config::Config;
use resumer_core::settings_merge as sm;
use resumer_core::{now_epoch, paths};
use std::io::Write;

fn confirm(prompt: &str, yes: bool) -> bool {
    if yes {
        return true;
    }
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line.trim().eq_ignore_ascii_case("y")
}

fn load(path: &std::path::Path) -> Result<(String, serde_json::Value), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let v = sm::parse(&text).map_err(|e| e.to_string())?;
    Ok((text, v))
}

pub fn install(a: SettingsArgs) -> i32 {
    let path = a.settings.clone().unwrap_or_else(paths::claude_settings_file);
    let exe = a
        .exe
        .clone()
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_default();
    let exe_s = exe.to_string_lossy().to_string();
    let (old_text, old) = match load(&path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let res = match sm::install(&old, &exe_s) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let new_text = sm::render(&res.settings);
    let d = sm::diff(&sm::render(&old), &new_text);
    if d.is_empty() {
        println!("Already installed, nothing to change ({}).", path.display());
        return 0;
    }
    println!("Changes to {}:\n{d}", path.display());
    if let Some(c) = &res.chained {
        println!("Existing statusLine will be kept and called through Resumer:\n  {c}\n");
    }
    if a.dry_run {
        println!("(dry run: nothing written)");
        return 0;
    }
    if !confirm("Apply?", a.yes) {
        println!("Cancelled, nothing changed.");
        return 1;
    }
    let _ = old_text;
    match sm::write_with_backup(&path, &new_text, now_epoch()) {
        Ok(ap) => {
            if res.chained.is_some() {
                let mut cfg = Config::load();
                cfg.chained_statusline = res.chained.clone();
                if let Err(e) = cfg.save() {
                    eprintln!("warning: could not save the chained statusLine to config: {e}");
                }
            }
            println!(
                "Done. Backup: {}",
                ap.backup
                    .map(|b| b.display().to_string())
                    .unwrap_or_else(|| "(file did not exist)".into())
            );
            println!("Restart running Claude Code sessions (or open /hooks) so they pick up the new settings.");
            0
        }
        Err(e) => {
            eprintln!("Write failed: {e}");
            1
        }
    }
}

pub fn uninstall(a: SettingsArgs) -> i32 {
    let path = a.settings.clone().unwrap_or_else(paths::claude_settings_file);
    let (_, old) = match load(&path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let cfg = Config::load();
    let new = match sm::uninstall(&old, cfg.chained_statusline.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let d = sm::diff(&sm::render(&old), &sm::render(&new));
    if d.is_empty() {
        println!("Resumer entries not found in {}; nothing to remove.", path.display());
        return 0;
    }
    println!("Changes to {}:\n{d}", path.display());
    if a.dry_run {
        println!("(dry run: nothing written)");
        return 0;
    }
    if !confirm("Apply?", a.yes) {
        println!("Cancelled, nothing changed.");
        return 1;
    }
    match sm::write_with_backup(&path, &sm::render(&new), now_epoch()) {
        Ok(ap) => {
            let mut cfg = cfg;
            cfg.chained_statusline = None;
            let _ = cfg.save();
            println!(
                "Done. Backup: {}",
                ap.backup.map(|b| b.display().to_string()).unwrap_or_default()
            );
            0
        }
        Err(e) => {
            eprintln!("Write failed: {e}");
            1
        }
    }
}
