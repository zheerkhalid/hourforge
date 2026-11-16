//! The tracker's shared state and the commands the window sends to it.
//!
//! The tracker itself (hooks, sampling, notification-area icon, hotkey) runs
//! on its own thread, see `tracker`. The window and the tracker meet here: a
//! [`Shared`] holds the store and a [`Status`] snapshot; the window sends
//! [`Cmd`]s.

use crate::model::{AwayLabel, BlockId, TrackState};
use crate::settings::Settings;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunningTimer {
    pub id: BlockId,
    pub start: i64,
    pub task: String,
    pub project: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunningFocus {
    pub id: BlockId,
    pub start: i64,
    /// When the session ends.
    pub until: i64,
}

/// An absence the user has not been asked about yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Returned {
    pub id: BlockId,
    pub start: i64,
    pub end: i64,
}

/// A snapshot of what the tracker is doing, for the window.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub state: TrackState,
    /// Tracking resumes by itself at this time; `i64::MAX` = until resumed by hand.
    pub paused_until: Option<i64>,
    /// The program in front (friendly name and file name); empty when none.
    pub app: String,
    pub exe: String,
    pub timer: Option<RunningTimer>,
    pub focus: Option<RunningFocus>,
    /// Absences waiting for "what were you doing?", oldest first.
    pub returned: Vec<Returned>,
}

/// What the tracker knows about a program it has seen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    /// Friendly name from the program's version information.
    pub name: String,
    /// Full path, for the icon.
    pub path: String,
}

pub enum Cmd {
    /// Stop recording until this time (`i64::MAX` = until resumed).
    Pause {
        until: i64,
    },
    Resume,
    StartTimer {
        task: String,
        project: Option<String>,
    },
    StopTimer,
    StartFocus {
        minutes: u32,
    },
    StopFocus,
    /// The answer to "what were you doing while away?". `None` discards the
    /// absence (the away block is deleted); `Some(AwayLabel::None)` keeps it
    /// as plain away time.
    LabelAway {
        id: BlockId,
        label: Option<AwayLabel>,
    },
    /// The settings were changed in the window.
    Settings(Box<Settings>),
    /// Write everything to disk now.
    Flush,
}

/// State shared by the tracker thread and the window.
pub struct Shared {
    pub dir: PathBuf,
    pub store: Mutex<Store>,
    pub status: Mutex<Status>,
    /// Programs seen so far, by lower-case file name.
    pub apps: Mutex<HashMap<String, AppInfo>>,
    /// Set when the user chose Exit (as opposed to closing the window).
    pub quit: AtomicBool,
    /// Set by the tracker to ask the window to show a page (`away`, `today`).
    pub show: Mutex<Option<String>>,
    /// Wakes the window when something changed; set while a window exists.
    pub waker: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Shared {
    pub fn new(dir: PathBuf, store: Store) -> Shared {
        Shared {
            dir,
            store: Mutex::new(store),
            status: Mutex::new(Status::default()),
            apps: Mutex::new(HashMap::new()),
            quit: AtomicBool::new(false),
            show: Mutex::new(None),
            waker: Mutex::new(None),
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Asks the window (if there is one) to repaint.
    pub fn wake(&self) {
        if let Ok(guard) = self.waker.lock() {
            if let Some(wake) = guard.as_ref() {
                wake();
            }
        }
    }
}

/// Something that accepts commands: the tracker thread, or the stand-in used
/// in demo mode.
pub trait Control: Send + Sync {
    fn send(&self, cmd: Cmd);
}

fn apps_path(dir: &std::path::Path) -> PathBuf {
    dir.join("apps.json")
}

/// Loads the table of programs seen so far (names and paths for icons).
pub fn load_apps(dir: &std::path::Path) -> HashMap<String, AppInfo> {
    std::fs::read(apps_path(dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the table of programs next to the data.
pub fn save_apps(shared: &Shared) {
    let Ok(apps) = shared.apps.lock() else { return };
    if let Ok(json) = serde_json::to_vec(&*apps) {
        let tmp = shared.dir.join("apps.json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, apps_path(&shared.dir));
        }
    }
}
