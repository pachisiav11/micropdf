//! Single instance: the first micropdf process listens on a per-user named pipe; later launches
//! hand their file arguments to it and exit, so files open as tabs in one window.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!(r"\\.\pipe\micropdf-{user}")
}

/// Sends `paths` to a running instance. Returns false if none is running.
pub fn hand_off(paths: &[PathBuf]) -> bool {
    let Ok(mut pipe) = std::fs::OpenOptions::new().write(true).open(pipe_name()) else {
        return false;
    };
    // Let the running instance bring its window to the front.
    // SAFETY: plain Win32 call with no pointers.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let mut message = String::from("micropdf/1\n");
    for p in paths {
        let absolute = std::path::absolute(p).unwrap_or_else(|_| p.clone());
        message.push_str(&absolute.to_string_lossy());
        message.push('\n');
    }
    pipe.write_all(message.as_bytes()).is_ok()
}

/// Listens for later launches on a background thread. `on_open` gets their paths (possibly
/// none, meaning "bring the window forward").
pub fn listen(on_open: impl Fn(Vec<PathBuf>) + Send + 'static) {
    let name: Vec<u16> = OsStr::new(&pipe_name()).encode_wide().chain([0]).collect();
    std::thread::Builder::new()
        .name("mp-instance".into())
        .spawn(move || {
            loop {
                // SAFETY: `name` is NUL-terminated and outlives the call.
                let handle = unsafe {
                    CreateNamedPipeW(
                        name.as_ptr(),
                        PIPE_ACCESS_INBOUND,
                        PIPE_TYPE_BYTE
                            | PIPE_READMODE_BYTE
                            | PIPE_WAIT
                            | PIPE_REJECT_REMOTE_CLIENTS,
                        PIPE_UNLIMITED_INSTANCES,
                        0,
                        64 * 1024,
                        0,
                        std::ptr::null(),
                    )
                };
                if handle == INVALID_HANDLE_VALUE {
                    return;
                }
                // SAFETY: valid pipe handle; blocking connect with no OVERLAPPED.
                let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
                    || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                // SAFETY: we own the handle; File closes it on drop.
                let mut pipe = unsafe { std::fs::File::from_raw_handle(handle as _) };
                if !connected {
                    continue;
                }
                let mut message = String::new();
                if pipe.read_to_string(&mut message).is_err() {
                    continue;
                }
                let mut lines = message.lines();
                if lines.next() != Some("micropdf/1") {
                    continue;
                }
                on_open(lines.filter(|l| !l.is_empty()).map(PathBuf::from).collect());
            }
        })
        .expect("failed to spawn instance listener");
}
