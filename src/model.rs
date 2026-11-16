//! The data model: recorded blocks of time, categories, rules, projects and goals.
//!
//! Everything is plain data. Times are UTC seconds since the Unix epoch; the
//! local calendar is only applied when data is shown or grouped (see `timeutil`).

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

pub type BlockId = u64;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// A new block id: unique within this PC's data, roughly ordered by creation time.
pub fn new_id() -> BlockId {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(1);
    // Never go backwards, even if the clock does.
    let mut current = NEXT_ID.load(Ordering::Relaxed);
    loop {
        let next = current.max(seed).wrapping_add(1);
        match NEXT_ID.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => current = seen,
        }
    }
}

/// What a block of time is.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum Kind {
    /// Automatically tracked time in a foreground app.
    #[default]
    #[serde(rename = "a")]
    Active,
    /// Away from the keyboard: idle, locked or asleep. See [`AwayLabel`].
    #[serde(rename = "w")]
    Away,
    /// Time added by hand (offline work).
    #[serde(rename = "m")]
    Manual,
    /// An explicit start/stop timer task. An overlay: it runs alongside the
    /// automatic tracking and is never added to the active total.
    #[serde(rename = "t")]
    Timer,
    /// A focus-mode session. Also an overlay.
    #[serde(rename = "f")]
    Focus,
}

/// What the user said they were doing while away.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum AwayLabel {
    /// Not answered: plain idle time.
    #[default]
    #[serde(rename = "")]
    None,
    #[serde(rename = "meeting")]
    Meeting,
    #[serde(rename = "call")]
    Call,
    #[serde(rename = "break")]
    Break,
}

impl AwayLabel {
    pub fn name(self) -> &'static str {
        match self {
            AwayLabel::None => "Away",
            AwayLabel::Meeting => "Meeting",
            AwayLabel::Call => "Call",
            AwayLabel::Break => "Break",
        }
    }

    /// Meetings and calls are work done away from the keyboard.
    pub fn counts_as_work(self) -> bool {
        matches!(self, AwayLabel::Meeting | AwayLabel::Call)
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// One recorded stretch of time. `[start, end)` in UTC seconds.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Block {
    #[serde(rename = "i")]
    pub id: BlockId,
    #[serde(rename = "s")]
    pub start: i64,
    #[serde(rename = "e")]
    pub end: i64,
    #[serde(rename = "k", default, skip_serializing_if = "is_default")]
    pub kind: Kind,
    #[serde(rename = "w", default, skip_serializing_if = "is_default")]
    pub away: AwayLabel,
    /// Lower-case file name of the program (`code.exe`). Empty for blocks that
    /// are not about a program.
    #[serde(rename = "x", default, skip_serializing_if = "String::is_empty")]
    pub exe: String,
    /// Friendly program name (`Visual Studio Code`).
    #[serde(rename = "a", default, skip_serializing_if = "String::is_empty")]
    pub app: String,
    /// Window title (after privacy rules), task name of a timer, or the
    /// description of a manual entry.
    #[serde(rename = "t", default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Website domain for browser windows (`example.org`), never a full address.
    #[serde(rename = "d", default, skip_serializing_if = "String::is_empty")]
    pub domain: String,
    /// Project id assigned by hand; wins over project rules.
    #[serde(rename = "p", default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Category id assigned by hand; wins over category rules.
    #[serde(rename = "c", default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(rename = "n", default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(rename = "g", default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl Block {
    pub fn secs(&self) -> i64 {
        (self.end - self.start).max(0)
    }

    /// Whether this block is added to worked time: tracked app time, manual
    /// entries, and away time labelled as a meeting or call.
    pub fn is_work(&self) -> bool {
        match self.kind {
            Kind::Active | Kind::Manual => true,
            Kind::Away => self.away.counts_as_work(),
            Kind::Timer | Kind::Focus => false,
        }
    }

    pub fn is_overlay(&self) -> bool {
        matches!(self.kind, Kind::Timer | Kind::Focus)
    }

    /// The name to show for the block's "app" column.
    pub fn app_label(&self) -> &str {
        match self.kind {
            Kind::Away => self.away.name(),
            Kind::Manual => "Manual entry",
            Kind::Timer => "Timer",
            Kind::Focus => "Focus session",
            Kind::Active => {
                if !self.app.is_empty() {
                    &self.app
                } else if !self.exe.is_empty() {
                    &self.exe
                } else {
                    "Unknown"
                }
            }
        }
    }
}

/// How a category counts towards the productivity score.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Score {
    Productive,
    #[default]
    Neutral,
    Distracting,
}

impl Score {
    pub const ALL: [Score; 3] = [Score::Productive, Score::Neutral, Score::Distracting];

    pub fn name(self) -> &'static str {
        match self {
            Score::Productive => "Productive",
            Score::Neutral => "Neutral",
            Score::Distracting => "Distracting",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Category {
    /// Stable id (`development`); rules and overrides refer to it.
    pub id: String,
    pub name: String,
    pub color: [u8; 3],
    #[serde(default)]
    pub score: Score,
    /// Built-in categories can be renamed and recoloured but not removed.
    #[serde(default)]
    pub builtin: bool,
}

/// The id of the catch-all category. It always exists.
pub const OTHER: &str = "other";
/// The category for meetings and calls labelled after being away.
pub const COMMUNICATION: &str = "communication";

/// What a rule looks at.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// Program file name, case-insensitive, `.exe` optional, `*` wildcard allowed.
    Exe,
    /// Case-insensitive text anywhere in the window title.
    TitleContains,
    /// Regular expression on the window title (case-insensitive).
    TitleRegex,
    /// Website domain; `example.org` also matches `www.example.org` and `a.example.org`.
    Domain,
}

impl Field {
    pub const ALL: [Field; 4] = [
        Field::Exe,
        Field::TitleContains,
        Field::TitleRegex,
        Field::Domain,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Field::Exe => "Program",
            Field::TitleContains => "Title contains",
            Field::TitleRegex => "Title matches (regex)",
            Field::Domain => "Website",
        }
    }
}

/// "Time matching this goes into that category."
///
/// Precedence (see `rules::Classifier`): a category set by hand on a block,
/// then website rules, then title-regex rules, then title-contains rules, then
/// program rules; inside each group the rule higher in the list wins.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Rule {
    pub field: Field,
    pub pattern: String,
    /// Category id.
    pub category: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub client: String,
    pub color: [u8; 3],
    /// Hourly rate for timesheets; 0 = not billed.
    #[serde(default)]
    pub rate: f64,
    /// Title keywords or folder names. Time whose window title (or website)
    /// contains one of them, ignoring case, belongs to the project. When
    /// several projects match, the longest keyword wins.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub archived: bool,
}

/// What a goal or limit measures.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum GoalTarget {
    /// All worked time.
    Total,
    Category(String),
    Project(String),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Goal {
    pub target: GoalTarget,
    /// Minutes per day.
    pub minutes: u32,
    /// `true`: a limit ("at most"); `false`: a target ("at least").
    #[serde(default)]
    pub limit: bool,
    /// Show a notification when the limit or target is reached.
    #[serde(default = "yes")]
    pub alert: bool,
}

/// How a private program is treated.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PrivateMode {
    /// Never recorded: the time leaves a gap.
    Skip,
    /// The time is recorded, the window title and website are not.
    NoTitle,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct PrivateApp {
    pub exe: String,
    pub mode: PrivateMode,
}

/// A regular expression whose matches are replaced in window titles before
/// they are stored.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Redaction {
    pub pattern: String,
    #[serde(default = "default_mask")]
    pub replace: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

pub fn default_mask() -> String {
    "•••".to_string()
}

/// Where a browser's website domain comes from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DomainSource {
    /// Websites are not recorded for this browser.
    Off,
    /// Guessed from the window title only.
    Title,
    /// Read from the address bar (UI Automation), falling back to the title.
    AddressBar,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Browser {
    pub exe: String,
    pub name: String,
    pub source: DomainSource,
}

/// How timesheet durations are rounded.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    #[default]
    None,
    Up,
    Nearest,
    Down,
}

impl Rounding {
    pub const ALL: [Rounding; 4] = [
        Rounding::None,
        Rounding::Nearest,
        Rounding::Up,
        Rounding::Down,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Rounding::None => "No rounding",
            Rounding::Up => "Round up",
            Rounding::Nearest => "Round to nearest",
            Rounding::Down => "Round down",
        }
    }
}

/// What the tracker is doing right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TrackState {
    /// The tracker is not running (demo mode, or it was stopped).
    #[default]
    Off,
    Active,
    Idle,
    Locked,
    Asleep,
    Paused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_increasing() {
        let a = new_id();
        let b = new_id();
        let c = new_id();
        assert!(a < b && b < c);
    }

    #[test]
    fn block_round_trips_compactly() {
        let b = Block {
            id: 7,
            start: 100,
            end: 160,
            exe: "editor.exe".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, r#"{"i":7,"s":100,"e":160,"x":"editor.exe"}"#);
        assert_eq!(serde_json::from_str::<Block>(&json).unwrap(), b);
        let away = Block {
            kind: Kind::Away,
            away: AwayLabel::Meeting,
            ..b.clone()
        };
        let back: Block = serde_json::from_str(&serde_json::to_string(&away).unwrap()).unwrap();
        assert_eq!(back, away);
        assert!(back.is_work());
        assert!(!Block {
            kind: Kind::Timer,
            ..b
        }
        .is_work());
    }
}
