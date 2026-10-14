//! Process discovery: find Claude Code processes, walk ancestors, guard against PID reuse.
//! The matching/ancestry logic is pure; the Win32 part only produces `ProcessInfo` values.
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    /// Image name, e.g. `claude.exe`.
    pub name: String,
    /// Creation time as FILETIME ticks (0 = unknown).
    pub started: u64,
    pub cwd: Option<String>,
    pub cmdline: Option<String>,
    /// An ancestor is `WindowsTerminal.exe`/`OpenConsole.exe`, i.e. the process lives in a WT tab.
    pub in_windows_terminal: bool,
}

pub fn name_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// `claude.exe` (native install) or `node.exe` running the `claude-code` package.
pub fn is_claude_image(name: &str, cmdline: Option<&str>) -> bool {
    if name_eq(name, "claude.exe") {
        return true;
    }
    name_eq(name, "node.exe") && cmdline.is_some_and(|c| c.to_ascii_lowercase().contains("claude-code"))
}

/// Pids from `start` upwards (inclusive), stopping at cycles and after 64 levels.
pub fn ancestors(start: u32, parents: &HashMap<u32, u32>) -> Vec<u32> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = start;
    while cur != 0 && seen.insert(cur) && out.len() < 64 {
        out.push(cur);
        match parents.get(&cur) {
            Some(&p) => cur = p,
            None => break,
        }
    }
    out
}

/// First process on the way up (starting at `start`) that is Claude Code.
pub fn find_claude_ancestor(start: u32, procs: &HashMap<u32, ProcessInfo>) -> Option<u32> {
    let parents: HashMap<u32, u32> = procs.values().map(|p| (p.pid, p.ppid)).collect();
    ancestors(start, &parents).into_iter().find(|pid| {
        procs
            .get(pid)
            .is_some_and(|p| is_claude_image(&p.name, p.cmdline.as_deref()))
    })
}

pub fn lives_in_windows_terminal(pid: u32, procs: &HashMap<u32, ProcessInfo>) -> bool {
    let parents: HashMap<u32, u32> = procs.values().map(|p| (p.pid, p.ppid)).collect();
    ancestors(pid, &parents).into_iter().skip(1).any(|a| {
        procs
            .get(&a)
            .is_some_and(|p| name_eq(&p.name, "WindowsTerminal.exe") || name_eq(&p.name, "OpenConsole.exe"))
    })
}

/// Claude Code sessions among `all` (helper processes spawned by another Claude are dropped).
pub fn pick_claude_sessions(all: &[ProcessInfo]) -> Vec<ProcessInfo> {
    let map: HashMap<u32, ProcessInfo> = all.iter().cloned().map(|p| (p.pid, p)).collect();
    let mut out: Vec<ProcessInfo> = all
        .iter()
        .filter(|p| is_claude_image(&p.name, p.cmdline.as_deref()))
        .filter(|p| {
            !map.get(&p.ppid)
                .is_some_and(|par| is_claude_image(&par.name, par.cmdline.as_deref()))
        })
        .cloned()
        .collect();
    for p in &mut out {
        p.in_windows_terminal = lives_in_windows_terminal(p.pid, &map);
    }
    out.sort_by_key(|p| p.pid);
    out
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    pub fn current_pid() -> u32 {
        unsafe { GetCurrentProcessId() }
    }

    fn wide_to_string(w: &[u16]) -> String {
        let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..end])
    }

    /// Cheap listing of every process: pid, parent, image name.
    pub fn snapshot() -> Vec<ProcessInfo> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut e: PROCESSENTRY32W = zeroed();
            e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e);
            while ok != 0 {
                out.push(ProcessInfo {
                    pid: e.th32ProcessID,
                    ppid: e.th32ParentProcessID,
                    name: wide_to_string(&e.szExeFile),
                    ..Default::default()
                });
                ok = Process32NextW(snap, &mut e);
            }
            CloseHandle(snap);
        }
        out
    }

    pub fn snapshot_map() -> HashMap<u32, ProcessInfo> {
        snapshot().into_iter().map(|p| (p.pid, p)).collect()
    }

    fn filetime_u64(ft: &FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    /// Creation time of `pid` (FILETIME ticks); `None` if the process does not exist or is not accessible.
    pub fn start_time(pid: u32) -> Option<u64> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = zeroed();
            let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
            CloseHandle(h);
            (ok != 0).then(|| filetime_u64(&c))
        }
    }

    /// Alive and (when `started` is known) still the same process, not a reused PID.
    pub fn is_alive(pid: u32, started: Option<u64>) -> bool {
        match (start_time(pid), started) {
            (None, _) => false,
            (Some(_), None) | (Some(_), Some(0)) => true,
            (Some(now), Some(then)) => now == then,
        }
    }

    unsafe fn read_mem(h: HANDLE, addr: usize, buf: *mut c_void, len: usize) -> bool {
        let mut read = 0usize;
        unsafe { ReadProcessMemory(h, addr as *const c_void, buf, len, &mut read) != 0 && read == len }
    }

    unsafe fn read_unicode_string(h: HANDLE, addr: usize) -> Option<String> {
        // UNICODE_STRING on 64-bit: u16 len, u16 max, 4 bytes pad, *u16 buffer.
        let mut raw = [0u8; 16];
        unsafe {
            if !read_mem(h, addr, raw.as_mut_ptr().cast(), 16) {
                return None;
            }
            let len = u16::from_le_bytes([raw[0], raw[1]]) as usize;
            let buf = usize::from_le_bytes(raw[8..16].try_into().ok()?);
            if len == 0 || buf == 0 || len > 64 * 1024 {
                return None;
            }
            let mut w = vec![0u16; len / 2];
            if !read_mem(h, buf, w.as_mut_ptr().cast(), len) {
                return None;
            }
            Some(String::from_utf16_lossy(&w))
        }
    }

    /// Command line and current directory read from the target's PEB (same user, not elevated).
    pub fn cmdline_and_cwd(pid: u32) -> (Option<String>, Option<String>) {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if h.is_null() {
                return (None, None);
            }
            let mut pbi: PROCESS_BASIC_INFORMATION = zeroed();
            let mut ret = 0u32;
            let st = NtQueryInformationProcess(
                h,
                ProcessBasicInformation,
                (&mut pbi as *mut PROCESS_BASIC_INFORMATION).cast(),
                size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                &mut ret,
            );
            let mut result = (None, None);
            if st >= 0 && !pbi.PebBaseAddress.is_null() {
                let mut params: usize = 0;
                // PEB.ProcessParameters @ 0x20; RTL_USER_PROCESS_PARAMETERS.CurrentDirectory @ 0x38, CommandLine @ 0x70.
                if read_mem(
                    h,
                    pbi.PebBaseAddress as usize + 0x20,
                    (&mut params as *mut usize).cast(),
                    8,
                ) && params != 0
                {
                    let cwd = read_unicode_string(h, params + 0x38).map(|s| s.trim_end_matches('\\').to_string());
                    result = (read_unicode_string(h, params + 0x70), cwd);
                }
            }
            CloseHandle(h);
            result
        }
    }

    /// Full scan for Claude Code sessions with command line, cwd and creation time filled in.
    pub fn scan_claude() -> Vec<ProcessInfo> {
        let mut all = snapshot();
        for p in &mut all {
            if name_eq(&p.name, "claude.exe") || name_eq(&p.name, "node.exe") {
                let (cmd, cwd) = cmdline_and_cwd(p.pid);
                p.cmdline = cmd;
                p.cwd = cwd;
                p.started = start_time(p.pid).unwrap_or(0);
            }
        }
        pick_claude_sessions(&all)
    }

    /// Pids of this process and all its ancestors: sessions we must never type into.
    pub fn own_ancestors() -> HashSet<u32> {
        let parents: HashMap<u32, u32> = snapshot().into_iter().map(|p| (p.pid, p.ppid)).collect();
        ancestors(current_pid(), &parents).into_iter().collect()
    }

    /// Still the same Claude Code process (name matches, PID not reused).
    pub fn still_claude(pid: u32, started: u64) -> bool {
        if !is_alive(pid, Some(started)) {
            return false;
        }
        let map = snapshot_map();
        let Some(p) = map.get(&pid) else { return false };
        if name_eq(&p.name, "claude.exe") {
            return true;
        }
        name_eq(&p.name, "node.exe") && is_claude_image(&p.name, cmdline_and_cwd(pid).0.as_deref())
    }

    /// For hooks: the Claude Code process above `start` as `(pid, creation time)`.
    pub fn claude_ancestor_of(start: u32) -> Option<(u32, u64)> {
        let mut map = snapshot_map();
        // node.exe candidates need their command line to be recognised.
        let parents: HashMap<u32, u32> = map.values().map(|p| (p.pid, p.ppid)).collect();
        for pid in ancestors(start, &parents) {
            if let Some(p) = map.get_mut(&pid)
                && name_eq(&p.name, "node.exe")
            {
                p.cmdline = cmdline_and_cwd(pid).0;
            }
        }
        let pid = find_claude_ancestor(start, &map)?;
        Some((pid, start_time(pid).unwrap_or(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str, cmd: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            name: name.into(),
            cmdline: cmd.map(Into::into),
            ..Default::default()
        }
    }

    fn tree() -> Vec<ProcessInfo> {
        vec![
            p(1, 0, "explorer.exe", None),
            p(10, 1, "WindowsTerminal.exe", None),
            p(11, 10, "OpenConsole.exe", None),
            p(12, 11, "pwsh.exe", None),
            p(13, 12, "claude.exe", Some("claude")),
            p(14, 13, "bash.exe", None),
            p(15, 14, "resumer-cli.exe", None),
            p(20, 1, "cmd.exe", None),
            p(
                21,
                20,
                "node.exe",
                Some("node C:\\npm\\@anthropic-ai\\claude-code\\cli.js"),
            ),
            p(22, 21, "claude.exe", Some("claude helper")),
            p(30, 1, "node.exe", Some("node server.js")),
        ]
    }

    #[test]
    fn hook_finds_claude_above_grandchild() {
        let m: HashMap<u32, ProcessInfo> = tree().into_iter().map(|p| (p.pid, p)).collect();
        assert_eq!(find_claude_ancestor(15, &m), Some(13));
        assert_eq!(find_claude_ancestor(30, &m), None);
        assert_eq!(find_claude_ancestor(21, &m), Some(21));
    }

    #[test]
    fn sessions_exclude_helpers_and_plain_node() {
        let s = pick_claude_sessions(&tree());
        let pids: Vec<u32> = s.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![13, 21]);
        assert!(s[0].in_windows_terminal);
        assert!(!s[1].in_windows_terminal);
    }

    #[test]
    fn ancestors_survive_cycles() {
        let parents: HashMap<u32, u32> = [(1, 2), (2, 3), (3, 1)].into_iter().collect();
        assert_eq!(ancestors(1, &parents), vec![1, 2, 3]);
    }

    #[cfg(windows)]
    #[test]
    fn own_process_is_visible_and_alive() {
        let me = current_pid();
        let st = start_time(me).expect("start time");
        assert!(is_alive(me, Some(st)));
        assert!(!is_alive(me, Some(st + 1)));
        assert!(snapshot().iter().any(|p| p.pid == me));
        assert!(own_ancestors().contains(&me));
    }
}
