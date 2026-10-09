//! Installs micropdf for the current user, with no administrator rights: the release's files
//! go to %LOCALAPPDATA%\Programs\micropdf, with a Start menu entry, an entry in Settings > Apps
//! that uninstalls it, "Open with" for PDF files and the browser extension's bridge.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_YESNO, MessageBoxW,
};

use crate::assoc::{self, Entry, Value, wide};

/// What a release holds; the installer copies those it finds beside itself.
const FILES: [&str; 4] = [
    "micropdf.exe",
    "micropdf-bridge.exe",
    "LICENSE",
    "THIRD-PARTY-NOTICES.md",
];
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\micropdf";

/// %LOCALAPPDATA%\Programs\micropdf.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Programs\micropdf"))
}

fn shortcut() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs\micropdf.lnk"))
}

/// `--install`: copies the release into place and starts it from there, which finishes the
/// install. Run from there, finishes it. Returns whether to go on and open the app.
pub fn install() -> bool {
    match place() {
        Ok(open) => open,
        Err(e) => {
            alert(
                &format!("micropdf could not be installed: {e}"),
                MB_ICONERROR,
            );
            false
        }
    }
}

fn place() -> io::Result<bool> {
    let exe = std::env::current_exe()?;
    let from = exe.parent().ok_or(io::ErrorKind::NotFound)?;
    let to = dir().ok_or_else(|| io::Error::other("LOCALAPPDATA is not set"))?;
    if !same(from, &to) {
        std::fs::create_dir_all(&to)?;
        for name in FILES {
            let source = from.join(name);
            if source.is_file() {
                copy(&source, &to.join(name))?;
            }
        }
        Command::new(to.join("micropdf.exe"))
            .arg("--install")
            .spawn()?;
        return Ok(false);
    }
    assoc::register()?;
    if let Some(link) = shortcut() {
        let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "$s = (New-Object -ComObject WScript.Shell).CreateShortcut({}); \
                 $s.TargetPath = {}; $s.Save()",
                quote(&link),
                quote(&exe)
            ))
            .creation_flags(CREATE_NO_WINDOW)
            .status()?;
    }
    let size: u64 = FILES
        .iter()
        .filter_map(|name| std::fs::metadata(to.join(name)).ok())
        .map(|m| m.len())
        .sum();
    for entry in entries(&exe, &to, (size / 1024) as u32) {
        assoc::write(&entry)?;
    }
    Ok(true)
}

/// Copies `from` to `to`, waiting a while if `to` is in use: during an update the old app is
/// still closing.
fn copy(from: &Path, to: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(_) if tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    }
}

fn same(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The Settings > Apps entry.
fn entries(exe: &Path, dir: &Path, kilobytes: u32) -> Vec<Entry> {
    let exe = exe.to_string_lossy();
    let text = |name, value: String| Entry {
        key: UNINSTALL.to_owned(),
        name,
        value: Value::Text(value),
    };
    let number = |name, n| Entry {
        key: UNINSTALL.to_owned(),
        name,
        value: Value::Number(n),
    };
    vec![
        text("DisplayName", "micropdf".into()),
        text("DisplayVersion", env!("CARGO_PKG_VERSION").into()),
        text("Publisher", "micropdf".into()),
        text("DisplayIcon", format!("\"{exe}\",0")),
        text("InstallLocation", dir.to_string_lossy().into_owned()),
        text("UninstallString", format!("\"{exe}\" --uninstall")),
        text("URLInfoAbout", env!("CARGO_PKG_REPOSITORY").into()),
        number("NoModify", 1),
        number("NoRepair", 1),
        number("EstimatedSize", kilobytes),
    ]
}

/// `--uninstall`: takes out everything the install put in, once the user agrees. Settings and
/// the library stay, as the privacy page says.
pub fn uninstall() {
    if !ask("Remove micropdf from this computer? Your documents and settings stay.") {
        return;
    }
    let _ = assoc::unregister();
    if let Some(link) = shortcut() {
        let _ = std::fs::remove_file(link);
    }
    assoc::delete_tree(UNINSTALL);
    let (Ok(exe), Some(to)) = (std::env::current_exe(), dir()) else {
        return;
    };
    // Only the install folder goes, and only once this exe has ended and let go of it.
    if exe.parent().is_some_and(|from| same(from, &to)) {
        let _ = Command::new("cmd.exe")
            .raw_arg(format!(
                "/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
                to.display()
            ))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

fn alert(text: &str, icon: u32) -> i32 {
    let (text, title) = (wide(text), wide("micropdf"));
    // SAFETY: both strings are NUL-terminated; no owner window.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), icon) }
}

fn ask(text: &str) -> bool {
    alert(text, MB_YESNO | MB_ICONQUESTION) == IDYES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_uninstalls_with_the_installed_exe() {
        let exe = Path::new(r"C:\Users\a\AppData\Local\Programs\micropdf\micropdf.exe");
        let list = entries(exe, exe.parent().unwrap(), 30_000);
        assert!(list.iter().all(|e| e.key == UNINSTALL));
        assert!(list.contains(&Entry {
            key: UNINSTALL.into(),
            name: "UninstallString",
            value: Value::Text(format!("\"{}\" --uninstall", exe.display())),
        }));
        assert!(list.contains(&Entry {
            key: UNINSTALL.into(),
            name: "EstimatedSize",
            value: Value::Number(30_000),
        }));
    }
}
