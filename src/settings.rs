//! Settings, including the categories, rules, projects, goals and privacy
//! lists. One JSON file in the data folder, written atomically.

use crate::model::{
    Browser, Category, DomainSource, Field, Goal, PrivateApp, Project, Redaction, Rounding, Rule,
    Score, OTHER,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

/// What happens when the user comes back after being away.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReturnPrompt {
    /// Open the "what were you doing?" question.
    #[default]
    Ask,
    /// Only a notification near the clock; answer later in the timeline.
    Notify,
    /// Leave away time unlabelled.
    Never,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Privacy {
    /// The global switch: when off, no window title is ever stored.
    pub record_titles: bool,
    /// Record website domains for browsers at all.
    pub record_domains: bool,
    /// Drop title and website of private / incognito browser windows.
    pub hide_private_windows: bool,
    pub private_apps: Vec<PrivateApp>,
    pub redactions: Vec<Redaction>,
}

impl Default for Privacy {
    fn default() -> Self {
        Privacy {
            record_titles: true,
            record_domains: true,
            hide_private_windows: true,
            private_apps: Vec::new(),
            redactions: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Timesheet {
    pub rounding: Rounding,
    /// Rounding step in minutes (6, 10, 15, 30, 60).
    pub step_minutes: u32,
    /// Shown next to amounts; purely a label.
    pub currency: String,
    /// Your name or business, printed on invoices under "From".
    pub business: String,
    /// A line at the foot of invoices: payment terms, bank details.
    pub invoice_note: String,
}

impl Default for Timesheet {
    fn default() -> Self {
        Timesheet {
            rounding: Rounding::Nearest,
            step_minutes: 15,
            currency: "USD".into(),
            business: String::new(),
            invoice_note: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub week_starts_monday: bool,

    // Tracking
    /// Minutes without keyboard or mouse input before time counts as away.
    pub idle_minutes: u32,
    pub return_prompt: ReturnPrompt,
    /// Only ask about absences at least this long.
    pub ask_min_minutes: u32,
    /// Fullscreen video and apps that keep the screen on count as active
    /// even without input.
    pub media_counts_active: bool,
    /// Keep tracking in the notification area when the window is closed.
    pub close_to_tray: bool,
    /// The small always-on-top "now tracking" pill.
    pub indicator: bool,
    /// Global hotkey that starts / stops the timer, e.g. `Ctrl+Alt+T`. Empty = none.
    pub hotkey: String,
    pub last_task: String,
    pub last_task_project: Option<String>,

    // Storage
    /// Delete data older than this many days; 0 keeps everything.
    pub retention_days: u32,

    // Goals, breaks, focus
    pub goals: Vec<Goal>,
    /// Remind to take a break after this many minutes of unbroken activity; 0 = off.
    pub break_reminder_minutes: u32,
    pub focus_minutes: u32,
    pub focus_break_minutes: u32,
    /// A notification at the start of each week with last week's totals.
    pub weekly_summary: bool,

    pub categories: Vec<Category>,
    pub rules: Vec<Rule>,
    pub projects: Vec<Project>,
    pub browsers: Vec<Browser>,
    pub privacy: Privacy,
    pub timesheet: Timesheet,
}

/// The palette of the built-in categories; also offered when making new ones.
pub const PALETTE: [[u8; 3]; 12] = [
    [0x4f, 0x8c, 0xff],
    [0x22, 0xc3, 0xa6],
    [0xf5, 0xb0, 0x41],
    [0x4c, 0xc9, 0xf0],
    [0xec, 0x6f, 0xb1],
    [0xf0, 0x68, 0x5c],
    [0xa9, 0x71, 0xf5],
    [0x7f, 0xbf, 0x5f],
    [0x77, 0x82, 0x8f],
    [0xff, 0x92, 0x48],
    [0xc9, 0xb4, 0x58],
    [0x5f, 0xd0, 0x8a],
];

const BUILTIN: [(&str, &str, usize, Score); 9] = [
    ("development", "Development", 0, Score::Productive),
    ("writing", "Writing", 1, Score::Productive),
    ("communication", "Communication", 2, Score::Neutral),
    ("browsing", "Browsing", 3, Score::Neutral),
    ("design", "Design", 4, Score::Productive),
    ("media", "Media", 5, Score::Distracting),
    ("games", "Games", 6, Score::Distracting),
    ("utilities", "Utilities", 7, Score::Neutral),
    (OTHER, "Other", 8, Score::Neutral),
];

/// Starter rules: `|`-separated patterns per category.
const DEFAULT_RULES: [(Field, &str, &str); 14] = [
    (Field::Domain, "development", "github.com|gitlab.com|stackoverflow.com|docs.rs|crates.io|developer.mozilla.org|learn.microsoft.com|npmjs.com|bitbucket.org"),
    (Field::Domain, "writing", "docs.google.com|notion.so|overleaf.com|medium.com"),
    (Field::Domain, "communication", "mail.google.com|outlook.live.com|outlook.office.com|web.whatsapp.com|web.telegram.org|slack.com|teams.microsoft.com|meet.google.com|zoom.us"),
    (Field::Domain, "design", "figma.com|canva.com|dribbble.com"),
    (Field::Domain, "media", "youtube.com|netflix.com|twitch.tv|spotify.com|primevideo.com|disneyplus.com|facebook.com|instagram.com|x.com|twitter.com|reddit.com|tiktok.com"),
    (Field::Exe, "development", "code.exe|devenv.exe|idea64.exe|pycharm64.exe|webstorm64.exe|rider64.exe|clion64.exe|studio64.exe|datagrip64.exe|sublime_text.exe|notepad++.exe|windowsterminal.exe|cmd.exe|powershell.exe|pwsh.exe|mintty.exe|conhost.exe|wsl.exe|githubdesktop.exe|docker desktop.exe|postman.exe|ssms.exe|zed.exe|nvim.exe|gvim.exe"),
    (Field::Exe, "writing", "winword.exe|excel.exe|powerpnt.exe|notepad.exe|wordpad.exe|onenote.exe|obsidian.exe|typora.exe|soffice.bin|acrobat.exe|acrord32.exe|notion.exe"),
    (Field::Exe, "communication", "outlook.exe|olk.exe|thunderbird.exe|teams.exe|ms-teams.exe|slack.exe|discord.exe|zoom.exe|telegram.exe|whatsapp.exe|signal.exe|skype.exe|viber.exe"),
    (Field::Exe, "browsing", "chrome.exe|msedge.exe|firefox.exe|brave.exe|opera.exe|vivaldi.exe"),
    (Field::Exe, "design", "photoshop.exe|illustrator.exe|indesign.exe|afterfx.exe|adobe premiere pro.exe|figma.exe|blender.exe|gimp*.exe|inkscape.exe|krita.exe|mspaint.exe|resolve.exe"),
    (Field::Exe, "media", "vlc.exe|spotify.exe|wmplayer.exe|mpc-hc64.exe|mpv.exe|itunes.exe|music.ui.exe|video.ui.exe|microsoft.media.player.exe"),
    (Field::Exe, "games", "steam.exe|steamwebhelper.exe|epicgameslauncher.exe|battle.net.exe|riotclientux.exe|leagueclient.exe|minecraft*.exe|gog galaxy.exe"),
    (Field::Exe, "utilities", "explorer.exe|taskmgr.exe|7zfm.exe|winrar.exe|systemsettings.exe|calculatorapp.exe|snippingtool.exe|control.exe|mmc.exe|regedit.exe|searchhost.exe"),
    (Field::Exe, "utilities", "hourforge.exe"),
];

pub fn default_categories() -> Vec<Category> {
    BUILTIN
        .iter()
        .map(|(id, name, color, score)| Category {
            id: (*id).into(),
            name: (*name).into(),
            color: PALETTE[*color],
            score: *score,
            builtin: true,
        })
        .collect()
}

pub fn default_rules() -> Vec<Rule> {
    let mut rules = Vec::new();
    for (field, category, patterns) in DEFAULT_RULES {
        for pattern in patterns.split('|') {
            rules.push(Rule {
                field,
                pattern: pattern.into(),
                category: category.into(),
                enabled: true,
            });
        }
    }
    rules
}

pub fn default_browsers() -> Vec<Browser> {
    [
        ("chrome.exe", "Chrome"),
        ("msedge.exe", "Edge"),
        ("firefox.exe", "Firefox"),
        ("brave.exe", "Brave"),
        ("opera.exe", "Opera"),
        ("vivaldi.exe", "Vivaldi"),
    ]
    .iter()
    .map(|(exe, name)| Browser {
        exe: (*exe).into(),
        name: (*name).into(),
        source: DomainSource::AddressBar,
    })
    .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Dark,
            week_starts_monday: true,
            idle_minutes: 5,
            return_prompt: ReturnPrompt::Ask,
            ask_min_minutes: 10,
            media_counts_active: true,
            close_to_tray: true,
            indicator: false,
            hotkey: "Ctrl+Alt+T".into(),
            last_task: String::new(),
            last_task_project: None,
            retention_days: 0,
            goals: Vec::new(),
            break_reminder_minutes: 0,
            focus_minutes: 25,
            focus_break_minutes: 5,
            weekly_summary: true,
            categories: default_categories(),
            rules: default_rules(),
            projects: Vec::new(),
            browsers: default_browsers(),
            privacy: Privacy::default(),
            timesheet: Timesheet::default(),
        }
    }
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// Loads the settings; a missing or damaged file gives the defaults.
    ///
    /// A file that is there but cannot be read as settings is first copied to
    /// `settings.damaged.json`: the next save would otherwise replace the
    /// user's categories, rules and projects with the defaults for good.
    pub fn load(dir: &Path) -> Settings {
        let mut s: Settings = match std::fs::read(Self::path(dir)) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(settings) => settings,
                Err(_) => {
                    let _ = std::fs::write(Self::damaged_path(dir), &bytes);
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        };
        s.repair();
        s
    }

    /// Where a settings file that could not be read is kept.
    pub fn damaged_path(dir: &Path) -> PathBuf {
        dir.join("settings.damaged.json")
    }

    /// Writes the settings (temporary file, then rename, so a crash never
    /// leaves half a file).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, Self::path(dir))
    }

    /// Makes the settings consistent: sane numbers, every built-in category
    /// present, rules and goals pointing at categories that exist.
    pub fn repair(&mut self) {
        self.idle_minutes = self.idle_minutes.clamp(1, 120);
        self.ask_min_minutes = self.ask_min_minutes.clamp(1, 240);
        self.focus_minutes = self.focus_minutes.clamp(5, 180);
        self.focus_break_minutes = self.focus_break_minutes.clamp(1, 60);
        self.timesheet.step_minutes = self.timesheet.step_minutes.clamp(1, 120);
        for builtin in default_categories() {
            if !self.categories.iter().any(|c| c.id == builtin.id) {
                self.categories.push(builtin);
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.categories
            .retain(|c| !c.id.is_empty() && seen.insert(c.id.clone()));
        let known: std::collections::HashSet<&str> =
            self.categories.iter().map(|c| c.id.as_str()).collect();
        self.rules
            .retain(|r| known.contains(r.category.as_str()) && !r.pattern.trim().is_empty());
        let mut seen = std::collections::HashSet::new();
        self.projects
            .retain(|p| !p.id.is_empty() && seen.insert(p.id.clone()));
    }

    pub fn category(&self, id: &str) -> Option<&Category> {
        self.categories.iter().find(|c| c.id == id)
    }

    pub fn category_name(&self, id: &str) -> &str {
        self.category(id)
            .map(|c| c.name.as_str())
            .unwrap_or("Other")
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn project_name(&self, id: &str) -> &str {
        self.project(id).map(|p| p.name.as_str()).unwrap_or("")
    }

    /// A new id derived from a name, unique among `taken`.
    pub fn make_id(name: &str, taken: &[&str]) -> String {
        let mut base: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        while base.contains("--") {
            base = base.replace("--", "-");
        }
        let base = base.trim_matches('-').to_string();
        let base = if base.is_empty() { "item".into() } else { base };
        let mut id = base.clone();
        let mut n = 2;
        while taken.contains(&id.as_str()) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

/// `%LOCALAPPDATA%\HourForge`, or the folder named by `HOURFORGE_DATA`.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HOURFORGE_DATA") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("HourForge")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_consistent() {
        let mut s = Settings::default();
        let before = s.clone();
        s.repair();
        assert_eq!(s, before);
        assert_eq!(s.categories.len(), 9);
        assert!(s.category(OTHER).is_some());
        assert!(s.rules.len() > 80);
    }

    #[test]
    fn repair_restores_builtins_and_drops_dangling_rules() {
        let mut s = Settings::default();
        s.categories.retain(|c| c.id != "games");
        s.rules.push(Rule {
            field: Field::Exe,
            pattern: "x.exe".into(),
            category: "nowhere".into(),
            enabled: true,
        });
        s.idle_minutes = 0;
        s.repair();
        assert!(s.category("games").is_some());
        assert!(s.rules.iter().all(|r| r.category != "nowhere"));
        assert_eq!(s.idle_minutes, 1);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("hourforge-settings-{}", crate::model::new_id()));
        let mut s = Settings {
            idle_minutes: 9,
            ..Default::default()
        };
        s.projects.push(Project {
            id: "apollo".into(),
            name: "Apollo".into(),
            keywords: vec!["apollo".into()],
            ..Default::default()
        });
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        // Damaged file: defaults, no panic, and the damaged file is kept.
        std::fs::write(Settings::path(&dir), b"{ not json").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        assert_eq!(
            std::fs::read(Settings::damaged_path(&dir)).unwrap(),
            b"{ not json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A settings file as version 1.0.0 wrote it has none of the newer
    /// fields. It must load as it was, with the new options at their defaults.
    #[test]
    fn settings_of_1_0_0_load_unchanged() {
        let old = r#"{
            "theme": "light",
            "week_starts_monday": false,
            "idle_minutes": 9,
            "return_prompt": "notify",
            "hotkey": "Ctrl+Alt+H",
            "goals": [{"target": {"project": "atlas"}, "minutes": 90, "limit": false, "alert": true}],
            "projects": [{
                "id": "atlas", "name": "Atlas", "client": "Northwind",
                "color": [79, 140, 255], "rate": 50.0,
                "keywords": ["atlas"], "archived": false
            }],
            "privacy": {"record_titles": false, "record_domains": true,
                        "hide_private_windows": true, "private_apps": [], "redactions": []},
            "timesheet": {"rounding": "up", "step_minutes": 6, "currency": "EUR"}
        }"#;
        let mut s: Settings = serde_json::from_str(old).unwrap();
        s.repair();
        assert_eq!(s.theme, ThemeChoice::Light);
        assert!(!s.week_starts_monday);
        assert_eq!(s.idle_minutes, 9);
        assert_eq!(s.return_prompt, ReturnPrompt::Notify);
        assert_eq!(s.hotkey, "Ctrl+Alt+H");
        assert_eq!(s.goals.len(), 1);
        assert!(!s.privacy.record_titles);
        assert_eq!(s.timesheet.rounding, Rounding::Up);
        assert_eq!(s.timesheet.step_minutes, 6);
        assert_eq!(s.timesheet.currency, "EUR");
        // What 1.1 added.
        assert!(s.weekly_summary);
        assert_eq!(s.timesheet.business, "");
        assert_eq!(s.timesheet.invoice_note, "");
        let atlas = s.project("atlas").unwrap();
        assert!(!atlas.non_billable && atlas.charged());
        assert_eq!(atlas.rate, 50.0);
        // A billable project is written exactly as before, so a settings
        // file can still be read by 1.0.0 (which ignores what it does not know).
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("non_billable"));
        let mut marked = s.clone();
        marked.projects[0].non_billable = true;
        let json = serde_json::to_string(&marked).unwrap();
        assert!(json.contains("\"non_billable\":true"));
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), marked);
        assert!(!marked.projects[0].charged());
    }

    #[test]
    fn ids_from_names() {
        assert_eq!(Settings::make_id("Client Work!", &[]), "client-work");
        assert_eq!(Settings::make_id("Media", &["media"]), "media-2");
        assert_eq!(Settings::make_id("***", &[]), "item");
    }
}
