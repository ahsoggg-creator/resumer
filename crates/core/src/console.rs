//! Typing into another process's console: `FreeConsole` + `AttachConsole(pid)` + `WriteConsoleInputW`.
//! The caller must not have a console it still needs (GUI process or a detached helper).
//! Text goes in as one batch; Enter follows as a separate call after a configurable delay, so that the
//! Claude Code TUI treats it as a keypress and not as the end of a multi-line paste.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyRec {
    pub down: bool,
    pub vk: u16,
    pub scan: u16,
    pub ch: u16,
    pub ctrl_state: u32,
}

#[derive(Clone, Debug)]
pub struct SendOptions {
    pub enter_delay_ms: u64,
    /// Send characters with virtual-key 0 instead of looking the key up in the current layout.
    pub raw_vk: bool,
    pub press_enter: bool,
}

impl Default for SendOptions {
    fn default() -> Self {
        SendOptions {
            enter_delay_ms: 80,
            raw_vk: false,
            press_enter: true,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SendError {
    #[error("nothing to send")]
    Empty,
    #[error("AttachConsole({pid}) failed, Win32 error {code}")]
    Attach { pid: u32, code: u32 },
    #[error("cannot open CONIN$, Win32 error {0}")]
    OpenInput(u32),
    #[error("WriteConsoleInputW failed, Win32 error {0}")]
    Write(u32),
    #[error("console input is only supported on Windows")]
    Unsupported,
}

pub const VK_RETURN: u16 = 0x0D;
const SHIFT_PRESSED: u32 = 0x0010;

/// Single-line text: line breaks would be sent as Enter and submit half a message.
pub fn sanitize_text(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\r', '\n'], " ").trim().to_string()
}

/// Key-down/key-up pair per UTF-16 unit. `lookup` maps a char to `(vk, scan, ctrl_state)` for the current layout.
pub fn text_records(text: &str, lookup: &dyn Fn(u16) -> Option<(u16, u16, u32)>) -> Vec<KeyRec> {
    let mut out = Vec::new();
    for unit in text.encode_utf16() {
        let (vk, scan, ctrl_state) = lookup(unit).unwrap_or((0, 0, 0));
        for down in [true, false] {
            out.push(KeyRec {
                down,
                vk,
                scan,
                ch: unit,
                ctrl_state,
            });
        }
    }
    out
}

pub fn enter_records() -> [KeyRec; 2] {
    [true, false].map(|down| KeyRec {
        down,
        vk: VK_RETURN,
        scan: 0x1C,
        ch: 0x0D,
        ctrl_state: 0,
    })
}

/// Maps the result of `VkKeyScanW` to `(vk, shift-state flags)`; `None` when the key needs Ctrl/Alt or is unknown.
pub fn decode_vk_scan(v: i16) -> Option<(u16, u32)> {
    if v == -1 {
        return None;
    }
    let vk = (v & 0xFF) as u16;
    let mods = ((v >> 8) & 0xFF) as u8;
    if vk == 0xFF || mods & 0b110 != 0 {
        return None;
    }
    Some((vk, if mods & 1 != 0 { SHIFT_PRESSED } else { 0 }))
}

#[cfg(windows)]
pub use win::send_text;

#[cfg(not(windows))]
pub fn send_text(_pid: u32, _text: &str, _opts: &SendOptions) -> Result<(), SendError> {
    Err(SendError::Unsupported)
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
        WriteConsoleInputW,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW, VkKeyScanW};

    /// `AttachConsole` is process-global: only one console at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn to_input(r: &KeyRec) -> INPUT_RECORD {
        INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: r.down as i32,
                    wRepeatCount: 1,
                    wVirtualKeyCode: r.vk,
                    wVirtualScanCode: r.scan,
                    uChar: KEY_EVENT_RECORD_0 { UnicodeChar: r.ch },
                    dwControlKeyState: r.ctrl_state,
                },
            },
        }
    }

    fn layout_lookup(unit: u16) -> Option<(u16, u16, u32)> {
        let (vk, state) = decode_vk_scan(unsafe { VkKeyScanW(unit) })?;
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        Some((vk, scan, state))
    }

    unsafe fn write_all(h: windows_sys::Win32::Foundation::HANDLE, recs: &[KeyRec]) -> Result<(), SendError> {
        let inputs: Vec<INPUT_RECORD> = recs.iter().map(to_input).collect();
        for chunk in inputs.chunks(256) {
            let mut written = 0u32;
            let ok = unsafe { WriteConsoleInputW(h, chunk.as_ptr(), chunk.len() as u32, &mut written) };
            if ok == 0 || written as usize != chunk.len() {
                return Err(SendError::Write(unsafe { GetLastError() }));
            }
        }
        Ok(())
    }

    pub fn send_text(pid: u32, text: &str, opts: &SendOptions) -> Result<(), SendError> {
        let text = sanitize_text(text);
        if text.is_empty() && !opts.press_enter {
            return Err(SendError::Empty);
        }
        let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            FreeConsole();
            if AttachConsole(pid) == 0 {
                return Err(SendError::Attach {
                    pid,
                    code: GetLastError(),
                });
            }
            let name: Vec<u16> = "CONIN$\0".encode_utf16().collect();
            let h = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                let code = GetLastError();
                FreeConsole();
                return Err(SendError::OpenInput(code));
            }
            let result = (|| {
                if !text.is_empty() {
                    let none: &dyn Fn(u16) -> Option<(u16, u16, u32)> = &|_| None;
                    let recs = if opts.raw_vk {
                        text_records(&text, none)
                    } else {
                        text_records(&text, &layout_lookup)
                    };
                    write_all(h, &recs)?;
                }
                if opts.press_enter {
                    std::thread::sleep(Duration::from_millis(opts.enter_delay_ms));
                    write_all(h, &enter_records())?;
                }
                Ok(())
            })();
            CloseHandle(h);
            FreeConsole();
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_down_up_pairs() {
        let r = text_records("ab", &|_| None);
        assert_eq!(r.len(), 4);
        assert!(r[0].down && !r[1].down && r[0].ch == 'a' as u16 && r[2].ch == 'b' as u16);
        assert!(r.iter().all(|k| k.vk == 0));
    }

    #[test]
    fn astral_chars_become_surrogate_pairs() {
        let r = text_records("😀", &|_| None);
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn enter_is_return_key() {
        let e = enter_records();
        assert!(e[0].down && !e[1].down && e[0].vk == VK_RETURN && e[0].ch == 13);
    }

    #[test]
    fn newlines_never_become_enter() {
        assert_eq!(sanitize_text("a\r\nb\nc\r"), "a b c");
    }

    #[test]
    fn vk_scan_decoding() {
        assert_eq!(decode_vk_scan(-1), None);
        assert_eq!(decode_vk_scan(0x0141), Some((0x41, SHIFT_PRESSED)));
        assert_eq!(decode_vk_scan(0x0041), Some((0x41, 0)));
        assert_eq!(decode_vk_scan(0x0651), None); // AltGr (ctrl+alt) combination
    }
}
