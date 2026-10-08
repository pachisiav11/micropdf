//! Runs the built micropdf-bridge.exe as a browser would.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

const EXE: &str = env!("CARGO_BIN_EXE_micropdf-bridge");

fn frame(json: &str) -> Vec<u8> {
    let mut out = (json.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(json.as_bytes());
    out
}

#[test]
fn answers_the_extension() {
    let mut bridge = Command::new(EXE)
        .arg("chrome-extension://phhaejfhblmccnkhnhjflbhckkanlnki/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    bridge
        .stdin
        .take()
        .unwrap()
        .write_all(&frame(r#"{"type":"hello"}"#))
        .unwrap();
    let mut out = Vec::new();
    bridge.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    assert!(bridge.wait().unwrap().success());
    let len = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
    let reply = std::str::from_utf8(&out[4..4 + len]).unwrap();
    assert!(reply.contains(r#""type":"hello""#), "{reply}");
}

#[test]
fn turns_away_other_extensions() {
    let status = Command::new(EXE)
        .arg("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}
