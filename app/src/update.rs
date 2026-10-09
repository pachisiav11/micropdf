//! Updates, only when asked: Check for updates (or, if turned on, a check at start) reads the
//! manifest of the latest GitHub release and its signature, made with the release key whose
//! public half is `KEY`. A newer version is offered; its zip is downloaded, checked against the
//! manifest's SHA-256 and unpacked, and its installer runs once micropdf has closed.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_ECDSA_PUBLIC_P256_MAGIC,
    BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptImportKeyPair,
    BCryptOpenAlgorithmProvider, BCryptVerifySignature,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::convert::{hex, sha256};
use crate::tools::{Done, Form};
use crate::viewer::{self, App};

const RELEASES: &str = "https://github.com/pachisiav11/micropdf/releases/";

/// The update key's public half: the P-256 point's X, then Y (examples/sign_update.rs).
const KEY: [u8; 64] = [
    0xa6, 0x06, 0x91, 0x63, 0xf7, 0x55, 0x1e, 0x61, 0x75, 0x79, 0x11, 0x2b, 0x26, 0x4f, 0x8f, 0xe6,
    0x10, 0x36, 0x6e, 0x56, 0x1a, 0x90, 0x22, 0xab, 0x65, 0xab, 0xd1, 0x32, 0x01, 0xfe, 0x00, 0xdf,
    0xbe, 0xc0, 0x02, 0x00, 0x6c, 0xfe, 0x7c, 0x42, 0x25, 0x8a, 0x64, 0x1d, 0x4f, 0xf2, 0xcd, 0x2d,
    0x48, 0xea, 0xf5, 0xb5, 0xd7, 0x7b, 0xd5, 0x42, 0x91, 0xa9, 0x67, 0x29, 0x85, 0x92, 0x2e, 0xf0,
];

#[derive(Deserialize)]
struct Manifest {
    version: String,
    url: String,
    sha256: String,
}

static BUSY: AtomicBool = AtomicBool::new(false);
/// The release offered, until the user takes it.
static OFFERED: Mutex<Option<Manifest>> = Mutex::new(None);
/// The unpacked installer, run once the app has closed.
static READY: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Update command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    match id {
        "update-check" => check(true),
        "update-auto" => {
            app.settings.check_updates = !app.settings.check_updates;
            app.save_settings();
            app.status(
                if app.settings.check_updates {
                    "micropdf checks for updates when it starts"
                } else {
                    "micropdf checks for updates only when you ask"
                }
                .into(),
            );
        }
        _ => return false,
    }
    true
}

/// Checks at start when the user turned that on.
pub fn start(app: &App) {
    if app.settings.check_updates {
        check(false);
    }
}

/// Runs the downloaded installer, if any, as the app ends.
pub fn after_exit() {
    if let Some(exe) = READY.lock().ok().and_then(|mut r| r.take()) {
        let _ = Command::new(exe).arg("--install").spawn();
    }
}

fn tell(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        viewer::with(f);
    });
}

fn check(asked: bool) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let result = latest();
        BUSY.store(false, Ordering::SeqCst);
        tell(move |app| match result {
            Ok(Some(release)) => {
                let title = format!("micropdf {} is out", release.version);
                *OFFERED.lock().unwrap() = Some(release);
                app.show_form(
                    Form::Update,
                    &title,
                    "It is downloaded from GitHub and checked now, and installed when you close \
                     micropdf.",
                    "Update",
                    Vec::new(),
                );
            }
            Ok(None) if asked => app.status(format!(
                "micropdf {} is the latest version",
                env!("CARGO_PKG_VERSION")
            )),
            Err(e) if asked => app.message("Could not check for updates", e),
            _ => {}
        });
    });
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    mp_ai::get(url, &mut |b, _| {
        body.extend_from_slice(b);
        Ok(())
    })?;
    Ok(body)
}

/// The latest release's manifest if it is signed with the release key and newer than this
/// version.
fn latest() -> Result<Option<Manifest>, String> {
    let manifest = format!("{RELEASES}latest/download/latest.json");
    let (json, signature) = (fetch(&manifest)?, fetch(&format!("{manifest}.sig"))?);
    if !verify(&KEY, &json, &signature) {
        return Err("The update's signature does not match micropdf's release key.".into());
    }
    let release: Manifest = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
    Ok(newer(&release.version, env!("CARGO_PKG_VERSION")).then_some(release))
}

fn newer(version: &str, than: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(version) > parse(than)
}

/// Whether `signature` is `key`'s ECDSA signature of `message`'s SHA-256.
fn verify(key: &[u8; 64], message: &[u8], signature: &[u8]) -> bool {
    let Ok(digest) = sha256(message) else {
        return false;
    };
    let mut blob = Vec::with_capacity(72);
    blob.extend_from_slice(&BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes());
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(key);
    // SAFETY: the buffers hold the lengths passed with them; the handles come from CNG and are
    // released before returning.
    unsafe {
        let mut algorithm = null_mut();
        if BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_ECDSA_P256_ALGORITHM, null(), 0) != 0
        {
            return false;
        }
        let mut handle = null_mut();
        let ok = BCryptImportKeyPair(
            algorithm,
            null_mut(),
            BCRYPT_ECCPUBLIC_BLOB,
            &mut handle,
            blob.as_ptr(),
            blob.len() as u32,
            0,
        ) == 0
            && BCryptVerifySignature(
                handle,
                null(),
                digest.as_ptr(),
                32,
                signature.as_ptr(),
                signature.len() as u32,
                0,
            ) == 0;
        if !handle.is_null() {
            BCryptDestroyKey(handle);
        }
        BCryptCloseAlgorithmProvider(algorithm, 0);
        ok
    }
}

/// The user took the update offered: downloads and unpacks it on a thread.
pub(crate) fn download(app: &mut App) -> Done {
    let Some(release) = OFFERED.lock().unwrap().take() else {
        return Ok(());
    };
    if BUSY.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    app.status("Downloading the update\u{2026}".into());
    std::thread::spawn(move || {
        let result = unpack(&release);
        BUSY.store(false, Ordering::SeqCst);
        tell(move |app| match result {
            Ok(exe) => {
                *READY.lock().unwrap() = Some(exe);
                app.status(format!(
                    "micropdf {} installs when you close micropdf",
                    release.version
                ));
            }
            Err(e) => app.message("Could not update", e),
        });
    });
    Ok(())
}

fn unpack(release: &Manifest) -> Result<PathBuf, String> {
    if !release.url.starts_with(&format!("{RELEASES}download/")) {
        return Err("The update is not one of micropdf's releases.".into());
    }
    let io = |e: std::io::Error| e.to_string();
    let dir = std::env::temp_dir().join(format!("micropdf-{}", release.version));
    std::fs::create_dir_all(&dir).map_err(io)?;
    let zip = dir.join("update.zip");
    let mut file = BufWriter::new(File::create(&zip).map_err(io)?);
    let (mut done, mut shown) = (0u64, u64::MAX);
    mp_ai::get(&release.url, &mut |b, total| {
        file.write_all(b).map_err(io)?;
        done += b.len() as u64;
        let percent = (done * 100).checked_div(total).unwrap_or(0);
        if percent != shown {
            shown = percent;
            let text = format!("Downloading micropdf {}: {percent}%", release.version);
            tell(move |app| app.status(text));
        }
        Ok(())
    })?;
    file.flush().map_err(io)?;
    drop(file);
    if hex(&sha256(File::open(&zip).map_err(io)?)?) != release.sha256.to_lowercase() {
        return Err("The download does not match the release, so nothing was installed.".into());
    }
    let files = dir.join("files");
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
    let unzipped = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!(
            "Expand-Archive -Force -LiteralPath {} -DestinationPath {}",
            quote(&zip),
            quote(&files)
        ))
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(io)?;
    let exe = files.join("micropdf.exe");
    if !unzipped.success() || !exe.is_file() {
        return Err("The update could not be unpacked.".into());
    }
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest signed with the release key by examples/sign_update.rs.
    const SAMPLE: &str = r#"{"version":"9.9.9","url":"https://github.com/pachisiav11/micropdf/releases/download/v9.9.9/micropdf-9.9.9-windows-x64.zip","sha256":"00"}"#;
    const SIGNATURE: &str = "93898e8b9b534853afaa2f21e29f03e0f58868f6c1193088a0062de8b6e26c6d4fca3661407f3ed1d62ca8b70fee36afd59b5d173cd143de44412fa15367b986";

    #[test]
    fn only_the_release_key_signs_a_manifest() {
        let signature: Vec<u8> = (0..64)
            .map(|i| u8::from_str_radix(&SIGNATURE[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        assert!(verify(&KEY, SAMPLE.as_bytes(), &signature));
        let changed = SAMPLE.replace("9.9.9", "9.9.8");
        assert!(!verify(&KEY, changed.as_bytes(), &signature));
        let release: Manifest = serde_json::from_str(SAMPLE).unwrap();
        assert!(newer(&release.version, env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn later_versions_are_newer() {
        assert!(newer("1.0.1", "1.0.0"));
        assert!(newer("v1.10.0", "1.9.3"));
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("0.9", "1.0.0"));
    }
}
