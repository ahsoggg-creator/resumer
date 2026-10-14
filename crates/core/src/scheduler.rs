//! Background scheduler: sleeps until the next computed wake-up (no periodic polling), and re-plans when
//! a state file changes, the config is saved, or the computer resumes from sleep/hibernation.
use crate::config::Config;
use crate::engine::{Injector, execute_due};
use crate::logger::log;
use crate::now_epoch;
use crate::proc;
use crate::schedule::plan;
use crate::state::StateStore;
use crate::targets::RunSummary;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Longest single wait even when nothing is pending (a safety net, not a polling interval).
const IDLE_WAIT: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, Default)]
pub struct SchedStatus {
    pub next_wake: Option<i64>,
    pub last_run: Option<(i64, RunSummary)>,
}

#[derive(Clone)]
pub struct SchedulerHandle {
    wake: Sender<()>,
    stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<SchedStatus>>,
}

impl SchedulerHandle {
    /// Ask the scheduler to re-plan now (config or sessions changed).
    pub fn poke(&self) {
        let _ = self.wake.send(());
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.poke();
    }

    pub fn snapshot(&self) -> SchedStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Starts the scheduler thread. `on_run` is called after each automatic wake-up.
pub fn spawn(injector: Injector, on_run: impl Fn(&RunSummary) + Send + 'static) -> SchedulerHandle {
    let (tx, rx) = channel::<()>();
    let handle = SchedulerHandle {
        wake: tx.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        status: Arc::new(Mutex::new(SchedStatus::default())),
    };
    watch_dir(StateStore::default_store().dir().to_path_buf(), tx.clone());
    watch_resume(tx);
    let h = handle.clone();
    std::thread::Builder::new()
        .name("resumer-scheduler".into())
        .spawn(move || run(h, rx, injector, on_run))
        .expect("spawn scheduler");
    handle
}

fn run(h: SchedulerHandle, rx: Receiver<()>, injector: Injector, on_run: impl Fn(&RunSummary)) {
    log("scheduler started");
    let store = StateStore::default_store();
    // (session, reset) pairs we already tried: a failed send must not be retried in a tight loop.
    let mut attempted: HashSet<(String, i64)> = HashSet::new();
    let never_cancel = AtomicBool::new(false);
    loop {
        if h.stop.load(Ordering::SeqCst) {
            break;
        }
        let cfg = Config::load();
        store.prune_dead(proc::is_alive);
        let now = now_epoch();
        let mut sched = plan(&store.list(), &cfg, now);
        sched
            .due
            .retain(|d| !attempted.contains(&(d.session_id.clone(), d.reset_epoch)));
        if let Ok(mut s) = h.status.lock() {
            s.next_wake = sched.next_wake;
        }
        if !sched.due.is_empty() {
            for d in &sched.due {
                attempted.insert((d.session_id.clone(), d.reset_epoch));
            }
            let sum = execute_due(&sched, &cfg, &store, injector.clone(), &never_cancel);
            if let Ok(mut s) = h.status.lock() {
                s.last_run = Some((now_epoch(), sum.clone()));
            }
            on_run(&sum);
            continue;
        }
        let wait = sched
            .next_wake
            .map(|t| Duration::from_secs((t - now).max(1) as u64))
            .unwrap_or(IDLE_WAIT)
            .min(IDLE_WAIT);
        match rx.recv_timeout(wait) {
            Ok(()) => {
                // Coalesce bursts of file-change events before re-planning.
                std::thread::sleep(Duration::from_millis(250));
                while rx.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    log("scheduler stopped");
}

/// Sends `()` whenever something in `dir` changes (event-driven, no polling).
fn watch_dir(dir: std::path::PathBuf, tx: Sender<()>) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW,
        FindNextChangeNotification,
    };
    use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    let _ = std::fs::create_dir_all(&dir);
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        FindFirstChangeNotificationW(
            wide.as_ptr(),
            0,
            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        log("cannot watch the state folder");
        return;
    }
    let raw = handle as usize;
    std::thread::Builder::new()
        .name("resumer-watch".into())
        .spawn(move || {
            let h = raw as *mut c_void;
            loop {
                if unsafe { WaitForSingleObject(h, INFINITE) } != WAIT_OBJECT_0 {
                    break;
                }
                if tx.send(()).is_err() {
                    break;
                }
                if unsafe { FindNextChangeNotification(h) } == 0 {
                    break;
                }
            }
        })
        .ok();
}

/// Sends `()` when Windows resumes from sleep or hibernation: monotonic timers do not count that time.
fn watch_resume(tx: Sender<()>) {
    use windows_sys::Win32::System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
    };

    unsafe extern "system" fn callback(context: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
        if (kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND) && !context.is_null() {
            let tx = unsafe { &*(context as *const Sender<()>) };
            let _ = tx.send(());
        }
        0
    }

    // Both the context and the parameter block must outlive the registration: leak them on purpose.
    let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;
    let params = Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(callback),
        Context: ctx,
    }));
    let mut reg: *mut c_void = std::ptr::null_mut();
    let rc = unsafe { PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, params as *mut c_void, &mut reg) };
    if rc != 0 {
        log(&format!("resume notification unavailable (error {rc})"));
    }
}
