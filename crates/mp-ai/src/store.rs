//! What the assistant keeps between runs: API keys in Windows Credential Manager; the provider,
//! the model for each and today's usage in %APPDATA%\micropdf\ai.json; one chat per document in
//! %APPDATA%\micropdf\chats, named by the document's content hash so it survives renames. The app
//! and micropdf-bridge both read and write these.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use crate::{Provider, Turn, Usage};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub provider: Provider,
    pub models: HashMap<Provider, String>,
    pub usage: DailyUsage,
}

/// Tokens used today (local time), as the providers counted them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyUsage {
    pub day: String,
    pub input: u64,
    pub output: u64,
}

fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("micropdf"))
}

/// Write then rename, so a crash mid-write never leaves a truncated file.
fn write(path: &Path, json: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn today() -> String {
    let mut t = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

impl Config {
    pub fn load() -> Config {
        dir()
            .and_then(|d| std::fs::read_to_string(d.join("ai.json")).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let (Some(dir), Ok(json)) = (dir(), serde_json::to_string_pretty(self)) {
            write(&dir.join("ai.json"), &json);
        }
    }

    pub fn model(&self, provider: Provider) -> &str {
        self.models.get(&provider).map_or("", String::as_str)
    }

    /// Today's usage, which starts again at local midnight.
    pub fn today(&self) -> DailyUsage {
        self.usage_on(&today())
    }

    fn usage_on(&self, day: &str) -> DailyUsage {
        if self.usage.day == day {
            self.usage.clone()
        } else {
            DailyUsage {
                day: day.into(),
                input: 0,
                output: 0,
            }
        }
    }

    /// Adds one answer's usage to today's total, stored at once so the app and the extension
    /// count together, and returns the total.
    pub fn record(usage: Usage) -> DailyUsage {
        let mut config = Config::load();
        config.add(usage, &today());
        config.save();
        config.usage
    }

    fn add(&mut self, usage: Usage, day: &str) {
        let mut total = self.usage_on(day);
        total.input += usage.input;
        total.output += usage.output;
        self.usage = total;
    }
}

fn target(provider: Provider) -> Vec<u16> {
    format!("micropdf/{}", provider.id())
        .encode_utf16()
        .chain([0])
        .collect()
}

/// The provider's API key, from Credential Manager.
pub fn key(provider: Provider) -> Option<String> {
    let target = target(provider);
    let mut cred: *mut CREDENTIALW = null_mut();
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 {
            return None;
        }
        let blob =
            std::slice::from_raw_parts((*cred).CredentialBlob, (*cred).CredentialBlobSize as usize);
        let key = String::from_utf8(blob.to_vec()).ok();
        CredFree(cred.cast());
        key.filter(|k| !k.is_empty())
    }
}

/// Stores the key in Credential Manager, or removes it when `key` is empty.
pub fn set_key(provider: Provider, key: &str) -> Result<(), String> {
    let mut target = target(provider);
    let key = key.trim();
    let ok = unsafe {
        if key.is_empty() {
            CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) != 0
        } else {
            let cred = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: target.as_mut_ptr(),
                CredentialBlobSize: key.len() as u32,
                CredentialBlob: key.as_ptr().cast_mut(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                ..CREDENTIALW::default()
            };
            CredWriteW(&cred, 0) != 0
        }
    };
    if ok || key.is_empty() {
        Ok(())
    } else {
        Err("Windows did not store the key.".into())
    }
}

/// FNV-1a over the file's bytes: the same document gets the same chat under any name.
pub fn content_hash(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    format!("{hash:016x}")
}

fn chat_file(hash: &str) -> Option<PathBuf> {
    let safe = !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit());
    dir()
        .filter(|_| safe)
        .map(|d| d.join("chats").join(format!("{hash}.json")))
}

pub fn load_chat(hash: &str) -> Vec<Turn> {
    chat_file(hash)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Keeps the chat; an empty one (a new chat) removes the file.
pub fn save_chat(hash: &str, turns: &[Turn]) {
    let Some(file) = chat_file(hash) else { return };
    if turns.is_empty() {
        let _ = std::fs::remove_file(file);
    } else if let Ok(json) = serde_json::to_string(turns) {
        write(&file, &json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_adds_up_and_starts_again_each_day() {
        let mut config = Config::default();
        config.add(
            Usage {
                input: 10,
                output: 2,
            },
            "2026-10-09",
        );
        config.add(
            Usage {
                input: 5,
                output: 1,
            },
            "2026-10-09",
        );
        assert_eq!(
            config.usage,
            DailyUsage {
                day: "2026-10-09".into(),
                input: 15,
                output: 3
            }
        );
        assert_eq!(config.usage_on("2026-10-10").input, 0);
        config.add(
            Usage {
                input: 1,
                output: 1,
            },
            "2026-10-10",
        );
        assert_eq!(config.usage.input, 1);
    }

    #[test]
    fn the_config_file_reads_back() {
        let mut config = Config {
            provider: Provider::Google,
            ..Config::default()
        };
        config
            .models
            .insert(Provider::OpenAi, "gpt-5.6-luna".into());
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            json.contains(r#""provider":"google""#) && json.contains(r#""openai":"gpt-5.6-luna""#)
        );
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, config);
        assert_eq!(back.model(Provider::Anthropic), "");
        // An older or hand-edited file still loads.
        assert_eq!(
            serde_json::from_str::<Config>("{}").unwrap(),
            Config::default()
        );
    }

    #[test]
    fn content_hashes_are_stable_and_safe_as_names() {
        assert_eq!(content_hash(b""), "cbf29ce484222325");
        assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
        assert!(chat_file("../evil").is_none());
    }
}
