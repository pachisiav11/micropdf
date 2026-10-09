//! Registers micropdf as a PDF handler for the current user (no administrator rights needed).
//!
//! Windows does not let programs make themselves the default: registration adds micropdf to
//! "Open with" and to Settings > Default apps, where the user confirms the choice.

use std::io;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_NONE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW,
    RegSetValueExW,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

const PROG_ID: &str = "micropdf.pdf";
const CAPABILITIES: &str = r"Software\micropdf\Capabilities";

#[derive(Debug, PartialEq)]
pub(crate) enum Value {
    Text(String),
    Number(u32),
    /// An empty REG_NONE value, as `OpenWithProgids` expects.
    Empty,
}

/// One registry value under HKEY_CURRENT_USER; an empty name is the key's default value.
#[derive(Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) name: &'static str,
    pub(crate) value: Value,
}

fn entries(exe: &str) -> Vec<Entry> {
    let command = format!("\"{exe}\" \"%1\"");
    let text = |key: &str, name, value: &str| Entry {
        key: key.to_owned(),
        name,
        value: Value::Text(value.to_owned()),
    };
    let classes = |sub: &str| format!(r"Software\Classes\{sub}");
    vec![
        text(&classes(PROG_ID), "", "PDF Document"),
        text(
            &classes(&format!(r"{PROG_ID}\DefaultIcon")),
            "",
            &format!("\"{exe}\",0"),
        ),
        text(
            &classes(&format!(r"{PROG_ID}\shell\open\command")),
            "",
            &command,
        ),
        Entry {
            key: classes(r".pdf\OpenWithProgids"),
            name: PROG_ID,
            value: Value::Empty,
        },
        text(
            &classes(r"Applications\micropdf.exe"),
            "FriendlyAppName",
            "micropdf",
        ),
        text(
            &classes(r"Applications\micropdf.exe\SupportedTypes"),
            ".pdf",
            "",
        ),
        text(
            &classes(r"Applications\micropdf.exe\shell\open\command"),
            "",
            &command,
        ),
        text(CAPABILITIES, "ApplicationName", "micropdf"),
        text(
            CAPABILITIES,
            "ApplicationDescription",
            "Lightweight PDF reader and editor",
        ),
        text(
            &format!(r"{CAPABILITIES}\FileAssociations"),
            ".pdf",
            PROG_ID,
        ),
        text(r"Software\RegisteredApplications", "micropdf", CAPABILITIES),
    ]
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn check(status: u32) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

pub(crate) fn write(entry: &Entry) -> io::Result<()> {
    let key = wide(&entry.key);
    let mut handle: HKEY = std::ptr::null_mut();
    // SAFETY: `key` is NUL-terminated; `handle` receives an open key that is closed below.
    check(unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        )
    })?;
    let name = wide(entry.name);
    let (kind, data) = match &entry.value {
        Value::Text(text) => (REG_SZ, wide(text)),
        Value::Number(n) => (REG_DWORD, vec![*n as u16, (*n >> 16) as u16]),
        Value::Empty => (REG_NONE, Vec::new()),
    };
    // SAFETY: `handle` is open; `data` holds `data.len()` UTF-16 units.
    let status = unsafe {
        RegSetValueExW(
            handle,
            name.as_ptr(),
            0,
            kind,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    // SAFETY: `handle` was opened above.
    unsafe { RegCloseKey(handle) };
    check(status)
}

fn notify_shell() {
    // SAFETY: SHCNE_ASSOCCHANGED takes no item pointers.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
}

/// Runs the browser extension's bridge, when it ships beside the app, with `arg`.
fn bridge(arg: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    let bridge = std::env::current_exe()?.with_file_name("micropdf-bridge.exe");
    if !bridge.is_file() {
        return Ok(());
    }
    let status = std::process::Command::new(bridge)
        .arg(arg)
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("the browser extension bridge failed"))
    }
}

/// Adds micropdf to "Open with" for PDF files and to Default apps, for the running exe, and
/// lets the browser extension reach it.
pub fn register() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    for entry in entries(&exe.to_string_lossy()) {
        write(&entry)?;
    }
    notify_shell();
    bridge("--register")
}

/// Removes everything [`register`] wrote.
pub fn unregister() -> io::Result<()> {
    for tree in [
        format!(r"Software\Classes\{PROG_ID}"),
        r"Software\Classes\Applications\micropdf.exe".to_owned(),
        CAPABILITIES.to_owned(),
    ] {
        delete_tree(&tree);
    }
    for (key, name) in [
        (r"Software\Classes\.pdf\OpenWithProgids", PROG_ID),
        (r"Software\RegisteredApplications", "micropdf"),
    ] {
        let (key, name) = (wide(key), wide(name));
        // SAFETY: both strings are NUL-terminated.
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
    }
    notify_shell();
    bridge("--unregister")
}

/// Deletes `key` under HKEY_CURRENT_USER and all below it; a missing key is not an error worth
/// reporting.
pub(crate) fn delete_tree(key: &str) {
    let key = wide(key);
    // SAFETY: `key` is NUL-terminated.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
}

/// Whether PDF files are registered to open with this exe.
pub fn is_registered() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let key = wide(&format!(r"Software\Classes\{PROG_ID}\shell\open\command"));
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` holds `size` bytes; a NULL value name reads the default value.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }
    let len = (size as usize / 2).saturating_sub(1);
    let command = String::from_utf16_lossy(&buffer[..len]);
    command.eq_ignore_ascii_case(&format!("\"{}\" \"%1\"", exe.to_string_lossy()))
}

/// Settings > Apps > Default apps, opened at micropdf's page.
pub const DEFAULT_APPS_URI: &str = "ms-settings:defaultapps?registeredAppUser=micropdf";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_point_every_verb_at_the_exe() {
        let exe = r"C:\Apps\micropdf.exe";
        let list = entries(exe);
        let commands: Vec<_> = list
            .iter()
            .filter(|e| e.key.ends_with(r"shell\open\command"))
            .collect();
        assert_eq!(commands.len(), 2);
        for c in commands {
            assert_eq!(c.value, Value::Text(format!("\"{exe}\" \"%1\"")));
        }
        assert!(list.contains(&Entry {
            key: r"Software\Classes\.pdf\OpenWithProgids".into(),
            name: PROG_ID,
            value: Value::Empty,
        }));
        assert!(list.iter().all(|e| e.key.starts_with("Software\\")));
    }

    #[test]
    fn default_apps_finds_the_capabilities() {
        let list = entries("x");
        let registered = list
            .iter()
            .find(|e| e.key == r"Software\RegisteredApplications")
            .unwrap();
        assert_eq!(registered.value, Value::Text(CAPABILITIES.into()));
        assert!(list.iter().any(|e| e.key.starts_with(CAPABILITIES)
            && e.name == ".pdf"
            && e.value == Value::Text(PROG_ID.into())));
    }
}
