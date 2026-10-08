//! micropdf-bridge: the native messaging host of the micropdf browser extension.
//!
//! Chrome, Edge and Brave start it when the extension connects and talk to it over stdin and
//! stdout: each message is a 32-bit little-endian length and that many bytes of JSON. The
//! extension sends a PDF as `begin`, base64 `chunk`s (each acknowledged, so the browser never
//! queues a whole file) and `end`. The bridge writes the file and then either opens it in
//! micropdf, handing it to the running app over the app's pipe or starting the app, or, for a
//! local PDF the extension edited, puts it in place of that file. `open` opens a local PDF in
//! micropdf as it is.
//!
//! `micropdf-bridge --register` tells the browsers where the bridge is; `--unregister` undoes it.

use std::fmt::Display;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

/// The name the extension connects to.
const HOST: &str = "com.micropdf.bridge";
/// The extension's ID, fixed by the key in its manifest.
const EXTENSION_ID: &str = "phhaejfhblmccnkhnhjflbhckkanlnki";
/// Chrome sends at most 64 MiB in one message.
const MAX_MESSAGE: usize = 64 << 20;
/// Where each browser looks for native messaging hosts, under HKEY_CURRENT_USER.
const BROWSERS: [&str; 3] = [
    r"Software\Google\Chrome\NativeMessagingHosts",
    r"Software\Microsoft\Edge\NativeMessagingHosts",
    r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
];

fn invalid(message: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// The next message, or None when the browser closed the connection.
fn read_message(input: &mut impl Read) -> io::Result<Option<Value>> {
    let mut len = [0; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(invalid(format!("a message of {len} bytes is too long")));
    }
    let mut body = vec![0; len];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(invalid)
}

fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = message.to_string();
    output.write_all(&(body.len() as u32).to_le_bytes())?;
    output.write_all(body.as_bytes())?;
    output.flush()
}

fn error(message: impl Display) -> Value {
    json!({"type": "error", "message": message.to_string()})
}

/// A PDF on its way in: the partial file, and where it goes once complete.
struct Transfer {
    part: PathBuf,
    file: File,
    target: PathBuf,
    /// Replacing a local file the extension edited, rather than opening a new one.
    in_place: bool,
    received: u64,
}

impl Transfer {
    fn abandon(self) {
        drop(self.file);
        let _ = std::fs::remove_file(&self.part);
    }
}

/// `name` made safe as a Windows file name ending in .pdf.
fn file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows drops trailing dots and spaces from names.
    let clean = clean.trim().trim_end_matches(['.', ' ']);
    let stem = match clean.len().checked_sub(4) {
        Some(i) if clean.is_char_boundary(i) && clean[i..].eq_ignore_ascii_case(".pdf") => {
            &clean[..i]
        }
        _ => clean,
    };
    let mut stem: String = stem
        .trim_end_matches(['.', ' '])
        .chars()
        .take(100)
        .collect();
    if stem.is_empty() {
        stem = "document".into();
    }
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.to_ascii_uppercase().as_str())
        || (stem.len() == 4
            && stem
                .get(..3)
                .is_some_and(|p| ["COM", "LPT"].contains(&p.to_ascii_uppercase().as_str()))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        stem.insert(0, '_');
    }
    format!("{stem}.pdf")
}

/// `dir/name`, or `dir/stem (2).pdf` and so on when that is taken.
fn unused(dir: &Path, name: &str) -> PathBuf {
    let stem = name.strip_suffix(".pdf").unwrap_or(name);
    (1..)
        .map(|n| match n {
            1 => dir.join(name),
            n => dir.join(format!("{stem} ({n}).pdf")),
        })
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// `path` if it names an existing .pdf file by its full path.
fn local_pdf(path: &str) -> io::Result<PathBuf> {
    let path = PathBuf::from(path);
    let pdf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !path.is_absolute() || !pdf || !path.is_file() {
        return Err(invalid(format!(
            "{} is not a PDF file on this computer",
            path.display()
        )));
    }
    Ok(path)
}

fn begin(message: &Value, inbox: &Path) -> io::Result<Transfer> {
    let (target, in_place) = match message["target"].as_str() {
        Some(target) => (local_pdf(target)?, true),
        None => {
            std::fs::create_dir_all(inbox)?;
            let name = file_name(message["name"].as_str().unwrap_or_default());
            (unused(inbox, &name), false)
        }
    };
    let mut part = target.clone().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    Ok(Transfer {
        file: File::create(&part)?,
        part,
        target,
        in_place,
        received: 0,
    })
}

fn chunk(transfer: &mut Transfer, message: &Value) -> io::Result<()> {
    let data = BASE64_STANDARD
        .decode(message["data"].as_str().unwrap_or_default())
        .map_err(invalid)?;
    transfer.file.write_all(&data)?;
    transfer.received += data.len() as u64;
    Ok(())
}

/// Puts the complete file in place and returns where it is.
fn finish(
    transfer: Transfer,
    open: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let Transfer {
        part,
        mut file,
        target,
        in_place,
        ..
    } = transfer;
    file.flush()?;
    drop(file);
    // Whatever arrives replaces a file or opens in the app, so it has to be a PDF.
    let mut head = [0; 1024];
    let n = File::open(&part)?.read(&mut head)?;
    if !head[..n].windows(5).any(|w| w == b"%PDF-") {
        let _ = std::fs::remove_file(&part);
        return Err(invalid("the data is not a PDF"));
    }
    std::fs::rename(&part, &target)?;
    if !in_place {
        open(&target)?;
    }
    Ok(target)
}

/// Answers the extension until it disconnects. New files land in `inbox` and go to `open`: the
/// app in use, a list in tests.
fn serve(
    input: &mut impl Read,
    output: &mut impl Write,
    inbox: &Path,
    open: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let mut transfer: Option<Transfer> = None;
    while let Some(message) = read_message(input)? {
        let reply = match message["type"].as_str().unwrap_or_default() {
            "hello" => json!({"type": "hello", "version": env!("CARGO_PKG_VERSION")}),
            "open" => match local_pdf(message["path"].as_str().unwrap_or_default())
                .and_then(|path| open(&path).map(|()| path))
            {
                Ok(path) => json!({"type": "done", "path": path}),
                Err(e) => error(e),
            },
            "begin" => {
                if let Some(old) = transfer.take() {
                    old.abandon();
                }
                match begin(&message, inbox) {
                    Ok(t) => {
                        transfer = Some(t);
                        json!({"type": "ready"})
                    }
                    Err(e) => error(e),
                }
            }
            "chunk" => match transfer.as_mut().map(|t| chunk(t, &message)) {
                Some(Ok(())) => {
                    json!({"type": "chunk", "received": transfer.as_ref().map(|t| t.received)})
                }
                Some(Err(e)) => {
                    if let Some(t) = transfer.take() {
                        t.abandon();
                    }
                    error(e)
                }
                None => error("no file is being sent"),
            },
            "end" => match transfer.take() {
                Some(t) => match finish(t, open) {
                    Ok(path) => json!({"type": "done", "path": path}),
                    Err(e) => error(e),
                },
                None => error("no file is being sent"),
            },
            other => error(format!("unknown message type {other:?}")),
        };
        write_message(output, &reply)?;
    }
    if let Some(t) = transfer {
        t.abandon();
    }
    Ok(())
}

/// Where opened web PDFs are kept: %LOCALAPPDATA%\micropdf\Web.
fn inbox() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(r"micropdf\Web")
}

/// The running app's pipe; the same name as in app/src/instance.rs.
fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!(r"\\.\pipe\micropdf-{user}")
}

/// Opens `path` in micropdf: in the running app if there is one, else in a new one.
fn open_in_app(path: &Path) -> io::Result<()> {
    if let Ok(mut pipe) = std::fs::OpenOptions::new().write(true).open(pipe_name()) {
        // SAFETY: plain Win32 call with no pointers.
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        return pipe.write_all(format!("micropdf/1\n{}\n", path.display()).as_bytes());
    }
    use std::os::windows::process::CommandExt;
    let app = std::env::current_exe()?.with_file_name("micropdf.exe");
    let mut command = std::process::Command::new(app);
    command
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // The browser runs the bridge in a job that ends with it; the app has to leave that job to
    // outlive the bridge. A job that forbids leaving refuses the first try.
    match command
        .creation_flags(detached | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
    {
        Ok(_) => Ok(()),
        Err(_) => command.creation_flags(detached).spawn().map(drop),
    }
}

fn manifest(exe: &Path) -> Value {
    json!({
        "name": HOST,
        "description": "micropdf desktop bridge",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Sets the default value of `key` under HKEY_CURRENT_USER, creating the key.
fn set_default(key: &str, value: &str) -> io::Result<()> {
    let key = wide(key);
    let mut handle: HKEY = std::ptr::null_mut();
    // SAFETY: `key` is NUL-terminated; `handle` receives an open key that is closed below.
    let status = unsafe {
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
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let data = wide(value);
    // SAFETY: `handle` is open; `data` holds `data.len()` UTF-16 units; a NULL name is the
    // default value.
    let status = unsafe {
        RegSetValueExW(
            handle,
            std::ptr::null(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    // SAFETY: `handle` was opened above.
    unsafe { RegCloseKey(handle) };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

/// Writes the host manifest beside the exe and points Chrome, Edge and Brave at it.
fn register() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let path = exe.with_file_name(format!("{HOST}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&manifest(&exe))?)?;
    for browser in BROWSERS {
        set_default(&format!(r"{browser}\{HOST}"), &path.to_string_lossy())?;
    }
    Ok(())
}

/// Removes what [`register`] wrote.
fn unregister() -> io::Result<()> {
    for browser in BROWSERS {
        let key = wide(&format!(r"{browser}\{HOST}"));
        // SAFETY: `key` is NUL-terminated. A missing key is not an error worth reporting.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
    }
    let path = std::env::current_exe()?.with_file_name(format!("{HOST}.json"));
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn main() -> ExitCode {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let result = match arg.as_str() {
        "--register" => register(),
        "--unregister" => unregister(),
        // The browser passes the caller's origin first; only the micropdf extension may call.
        origin if origin.starts_with("chrome-extension://") => {
            if origin.trim_end_matches('/') != format!("chrome-extension://{EXTENSION_ID}") {
                return ExitCode::FAILURE;
            }
            serve(
                &mut io::stdin().lock(),
                &mut io::stdout().lock(),
                &inbox(),
                &mut open_in_app,
            )
        }
        _ => {
            eprintln!(
                "micropdf-bridge connects the micropdf browser extension to the app.\n\
                 Usage: micropdf-bridge --register | --unregister"
            );
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("micropdf-bridge: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own in the temp folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("mp-bridge-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn frames(messages: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for m in messages {
            write_message(&mut out, m).unwrap();
        }
        out
    }

    fn replies(mut bytes: &[u8]) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(m) = read_message(&mut bytes).unwrap() {
            out.push(m);
        }
        out
    }

    /// Runs the bridge over `messages`; returns its replies and the files it opened.
    fn run(messages: &[Value], inbox: &Path) -> (Vec<Value>, Vec<PathBuf>) {
        let input = frames(messages);
        let mut output = Vec::new();
        let mut opened = Vec::new();
        serve(&mut input.as_slice(), &mut output, inbox, &mut |p| {
            opened.push(p.to_path_buf());
            Ok(())
        })
        .unwrap();
        (replies(&output), opened)
    }

    fn chunk_of(bytes: &[u8]) -> Value {
        json!({"type": "chunk", "data": BASE64_STANDARD.encode(bytes)})
    }

    #[test]
    fn a_web_pdf_lands_in_the_inbox_and_opens() {
        let dir = Scratch::new("open");
        let send = [
            json!({"type": "hello"}),
            json!({"type": "begin", "name": "https://x.test/a/Q3: report?.PDF"}),
            chunk_of(b"%PDF-1.7\n"),
            chunk_of(b"rest of it"),
            json!({"type": "end"}),
        ];
        let (replies, opened) = run(&send, &dir.0);
        let target = dir.0.join("Q3_ report_.pdf");
        assert_eq!(replies[0]["type"], "hello");
        assert_eq!(replies[1]["type"], "ready");
        assert_eq!(replies[3]["received"], 19);
        assert_eq!(replies[4], json!({"type": "done", "path": target}));
        assert_eq!(opened, std::slice::from_ref(&target));
        assert_eq!(std::fs::read(&target).unwrap(), b"%PDF-1.7\nrest of it");

        // The same name again gets a number.
        let (replies, _) = run(&send, &dir.0);
        assert_eq!(replies[4]["path"], json!(dir.0.join("Q3_ report_ (2).pdf")));
    }

    #[test]
    fn an_edited_local_pdf_replaces_its_file() {
        let dir = Scratch::new("save");
        let target = dir.0.join("form.pdf");
        std::fs::write(&target, b"%PDF-1.4 old").unwrap();
        let (replies, opened) = run(
            &[
                json!({"type": "begin", "target": target}),
                chunk_of(b"%PDF-1.4 new"),
                json!({"type": "end"}),
            ],
            &dir.0,
        );
        assert_eq!(replies[2]["type"], "done", "{replies:?}");
        assert!(opened.is_empty());
        assert_eq!(std::fs::read(&target).unwrap(), b"%PDF-1.4 new");
    }

    #[test]
    fn a_local_pdf_opens_as_it_is() {
        let dir = Scratch::new("local");
        let pdf = dir.0.join("Local.PDF");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        let (replies, opened) = run(
            &[
                json!({"type": "open", "path": pdf}),
                json!({"type": "open", "path": dir.0.join("gone.pdf")}),
            ],
            &dir.0,
        );
        assert_eq!(replies[0], json!({"type": "done", "path": pdf}));
        assert_eq!(replies[1]["type"], "error");
        assert_eq!(opened, [pdf]);
    }

    #[test]
    fn refuses_what_is_not_a_pdf() {
        let dir = Scratch::new("refuse");
        let notes = dir.0.join("notes.txt");
        let target = dir.0.join("keep.pdf");
        std::fs::write(&notes, b"text").unwrap();
        std::fs::write(&target, b"%PDF-1.4 keep").unwrap();
        for bad in [
            json!(notes),
            json!("relative.pdf"),
            json!(dir.0.join("missing.pdf")),
        ] {
            let (replies, _) = run(&[json!({"type": "begin", "target": bad})], &dir.0);
            assert_eq!(replies[0]["type"], "error", "{bad}");
        }
        let (replies, _) = run(
            &[
                json!({"type": "begin", "target": target}),
                chunk_of(b"<html>not a pdf</html>"),
                json!({"type": "end"}),
            ],
            &dir.0,
        );
        assert_eq!(replies[2]["type"], "error");
        assert_eq!(std::fs::read(&target).unwrap(), b"%PDF-1.4 keep");

        let (replies, _) = run(&[chunk_of(b"%PDF-"), json!({"type": "end"})], &dir.0);
        assert!(replies.iter().all(|r| r["type"] == "error"));
        let (replies, _) = run(
            &[
                json!({"type": "begin"}),
                json!({"type": "chunk", "data": "!!"}),
            ],
            &dir.0,
        );
        assert_eq!(replies[1]["type"], "error");
    }

    #[test]
    fn a_dropped_connection_leaves_no_partial_file() {
        let dir = Scratch::new("drop");
        run(
            &[
                json!({"type": "begin", "name": "big.pdf"}),
                chunk_of(b"%PDF-1.7"),
            ],
            &dir.0,
        );
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    }

    #[test]
    fn oversized_messages_end_the_session() {
        let input = ((MAX_MESSAGE + 1) as u32).to_le_bytes();
        let result = serve(
            &mut input.as_slice(),
            &mut Vec::new(),
            Path::new("."),
            &mut |_| Ok(()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn names_are_safe_on_windows() {
        assert_eq!(file_name("a/b\\c.pdf"), "c.pdf");
        assert_eq!(file_name("report"), "report.pdf");
        assert_eq!(file_name("x.PDF. . "), "x.pdf");
        assert_eq!(file_name("con.pdf"), "_con.pdf");
        assert_eq!(file_name("COM1"), "_COM1.pdf");
        assert_eq!(file_name(""), "document.pdf");
        assert_eq!(file_name("tab\there"), "tab_here.pdf");
        assert_eq!(file_name(&"é".repeat(300)).chars().count(), 104);
    }

    #[test]
    fn the_manifest_admits_only_the_extension() {
        let m = manifest(Path::new(r"C:\micropdf\micropdf-bridge.exe"));
        assert_eq!(m["name"], HOST);
        assert_eq!(m["type"], "stdio");
        assert_eq!(m["path"], r"C:\micropdf\micropdf-bridge.exe");
        assert_eq!(
            m["allowed_origins"],
            json!([format!("chrome-extension://{EXTENSION_ID}/")])
        );
    }
}
