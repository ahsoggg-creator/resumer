//! Localisation table. Every user-visible string of the GUI/tray goes through `t()`.
use crate::config::Lang;

/// `(key, ru, en)`
pub const TABLE: &[(&str, &str, &str)] = &[
    ("app.title", "Resumer", "Resumer"),
    ("tab.sessions", "Сессии", "Sessions"),
    ("tab.settings", "Настройки", "Settings"),
    ("tab.about", "О программе", "About"),
    ("col.status", "Статус", "Status"),
    ("col.project", "Проект", "Project"),
    ("col.pid", "PID", "PID"),
    ("col.five", "5 ч", "5 h"),
    ("col.week", "Неделя", "Week"),
    ("col.auto", "Авто", "Auto"),
    ("status.running", "работает", "running"),
    ("status.idle", "ждёт ввода", "idle"),
    ("status.rate_limited", "упёрлась в лимит", "rate limited"),
    ("status.needs_input", "ждёт ответа", "needs input"),
    (
        "sessions.none",
        "Известных сессий нет. Установите хуки и запустите Claude Code.",
        "No known sessions. Install the hooks and start Claude Code.",
    ),
    ("next.none", "Пробуждение не запланировано", "No wake-up scheduled"),
    ("next.at", "Ближайшее пробуждение", "Next wake-up"),
    ("next.in", "через", "in"),
    ("btn.wake_now", "Разбудить все сейчас", "Wake all now"),
    ("btn.pause", "Пауза", "Pause"),
    ("btn.resume", "Возобновить", "Resume"),
    ("btn.open_logs", "Открыть логи", "Open logs"),
    ("btn.install_hooks", "Установить хуки", "Install hooks"),
    ("btn.remove_hooks", "Удалить хуки", "Remove hooks"),
    ("btn.update", "Обновление", "Update"),
    ("btn.check_update", "Проверить обновления", "Check for updates"),
    ("btn.copy_diag", "Скопировать диагностику", "Copy diagnostics"),
    ("btn.open_folder", "Открыть папку данных", "Open data folder"),
    ("btn.save", "Сохранить", "Save"),
    ("btn.add_folder", "Добавить", "Add"),
    ("btn.remove", "Убрать", "Remove"),
    ("btn.confirm", "Подтвердить", "Confirm"),
    ("btn.cancel", "Отмена", "Cancel"),
    ("btn.send_anyway", "Всё равно отправить", "Send anyway"),
    ("hooks.installed", "Хуки установлены", "Hooks installed"),
    (
        "hooks.missing",
        "Хуки не установлены — Resumer не видит сессии",
        "Hooks are not installed — Resumer cannot see sessions",
    ),
    ("set.language", "Язык", "Language"),
    ("set.message", "Сообщение", "Message"),
    ("set.margin", "Запас после сброса, с", "Margin after reset, s"),
    ("set.gap_min", "Пауза между окнами от, с", "Gap between windows from, s"),
    ("set.gap_max", "до, с", "to, s"),
    ("set.enter_delay", "Задержка перед Enter, мс", "Delay before Enter, ms"),
    ("set.mode", "Кого будить", "Whom to wake"),
    (
        "set.mode.blocked",
        "Только упёршихся в лимит",
        "Only sessions stopped by a limit",
    ),
    ("set.mode.all", "Все известные сессии", "All known sessions"),
    (
        "set.include_waiting",
        "Ручной прогон: включая ждущих ответа",
        "Manual run: include sessions waiting for an answer",
    ),
    (
        "set.dry_run",
        "Пробный режим (ничего не вводить, только журнал)",
        "Dry run (type nothing, only log)",
    ),
    ("set.autostart", "Запускать вместе с Windows", "Start with Windows"),
    (
        "set.auto_update",
        "Проверять обновления раз в сутки",
        "Check for updates daily",
    ),
    ("set.notifications", "Уведомления Windows", "Windows notifications"),
    (
        "set.tray_limits",
        "Показывать лимиты в подсказке трея",
        "Show limits in the tray tooltip",
    ),
    (
        "set.hotkey",
        "Горячая клавиша «разбудить все»",
        "Hotkey for \"wake all\"",
    ),
    (
        "set.hotkey.hint",
        "Например Ctrl+Alt+R; пусто — не назначена",
        "E.g. Ctrl+Alt+R; empty = not assigned",
    ),
    (
        "set.whitelist",
        "Проекты для автопробуждения",
        "Projects for automatic wake-up",
    ),
    (
        "set.whitelist.hint",
        "Пусто — автопробуждение никого не трогает",
        "Empty — automatic wake-up touches nobody",
    ),
    ("set.excluded", "Никогда не трогать", "Never touch"),
    ("set.diag", "Диагностика", "Diagnostics"),
    ("tray.settings", "Настройки…", "Settings…"),
    ("tray.wake_all", "Разбудить все сейчас", "Wake all now"),
    ("tray.pause", "Пауза", "Pause"),
    ("tray.resume", "Возобновить", "Resume"),
    ("tray.logs", "Открыть логи", "Open logs"),
    ("tray.quit", "Выход", "Quit"),
    ("tray.tooltip.idle", "Resumer — ждёт лимит", "Resumer — watching limits"),
    ("tray.tooltip.paused", "Resumer — на паузе", "Resumer — paused"),
    (
        "tray.limits",
        "5 ч: осталось {five}% · сброс {five_in}; неделя: осталось {week}% · сброс {week_in}",
        "5 h: {five}% left · resets {five_in}; week: {week}% left · resets {week_in}",
    ),
    ("notify.woke", "Разбудил окон: {n}", "Woke {n} window(s)"),
    (
        "notify.limit_active",
        "Лимит ещё не сброшен, жду до {time}",
        "Limit not reset yet, waiting until {time}",
    ),
    ("notify.update", "Доступна версия {v}", "Version {v} is available"),
    (
        "notify.nothing",
        "Нет окон Claude Code для пробуждения",
        "No Claude Code windows to wake",
    ),
    (
        "confirm.limit",
        "Лимит ещё не сброшен до {time}. Всё равно отправить?",
        "The limit is not reset until {time}. Send anyway?",
    ),
    (
        "run.summary",
        "Найдено окон: {found}; отправлено: {sent}; пропущено: {skipped}; ошибок: {failed}",
        "Windows found: {found}; sent: {sent}; skipped: {skipped}; failed: {failed}",
    ),
    (
        "skip.needs_input",
        "ждёт ответа — не трогал",
        "waiting for an answer — left alone",
    ),
    (
        "skip.own",
        "это сессия, из которой запущен Resumer",
        "the session Resumer was launched from",
    ),
    ("skip.excluded", "папка в исключениях", "folder is excluded"),
    (
        "about.privacy",
        "Resumer читает только данные хуков и статус-строки. Содержимое сессий не читается и не записывается.",
        "Resumer reads only hook and status-line data. Session contents are never read or stored.",
    ),
    ("demo.badge", "ДЕМО — данные выдуманы", "DEMO — made-up data"),
    ("update.available", "Доступна версия {v}", "Version {v} is available"),
    (
        "update.latest",
        "Установлена последняя версия",
        "You are on the latest version",
    ),
    ("update.failed", "Не удалось обновиться", "Update failed"),
];

pub fn t(lang: Lang, key: &str) -> &'static str {
    TABLE
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, ru, en)| if lang == Lang::Ru { *ru } else { *en })
        .unwrap_or("?")
}

/// `{name}` placeholders.
pub fn fmt(lang: Lang, key: &str, args: &[(&str, String)]) -> String {
    let mut s = t(lang, key).to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

pub fn status_label(lang: Lang, s: crate::state::Status) -> &'static str {
    t(lang, &format!("status.{}", s.as_str()))
}

/// "2 ч 05 м" / "2 h 05 m"
pub fn duration(lang: Lang, secs: i64) -> String {
    let s = secs.max(0);
    let (h, m, sec) = (s / 3600, s % 3600 / 60, s % 60);
    let (hh, mm, ss) = if lang == Lang::Ru {
        ("ч", "м", "с")
    } else {
        ("h", "m", "s")
    };
    if s >= 86_400 {
        let d = s / 86_400;
        let dd = if lang == Lang::Ru { "д" } else { "d" };
        format!("{d} {dd} {:02} {hh}", h % 24)
    } else if h > 0 {
        format!("{h} {hh} {m:02} {mm}")
    } else if m > 0 {
        format!("{m} {mm} {sec:02} {ss}")
    } else {
        format!("{sec} {ss}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_unique_and_translated() {
        let mut seen = std::collections::HashSet::new();
        for (k, ru, en) in TABLE {
            assert!(seen.insert(*k), "duplicate key {k}");
            assert!(!ru.is_empty() && !en.is_empty(), "{k}");
            let ph = |s: &str| {
                let mut v: Vec<String> = s
                    .split('{')
                    .skip(1)
                    .filter_map(|p| p.split('}').next())
                    .map(String::from)
                    .collect();
                v.sort();
                v
            };
            assert_eq!(ph(ru), ph(en), "placeholders differ in {k}");
        }
    }

    #[test]
    fn lookup_and_format() {
        assert_eq!(t(Lang::En, "btn.pause"), "Pause");
        assert_eq!(t(Lang::Ru, "btn.pause"), "Пауза");
        assert_eq!(t(Lang::Ru, "nope"), "?");
        assert_eq!(fmt(Lang::En, "notify.woke", &[("n", "4".into())]), "Woke 4 window(s)");
        assert_eq!(status_label(Lang::En, crate::state::Status::NeedsInput), "needs input");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(Lang::En, 7500), "2 h 05 m");
        assert_eq!(duration(Lang::Ru, 125), "2 м 05 с");
        assert_eq!(duration(Lang::En, 9), "9 s");
        assert_eq!(duration(Lang::En, 90_000), "1 d 01 h");
        assert_eq!(duration(Lang::En, -5), "0 s");
    }
}
