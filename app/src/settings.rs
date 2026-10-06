//! Preferences, recent files and the open-tab session, in %APPDATA%\micropdf\settings.json.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::PageMode;
use crate::recolor::ReadingMode;

const RECENT_LIMIT: usize = 12;
const POSITION_LIMIT: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark_theme: bool,
    pub reading_mode: ReadingMode,
    pub vim: bool,
    pub page_mode: PageMode,
    pub sidebar: bool,
    pub recent: Vec<PathBuf>,
    /// Last page per file, so reopening a file returns to where the reader was.
    pub positions: HashMap<PathBuf, usize>,
    pub session: Session,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub files: Vec<PathBuf>,
    pub active: usize,
    /// False while the app runs; true after a normal exit. Still false at startup means the
    /// last run ended unexpectedly.
    pub clean_exit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dark_theme: true,
            reading_mode: ReadingMode::Normal,
            vim: false,
            page_mode: PageMode::Continuous,
            sidebar: false,
            recent: Vec::new(),
            positions: HashMap::new(),
            session: Session {
                clean_exit: true,
                ..Session::default()
            },
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Write then rename, so a crash mid-write never leaves a truncated file.
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(self)
            && std::fs::write(&tmp, json).is_ok()
        {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    pub fn add_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(RECENT_LIMIT);
    }

    pub fn remember_page(&mut self, path: &Path, page: usize) {
        if self.positions.len() >= POSITION_LIMIT && !self.positions.contains_key(path) {
            // Forget files that are no longer in the recent list first.
            let recent = &self.recent;
            self.positions.retain(|p, _| recent.contains(p));
        }
        self.positions.insert(path.to_path_buf(), page);
    }
}

fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("micropdf").join("settings.json"))
}
