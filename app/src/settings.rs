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
    /// The reader's signature and initials, kept for the Sign tool.
    pub signature: Option<SavedMark>,
    pub initials: Option<SavedMark>,
    /// The colour each comment tool draws with, by tool ("note", "rect", "highlight", ...),
    /// when the reader picked one.
    pub comment_colors: HashMap<String, [f32; 3]>,
    /// The library's folders, searched with their subfolders.
    pub library: Vec<PathBuf>,
    /// Look for a newer release at start; otherwise only when asked.
    pub check_updates: bool,
}

/// A signature or initials as kept in the settings file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum SavedMark {
    Ink {
        strokes: Vec<Vec<(f32, f32)>>,
        width: f32,
    },
    Typed {
        text: String,
        font: String,
    },
    /// A copy of the chosen image, kept beside the settings file.
    Image {
        file: PathBuf,
    },
}

impl SavedMark {
    pub fn to_mark(&self) -> std::io::Result<mp_engine::Mark> {
        Ok(match self {
            SavedMark::Ink { strokes, width } => mp_engine::Mark::Ink {
                strokes: strokes.clone(),
                width: *width,
            },
            SavedMark::Typed { text, font } => mp_engine::Mark::Typed {
                text: text.clone(),
                font: font.clone(),
            },
            SavedMark::Image { file } => mp_engine::Mark::Image(std::fs::read(file)?),
        })
    }
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
            signature: None,
            initials: None,
            comment_colors: HashMap::new(),
            library: Vec::new(),
            check_updates: false,
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

/// %APPDATA%\micropdf, which holds the settings file and saved signature images.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("micropdf"))
}

fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("settings.json"))
}
