//! Convert: exports the document to Word, Excel, PowerPoint, OpenDocument, images, text, a web
//! page or Markdown, and installs or removes the LibreOffice add-on that Office files and
//! PowerPoint need when Microsoft Office is not there. Making a PDF of other files is Combine
//! without the open document (tools.rs).

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use mp_engine::{Export, addon_soffice, addons_dir, libreoffice_dir, soffice};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider, BCryptCreateHash, BCryptDestroyHash,
    BCryptFinishHash, BCryptHashData, BCryptOpenAlgorithmProvider,
};

use crate::tools::{Done, Form, save_dialog, stem};
use crate::viewer::{self, App, file_name};

type Kind = (&'static str, &'static [&'static str]);

const FORMATS: [(&str, Export, Kind); 9] = [
    ("export-word", Export::Word, ("Word documents", &["docx"])),
    (
        "export-excel",
        Export::Excel,
        ("Excel workbooks", &["xlsx"]),
    ),
    (
        "export-powerpoint",
        Export::PowerPoint,
        ("PowerPoint presentations", &["pptx"]),
    ),
    (
        "export-odt",
        Export::OpenDocument,
        ("OpenDocument text", &["odt"]),
    ),
    ("export-png", Export::Png, ("PNG images", &["png"])),
    ("export-jpeg", Export::Jpeg, ("JPEG images", &["jpg"])),
    ("export-text", Export::Text, ("Text files", &["txt"])),
    ("export-html", Export::Html, ("Web pages", &["html"])),
    (
        "export-markdown",
        Export::Markdown,
        ("Markdown files", &["md"]),
    ),
];

/// Where LibreOffice's releases are published.
const STABLE: &str = "https://download.documentfoundation.org/libreoffice/stable/";

static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Convert command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    if id == "addon" {
        addon(app);
        return true;
    }
    let Some(&(_, format, kind)) = FORMATS.iter().find(|f| f.0 == id) else {
        return false;
    };
    let Some((doc, path, ..)) = app.reading() else {
        return true;
    };
    let name = format!("{}.{}", stem(&path), format.extension());
    let engine = app.engine();
    save_dialog("Export", &path, name, kind, move |target| {
        let files = engine.export(doc, format, target)?;
        Ok(match files.as_slice() {
            [one] => format!("Saved {}", file_name(one)),
            [first, .., last] => format!(
                "Saved {} images, {} to {}",
                files.len(),
                file_name(first),
                file_name(last)
            ),
            [] => String::new(),
        })
    });
    true
}

/// Offers to install the LibreOffice add-on, or to remove it.
fn addon(app: &mut App) {
    if INSTALLING.load(Ordering::SeqCst) {
        app.status("The LibreOffice add-on is being installed".into());
        return;
    }
    let dir = libreoffice_dir();
    if addon_soffice().is_some() {
        app.show_form(
            Form::Addon(false),
            "Remove the LibreOffice add-on?",
            &format!(
                "Office files then need Microsoft Office or LibreOffice to become PDFs. Its \
                 folder, {}, goes with everything in it.",
                dir.display()
            ),
            "Remove",
            Vec::new(),
        );
    } else if soffice().is_some() {
        app.status("LibreOffice is installed on this computer, so the add-on is not needed".into());
    } else {
        app.show_form(
            Form::Addon(true),
            "Install the LibreOffice add-on?",
            &format!(
                "Without Microsoft Office, Office files become PDFs, and PDFs PowerPoint files, \
                 in LibreOffice. micropdf downloads it (about 350 MB) from The Document \
                 Foundation, checks it against the checksum published with it, and unpacks it \
                 for you alone in {}; no admin rights are needed. Remove it here at any time.",
                dir.display()
            ),
            "Install",
            Vec::new(),
        );
    }
}

/// The add-on form was accepted: installs the add-on on a thread, or removes it.
pub(crate) fn run_addon(app: &mut App, install: bool) -> Done {
    if !install {
        std::fs::remove_dir_all(addons_dir())?;
        app.status("Removed the LibreOffice add-on".into());
        return Ok(());
    }
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    app.status("Finding LibreOffice's newest release\u{2026}".into());
    std::thread::spawn(|| {
        let result = download_and_unpack();
        INSTALLING.store(false, Ordering::SeqCst);
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| match result {
                Ok(version) => app.status(format!("Installed LibreOffice {version} as an add-on")),
                Err(e) => app.message("Could not install the LibreOffice add-on", e),
            });
        });
    });
    Ok(())
}

fn progress(text: String) {
    let _ = slint::invoke_from_event_loop(move || {
        viewer::with(|app| app.status(text));
    });
}

fn get_text(url: &str) -> Result<String, String> {
    let mut body = Vec::new();
    mp_ai::get(url, &mut |b, _| {
        body.extend_from_slice(b);
        Ok(())
    })?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Downloads the newest stable LibreOffice for 64-bit Windows, checks it against the SHA-256
/// published beside it, and unpacks it with an administrative install, which copies the
/// files out without installing anything. Returns the version.
fn download_and_unpack() -> Result<String, String> {
    let index = get_text(STABLE)?;
    let version = index
        .split("href=\"")
        .filter_map(|s| s.split_once('/').map(|(v, _)| v))
        .filter_map(|v| {
            let parts: Option<Vec<u32>> = v.split('.').map(|p| p.parse().ok()).collect();
            parts.map(|p| (p, v))
        })
        .max()
        .map(|(_, v)| v.to_owned())
        .ok_or("Could not find LibreOffice's newest release.")?;
    let name = format!("LibreOffice_{version}_Win_x86-64.msi");
    let url = format!("{STABLE}{version}/win/x86_64/{name}");
    let sum = get_text(&format!("{url}.sha256"))?;
    let expected = sum
        .split_whitespace()
        .next()
        .filter(|h| h.len() == 64)
        .ok_or("Could not read the checksum of the download.")?
        .to_ascii_lowercase();

    let io = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(addons_dir()).map_err(io)?;
    let msi = addons_dir().join(&name);
    let result = (|| {
        let mut file = BufWriter::new(File::create(&msi).map_err(io)?);
        let (mut done, mut shown) = (0u64, u64::MAX);
        mp_ai::get(&url, &mut |b, total| {
            file.write_all(b).map_err(io)?;
            done += b.len() as u64;
            let percent = (done * 100).checked_div(total).unwrap_or(0);
            if percent != shown {
                shown = percent;
                progress(format!("Downloading LibreOffice {version}: {percent}%"));
            }
            Ok(())
        })?;
        file.flush().map_err(io)?;
        drop(file);
        if hex(&sha256(File::open(&msi).map_err(io)?)?) != expected {
            return Err(
                "The download does not match the checksum The Document Foundation \
                        publishes, so nothing was installed."
                    .to_owned(),
            );
        }
        progress("Unpacking LibreOffice\u{2026}".into());
        Command::new("msiexec")
            .arg("/a")
            .arg(&msi)
            .arg("/qn")
            .raw_arg(format!("TARGETDIR=\"{}\"", libreoffice_dir().display()))
            .status()
            .map_err(io)?;
        addon_soffice()
            .map(|_| version.clone())
            .ok_or_else(|| "Unpacking LibreOffice failed.".to_owned())
    })();
    let _ = std::fs::remove_file(&msi);
    result
}

pub(crate) fn sha256(mut input: impl Read) -> Result<[u8; 32], String> {
    let mut buffer = vec![0u8; 1 << 20];
    let mut digest = [0u8; 32];
    unsafe {
        let mut algorithm = null_mut();
        if BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_SHA256_ALGORITHM, null(), 0) != 0 {
            return Err("Windows could not hash the download.".into());
        }
        let mut hash = null_mut();
        let mut ok = BCryptCreateHash(algorithm, &mut hash, null_mut(), 0, null(), 0, 0) == 0;
        while ok {
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            ok = BCryptHashData(hash, buffer.as_ptr(), n as u32, 0) == 0;
        }
        ok = ok && BCryptFinishHash(hash, digest.as_mut_ptr(), 32, 0) == 0;
        BCryptDestroyHash(hash);
        BCryptCloseAlgorithmProvider(algorithm, 0);
        if !ok {
            return Err("Windows could not hash the download.".into());
        }
    }
    Ok(digest)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{hex, sha256};

    #[test]
    fn sha256_matches_a_known_digest() {
        assert_eq!(
            hex(&sha256(&b"abc"[..]).unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
