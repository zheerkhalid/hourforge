//! Invented data for `--demo` and the screenshots. Nothing here comes from a
//! real PC: the programs, window titles, sites, projects and clients are all
//! made up, and the sites live under the reserved `.example` domain.
//!
//! The data is deterministic (a small seeded generator), so screenshots do
//! not change from run to run.

use crate::model::{
    AwayLabel, Block, Browser, DomainSource, Field, Goal, GoalTarget, Kind, PrivateApp,
    PrivateMode, Project, Redaction, Rule,
};
use crate::settings::Settings;
use crate::store::Store;
use crate::timeutil::{days_from_civil, weekday, Day, Tz};

/// The time zone of the invented PC.
pub fn tz() -> Tz {
    Tz::Fixed(3600)
}

/// "Now" in the demo: a fixed moment on an invented working day, so that
/// screenshots do not change from run to run.
pub fn now() -> i64 {
    // Thursday 12 November 2026, 16:42 local time.
    days_from_civil(2026, 11, 12) as i64 * 86_400 + 16 * 3600 + 42 * 60 - 3600
}

/// How many days of history [`populate`] invents (today included).
pub const DAYS: i32 = 90;

// Invented programs: (file name, friendly name, category id).
const PROGRAMS: [(&str, &str, &str); 14] = [
    ("quillcode.exe", "Quill Code", "development"),
    ("tern.exe", "Tern Terminal", "development"),
    ("lumen.exe", "Lumen Browser", "browsing"),
    ("postbird.exe", "Postbird Mail", "communication"),
    ("huddle.exe", "Huddle", "communication"),
    ("meetly.exe", "Meetly", "communication"),
    ("vectra.exe", "Vectra Studio", "design"),
    ("pagesmith.exe", "PageSmith", "writing"),
    ("gridworks.exe", "GridWorks", "writing"),
    ("notewell.exe", "Notewell", "writing"),
    ("tunebox.exe", "Tunebox", "media"),
    ("starfall.exe", "Starfall Tactics", "games"),
    ("folderly.exe", "Folderly", "utilities"),
    ("ledgerly.exe", "Ledgerly", "utilities"),
];

// Invented sites: (domain, category id).
const SITES: [(&str, &str); 10] = [
    ("codehost.example", "development"),
    ("stackhelp.example", "development"),
    ("devdocs.example", "development"),
    ("mailbox.example", "communication"),
    ("designboard.example", "design"),
    ("videotube.example", "media"),
    ("chirper.example", "media"),
    ("dailyledger.example", "browsing"),
    ("wikipage.example", "browsing"),
    ("shopmart.example", "browsing"),
];

fn friendly(exe: &str) -> &'static str {
    PROGRAMS
        .iter()
        .find(|(e, _, _)| *e == exe)
        .map(|(_, n, _)| *n)
        .unwrap_or("Unknown")
}

/// Default settings plus invented projects, goals and privacy examples.
pub fn settings() -> Settings {
    let mut s = Settings::default();
    let mut rules = Vec::new();
    for (domain, category) in SITES {
        rules.push(Rule {
            field: Field::Domain,
            pattern: domain.into(),
            category: category.into(),
            enabled: true,
        });
    }
    rules.push(Rule {
        field: Field::TitleContains,
        pattern: "release notes".into(),
        category: "writing".into(),
        enabled: true,
    });
    rules.push(Rule {
        field: Field::TitleRegex,
        pattern: r"^Pull request #\d+".into(),
        category: "development".into(),
        enabled: true,
    });
    for (exe, _, category) in PROGRAMS {
        rules.push(Rule {
            field: Field::Exe,
            pattern: exe.into(),
            category: category.into(),
            enabled: true,
        });
    }
    // The invented rules come first; the starter rules stay below them.
    rules.append(&mut s.rules);
    s.rules = rules;

    s.projects = vec![
        Project {
            id: "atlas".into(),
            name: "Atlas Web Shop".into(),
            client: "Harbor & Pine".into(),
            color: [0x4f, 0x8c, 0xff],
            rate: 85.0,
            keywords: vec!["atlas-shop".into(), "atlas".into()],
            archived: false,
            non_billable: false,
        },
        Project {
            id: "orbit".into(),
            name: "Orbit Mobile App".into(),
            client: "Bluefield Labs".into(),
            color: [0x22, 0xc3, 0xa6],
            rate: 95.0,
            keywords: vec!["orbit-app".into(), "orbit".into()],
            archived: false,
            non_billable: false,
        },
        Project {
            id: "meridian".into(),
            name: "Meridian Rebrand".into(),
            client: "Meridian Coffee".into(),
            color: [0xec, 0x6f, 0xb1],
            rate: 70.0,
            keywords: vec!["meridian".into()],
            archived: false,
            non_billable: false,
        },
        Project {
            id: "internal".into(),
            name: "Internal".into(),
            client: String::new(),
            color: [0xc9, 0xb4, 0x58],
            rate: 0.0,
            keywords: vec!["handbook".into(), "timesheet".into(), "q4 budget".into()],
            archived: false,
            non_billable: true,
        },
    ];
    s.goals = vec![
        Goal {
            target: GoalTarget::Category("development".into()),
            minutes: 240,
            limit: false,
            alert: true,
        },
        Goal {
            target: GoalTarget::Category("media".into()),
            minutes: 45,
            limit: true,
            alert: true,
        },
        Goal {
            target: GoalTarget::Total,
            minutes: 360,
            limit: false,
            alert: false,
        },
        Goal {
            target: GoalTarget::Project("atlas".into()),
            minutes: 120,
            limit: false,
            alert: true,
        },
    ];
    s.break_reminder_minutes = 50;
    s.browsers = vec![
        Browser {
            exe: "lumen.exe".into(),
            name: "Lumen Browser".into(),
            source: DomainSource::AddressBar,
        },
        Browser {
            exe: "kestrel.exe".into(),
            name: "Kestrel".into(),
            source: DomainSource::Title,
        },
    ];
    s.privacy.private_apps = vec![
        PrivateApp {
            exe: "vaultkeeper.exe".into(),
            mode: PrivateMode::Skip,
        },
        PrivateApp {
            exe: "ledgerly.exe".into(),
            mode: PrivateMode::NoTitle,
        },
    ];
    s.privacy.redactions = vec![
        Redaction {
            pattern: r"[\w.+-]+@[\w-]+\.[\w.]+".into(),
            replace: "(address)".into(),
            enabled: true,
        },
        Redaction {
            pattern: r"\b(?:\d[ -]?){13,16}\b".into(),
            replace: "(number)".into(),
            enabled: true,
        },
    ];
    s.last_task = "Checkout flow refactor".into();
    s.last_task_project = Some("atlas".into());
    s.repair();
    s
}

/// xorshift64*: small, fast, good enough to invent a working day.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `lo..=hi`
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() >> 11) as i64 % (hi - lo + 1)
    }

    /// True with probability `percent` / 100.
    fn chance(&mut self, percent: i64) -> bool {
        self.range(0, 99) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.range(0, items.len() as i64 - 1) as usize]
    }

    /// An index chosen by weight.
    fn weighted(&mut self, weights: &[i64]) -> usize {
        let total: i64 = weights.iter().sum();
        let mut roll = self.range(0, total.max(1) - 1);
        for (i, w) in weights.iter().enumerate() {
            if roll < *w {
                return i;
            }
            roll -= w;
        }
        weights.len() - 1
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Atlas,
    Orbit,
    Meridian,
}

struct Window {
    exe: &'static str,
    title: String,
    domain: &'static str,
    /// Seconds: shortest and longest stay.
    stay: (i64, i64),
}

fn win(exe: &'static str, title: String, domain: &'static str, stay: (i64, i64)) -> Window {
    Window {
        exe,
        title,
        domain,
        stay,
    }
}

const ATLAS_FILES: [&str; 7] = [
    "cart.rs",
    "checkout.rs",
    "inventory.rs",
    "orders_api.rs",
    "payment_form.tsx",
    "catalog.test.ts",
    "README.md",
];
const ORBIT_FILES: [&str; 6] = [
    "LoginScreen.kt",
    "SyncWorker.kt",
    "ProfileView.kt",
    "OfflineCache.kt",
    "build.gradle.kts",
    "strings.xml",
];
const HELP_TOPICS: [&str; 6] = [
    "Debounce input without a timer",
    "Borrowed value does not live long enough",
    "Retry a failed sync with backoff",
    "Round money without floating point",
    "Center a grid item both ways",
    "Coroutine cancelled before it starts",
];
const PULL_REQUESTS: [(&str, &str); 5] = [
    ("214", "Fix cart totals with mixed tax rates"),
    ("219", "Checkout: remember the delivery address"),
    ("221", "Inventory sync job"),
    ("87", "Offline cache for the profile screen"),
    ("91", "Login: clearer error messages"),
];
const NEWS: [&str; 5] = [
    "Morning briefing",
    "City approves new tram line",
    "Ten quiet keyboards compared",
    "Weekend weather outlook",
    "How lighthouses were automated",
];
const VIDEOS: [&str; 5] = [
    "Rainy cafe ambience (3 hours)",
    "Restoring a 1970s desk lamp",
    "Speedrun explained in ten minutes",
    "Sourdough from start to finish",
    "Keyboard build stream",
];
const MIXES: [&str; 4] = [
    "Evening Jazz Mix",
    "Deep Focus Piano",
    "Synth Roads",
    "Acoustic Morning",
];
const CHANNELS: [&str; 5] = [
    "#atlas-shop",
    "#orbit-app",
    "#general",
    "#design-review",
    "Direct message",
];
const MAIL: [&str; 6] = [
    "Inbox",
    "Re: Atlas launch checklist",
    "Orbit beta feedback, week 45",
    "Re: Meridian logo round 3",
    "Invoice for (address)",
    "Drafts",
];

/// A window from the development cluster of a project.
fn dev_window(rng: &mut Rng, focus: Focus) -> Window {
    let (repo, files): (&str, &[&str]) = match focus {
        Focus::Orbit => ("orbit-app", &ORBIT_FILES),
        _ => ("atlas-shop", &ATLAS_FILES),
    };
    match rng.weighted(&[52, 18, 12, 9, 9]) {
        0 => win(
            "quillcode.exe",
            format!("{} - {repo} - Quill Code", rng.pick(files)),
            "",
            (150, 840),
        ),
        1 => {
            let cmd = *rng.pick(&["test", "build", "git log", "run dev"]);
            win(
                "tern.exe",
                format!("{cmd} - ~/work/{repo} - Tern"),
                "",
                (40, 300),
            )
        }
        2 => {
            let (n, title) = *rng.pick(&PULL_REQUESTS);
            win(
                "lumen.exe",
                format!("Pull request #{n}: {title} · {repo} - Lumen"),
                "codehost.example",
                (90, 420),
            )
        }
        3 => win(
            "lumen.exe",
            format!("{} - StackHelp - Lumen", rng.pick(&HELP_TOPICS)),
            "stackhelp.example",
            (50, 260),
        ),
        _ => {
            let page = *rng.pick(&["Collections", "Error handling", "Layouts", "Testing"]);
            win(
                "lumen.exe",
                format!("{page} - DevDocs - Lumen"),
                "devdocs.example",
                (60, 300),
            )
        }
    }
}

fn design_window(rng: &mut Rng, focus: Focus) -> Window {
    match rng.weighted(&[70, 30]) {
        0 => {
            let file = match focus {
                Focus::Orbit => *rng.pick(&["Orbit onboarding screens", "Orbit icon set"]),
                _ => *rng.pick(&[
                    "Meridian logo v3",
                    "Meridian packaging",
                    "Meridian menu board",
                ]),
            };
            win(
                "vectra.exe",
                format!("{file}.vec - Vectra Studio"),
                "",
                (180, 900),
            )
        }
        _ => win(
            "lumen.exe",
            "Meridian moodboard - Designboard - Lumen".into(),
            "designboard.example",
            (60, 360),
        ),
    }
}

fn comms_window(rng: &mut Rng) -> Window {
    match rng.weighted(&[40, 40, 20]) {
        0 => win(
            "postbird.exe",
            format!("{} - Postbird Mail", rng.pick(&MAIL)),
            "",
            (40, 260),
        ),
        1 => win(
            "huddle.exe",
            format!("{} - Huddle", rng.pick(&CHANNELS)),
            "",
            (30, 220),
        ),
        _ => win(
            "lumen.exe",
            "Inbox (3) - Mailbox - Lumen".into(),
            "mailbox.example",
            (40, 200),
        ),
    }
}

fn writing_window(rng: &mut Rng, focus: Focus) -> Window {
    match rng.weighted(&[55, 25, 20]) {
        0 => {
            let doc = match focus {
                Focus::Meridian => "Proposal - Meridian rebrand",
                Focus::Orbit => "Orbit beta test plan",
                Focus::Atlas => "Atlas release notes",
            };
            win(
                "pagesmith.exe",
                format!("{doc}.doc - PageSmith"),
                "",
                (120, 700),
            )
        }
        1 => {
            let sheet = *rng.pick(&["Q4 budget", "Timesheet November", "Atlas price list"]);
            win(
                "gridworks.exe",
                format!("{sheet}.grid - GridWorks"),
                "",
                (90, 480),
            )
        }
        _ => {
            let note = *rng.pick(&["Meeting notes", "Team handbook", "Ideas"]);
            win("notewell.exe", format!("{note} - Notewell"), "", (60, 300))
        }
    }
}

fn browsing_window(rng: &mut Rng) -> Window {
    match rng.weighted(&[45, 30, 25]) {
        0 => win(
            "lumen.exe",
            format!("{} - The Daily Ledger - Lumen", rng.pick(&NEWS)),
            "dailyledger.example",
            (40, 280),
        ),
        1 => {
            let page = *rng.pick(&["Lighthouse", "Tram", "Sourdough", "Mechanical keyboard"]);
            win(
                "lumen.exe",
                format!("{page} - WikiPage - Lumen"),
                "wikipage.example",
                (40, 240),
            )
        }
        _ => win(
            "lumen.exe",
            "Desk lamps - ShopMart - Lumen".into(),
            "shopmart.example",
            (40, 220),
        ),
    }
}

fn media_window(rng: &mut Rng) -> Window {
    match rng.weighted(&[45, 25, 30]) {
        0 => win(
            "lumen.exe",
            format!("{} - VideoTube - Lumen", rng.pick(&VIDEOS)),
            "videotube.example",
            (120, 560),
        ),
        1 => win(
            "lumen.exe",
            "Home - Chirper - Lumen".into(),
            "chirper.example",
            (40, 240),
        ),
        _ => win(
            "tunebox.exe",
            format!("{} - Tunebox", rng.pick(&MIXES)),
            "",
            (30, 150),
        ),
    }
}

fn utility_window(rng: &mut Rng) -> Window {
    match rng.weighted(&[75, 25]) {
        0 => {
            let folder = *rng.pick(&["Downloads", "work", "Screenshots", "Invoices 2026"]);
            win("folderly.exe", format!("{folder} - Folderly"), "", (15, 80))
        }
        // Recorded without a title (see the privacy examples).
        _ => win("ledgerly.exe", String::new(), "", (40, 200)),
    }
}

struct Gen {
    rng: Rng,
    blocks: Vec<Block>,
    next_id: u64,
}

impl Gen {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn active(&mut self, start: i64, end: i64, w: &Window) {
        if end <= start {
            return;
        }
        // The same window twice in a row is one block.
        if let Some(last) = self.blocks.last_mut() {
            if last.kind == Kind::Active
                && last.end == start
                && last.exe == w.exe
                && last.title == w.title
            {
                last.end = end;
                return;
            }
        }
        let id = self.id();
        self.blocks.push(Block {
            id,
            start,
            end,
            exe: w.exe.into(),
            app: friendly(w.exe).into(),
            title: w.title.clone(),
            domain: w.domain.into(),
            ..Default::default()
        });
    }

    fn away(&mut self, start: i64, end: i64, label: AwayLabel, note: &str, tags: &[&str]) {
        let id = self.id();
        self.blocks.push(Block {
            id,
            start,
            end,
            kind: Kind::Away,
            away: label,
            title: note.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        });
    }

    fn overlay(&mut self, kind: Kind, start: i64, end: i64, title: &str, project: Option<&str>) {
        let id = self.id();
        self.blocks.push(Block {
            id,
            start,
            end,
            kind,
            title: title.into(),
            project: project.map(str::to_string),
            ..Default::default()
        });
    }

    /// Fills `[from, to)` with windows. `mix` weighs development, design,
    /// communication, writing, browsing, media and utilities.
    fn work(&mut self, from: i64, to: i64, focus: Focus, mix: [i64; 7]) {
        let mut t = from;
        let mut cluster = self.rng.weighted(&mix);
        while t < to {
            // Mostly stay with what you were doing.
            if !self.rng.chance(62) {
                cluster = self.rng.weighted(&mix);
            }
            let w = match cluster {
                0 => dev_window(&mut self.rng, focus),
                1 => design_window(&mut self.rng, focus),
                2 => comms_window(&mut self.rng),
                3 => writing_window(&mut self.rng, focus),
                4 => browsing_window(&mut self.rng),
                5 => media_window(&mut self.rng),
                _ => utility_window(&mut self.rng),
            };
            let stay = self.rng.range(w.stay.0, w.stay.1);
            let end = (t + stay).min(to);
            self.active(t, end, &w);
            t = end;
        }
    }

    fn evening(&mut self, from: i64, to: i64) {
        let mut t = from;
        let game = self.rng.chance(45);
        while t < to {
            let w = if game && self.rng.chance(70) {
                win("starfall.exe", "Starfall Tactics".into(), "", (600, 2400))
            } else if self.rng.chance(70) {
                media_window(&mut self.rng)
            } else {
                browsing_window(&mut self.rng)
            };
            let stay = self.rng.range(w.stay.0, w.stay.1);
            let end = (t + stay).min(to);
            self.active(t, end, &w);
            t = end;
        }
    }

    fn workday(&mut self, day: Day, index: i32) {
        const M: i64 = 60;
        let midnight = day as i64 * 86_400 - 3600;
        let at = |h: i64, m: i64| midnight + h * 3600 + m * M;
        // Projects come in runs of a few days.
        let focus = match (index / 3 + self.rng.range(0, 1) as i32) % 4 {
            0 | 3 => Focus::Atlas,
            1 => Focus::Orbit,
            _ => Focus::Meridian,
        };
        let mix: [i64; 7] = match focus {
            Focus::Meridian => [16, 40, 13, 13, 7, 6, 5],
            _ => [54, 5, 13, 9, 7, 7, 5],
        };

        let start = at(8, 30) + self.rng.range(0, 60) * M;
        let lunch = at(12, 0) + self.rng.range(0, 45) * M;
        let lunch_len = self.rng.range(35, 60) * M;
        let end = at(16, 50) + self.rng.range(0, 85) * M;

        // The fixed points of the day: (start, length, label, note, tags).
        let mut stops: Vec<(i64, i64, AwayLabel, &str, &[&str])> = Vec::new();
        if self.rng.chance(60) {
            let (note, tags): (&str, &[&str]) = *self.rng.pick(&[
                ("Daily stand-up", &["team"] as &[&str]),
                ("Sprint planning", &["team", "planning"] as &[&str]),
                ("Design review", &["design"] as &[&str]),
            ]);
            stops.push((
                at(9, 45) + self.rng.range(0, 50) * M,
                self.rng.range(15, 50) * M,
                AwayLabel::Meeting,
                note,
                tags,
            ));
        }
        stops.push((lunch, lunch_len, AwayLabel::Break, "Lunch", &[]));
        if self.rng.chance(50) {
            let call = self.rng.chance(45);
            stops.push((
                at(14, 0) + self.rng.range(0, 80) * M,
                self.rng.range(12, 45) * M,
                if call {
                    AwayLabel::Call
                } else {
                    AwayLabel::Meeting
                },
                if call { "Client call" } else { "Project sync" },
                if call { &["client"] } else { &["team"] },
            ));
        }
        // Short absences: coffee, a knock on the door.
        for _ in 0..self.rng.range(1, 3) {
            let when = start + self.rng.range(30, 480) * M;
            let brk = self.rng.chance(35);
            stops.push((
                when,
                self.rng.range(6, 16) * M,
                if brk {
                    AwayLabel::Break
                } else {
                    AwayLabel::None
                },
                "",
                &[],
            ));
        }
        stops.sort_by_key(|s| s.0);

        let mut t = start;
        for (when, len, label, note, tags) in stops {
            if when <= t || when >= end {
                continue;
            }
            self.work(t, when, focus, mix);
            let stop_end = (when + len).min(end);
            self.away(when, stop_end, label, note, tags);
            t = stop_end;
        }
        if t < end {
            // The last stretch of the day drifts a little.
            let mut late = mix;
            late[5] += 6;
            late[4] += 4;
            self.work(t, end, focus, late);
        }

        // Overlays: a timed task on some days, a focus session on others.
        let project = match focus {
            Focus::Atlas => "atlas",
            Focus::Orbit => "orbit",
            Focus::Meridian => "meridian",
        };
        if self.rng.chance(45) {
            let task = match focus {
                Focus::Atlas => *self.rng.pick(&[
                    "Checkout flow refactor",
                    "Cart totals bug",
                    "Inventory sync job",
                ]),
                Focus::Orbit => *self.rng.pick(&["Offline cache", "Login error messages"]),
                Focus::Meridian => *self.rng.pick(&["Logo round 3", "Packaging mock-ups"]),
            };
            let from = lunch + lunch_len + self.rng.range(5, 40) * M;
            let to = (from + self.rng.range(45, 120) * M).min(end);
            if to > from {
                self.overlay(Kind::Timer, from, to, task, Some(project));
            }
        }
        if self.rng.chance(35) {
            let from = start + self.rng.range(10, 45) * M;
            let len = *self.rng.pick(&[25, 25, 50]) * M;
            self.overlay(Kind::Focus, from, from + len, "Focus session", None);
        }
        // Some evenings the PC is used again.
        if self.rng.chance(22) {
            let from = at(20, 0) + self.rng.range(0, 50) * M;
            let to = from + self.rng.range(30, 95) * M;
            self.evening(from, to);
        }
    }

    fn weekend(&mut self, day: Day) {
        const M: i64 = 60;
        let midnight = day as i64 * 86_400 - 3600;
        if !self.rng.chance(42) {
            return;
        }
        let from = midnight + 10 * 3600 + self.rng.range(0, 240) * M;
        let to = from + self.rng.range(40, 170) * M;
        if self.rng.chance(35) {
            // A little catching up.
            self.work(from, to, Focus::Atlas, [40, 5, 10, 10, 15, 15, 5]);
        } else {
            self.evening(from, to);
        }
    }
}

/// The invented blocks, sorted by start: about [`DAYS`] days ending at [`now`].
pub fn blocks() -> Vec<Block> {
    let now = now();
    let today = tz().day_of(now);
    let mut g = Gen {
        rng: Rng(0x9E37_79B9_7F4A_7C15),
        blocks: Vec::new(),
        next_id: 1_000_000,
    };
    for index in 0..DAYS {
        let day = today - (DAYS - 1) + index;
        let before = g.blocks.len();
        if weekday(day) >= 5 {
            g.weekend(day);
        } else if day != today && (index % 23 == 11 || index % 37 == 30) {
            // A day off.
        } else {
            g.workday(day, index);
        }
        if day == today {
            // Today only exists up to "now", and it ends at the keyboard.
            let mut todays: Vec<Block> = g.blocks.split_off(before);
            todays.retain(|b| b.start < now);
            for b in &mut todays {
                b.end = b.end.min(now);
            }
            while todays
                .iter()
                .rev()
                .find(|b| !b.is_overlay())
                .is_some_and(|b| b.kind == Kind::Away)
            {
                let at = todays.iter().rposition(|b| !b.is_overlay());
                if let Some(at) = at {
                    todays.remove(at);
                }
            }
            let last_end = todays
                .iter()
                .filter(|b| !b.is_overlay())
                .map(|b| b.end)
                .max()
                .unwrap_or(now - 3600);
            g.blocks.append(&mut todays);
            if last_end < now {
                g.work(last_end, now, Focus::Atlas, [60, 4, 12, 8, 6, 5, 5]);
            }
        }
    }

    // Offline work added by hand.
    let midnight = |back: i32| (today - back) as i64 * 86_400 - 3600;
    let id = g.id();
    g.blocks.push(Block {
        id,
        start: midnight(9) + 18 * 3600 + 1800,
        end: midnight(9) + 20 * 3600,
        kind: Kind::Manual,
        title: "Client workshop at Harbor & Pine".into(),
        project: Some("atlas".into()),
        category: Some("communication".into()),
        note: "On site, no laptop. Agreed on the launch checklist.".into(),
        tags: vec!["client".into(), "billable".into()],
        ..Default::default()
    });
    let id = g.id();
    g.blocks.push(Block {
        id,
        start: midnight(2) + 7 * 3600 + 2400,
        end: midnight(2) + 8 * 3600 + 900,
        kind: Kind::Manual,
        title: "Sketching packaging ideas on paper".into(),
        project: Some("meridian".into()),
        category: Some("design".into()),
        note: "Three directions; photographed for the moodboard.".into(),
        tags: vec!["sketch".into()],
        ..Default::default()
    });
    // A note on the newest client call.
    if let Some(call) = g
        .blocks
        .iter_mut()
        .rev()
        .find(|b| b.kind == Kind::Away && b.away == AwayLabel::Call)
    {
        call.note = "Asked for the invoice to be split by project.".into();
    }

    g.blocks.sort_by_key(|b| (b.start, b.id));
    g.blocks
}

/// For `--view timeline-rtl`: gives part of today's invented blocks Kurdish
/// and Arabic window titles, and adds a project with a Kurdish name, to show
/// (and to test by eye) how right-to-left text is drawn.
pub fn showcase_rtl(store: &mut Store, settings: &mut Settings) {
    const TITLES: [&str; 6] = [
        "ڕاپۆرتی مانگانەی فرۆشتن – پەڕگەی کار",
        "پلانی پڕۆژەی ماڵپەڕی نوێ.docx",
        "تقرير المبيعات الشهري 2026.xlsx",
        "کۆبوونەوەی تیم – تێبینییەکان",
        "قائمة المهام – مشروع الموقع",
        "ماڵپەڕ: دیزاینی لاپەڕەی سەرەکی (v2)",
    ];
    if settings.project("malper").is_none() {
        settings.projects.push(Project {
            id: "malper".into(),
            name: "ماڵپەڕی کۆمپانیا".into(),
            client: "کۆمپانیای ڕێگا".into(),
            color: [0xff, 0x92, 0x48],
            rate: 60.0,
            keywords: vec!["ماڵپەڕ".into(), "الموقع".into()],
            archived: false,
            non_billable: false,
        });
    }
    let (from, to) = tz().day_bounds(tz().day_of(now()));
    let mut changed = Vec::new();
    let mut seen = 0;
    for mut b in store.range(from, to) {
        if b.kind != Kind::Active || b.title.is_empty() {
            continue;
        }
        seen += 1;
        if seen % 2 == 0 {
            continue;
        }
        b.title = TITLES[(seen / 2) % TITLES.len()].to_string();
        if seen % 6 == 1 {
            b.note = "پێداچوونەوە لەگەڵ تیم".into();
            b.tags = vec!["کار".into()];
        }
        changed.push(b);
    }
    let _ = store.put_many(&changed);
}

/// For `--view review`: turns part of the last week's invented time into
/// programs and websites that no rule knows, so the Review page has
/// something to show.
pub fn showcase_review(store: &mut Store) {
    const PROGRAMS: [(&str, &str); 3] = [
        ("novaboard.exe", "Nova Board"),
        ("ledgerly.exe", "Ledgerly"),
        ("pixelpress.exe", "PixelPress"),
    ];
    const SITES: [&str; 3] = [
        "papertrail.example",
        "greenmarket.example",
        "cloudnotes.example",
    ];
    let today = tz().day_of(now());
    let (from, to) = (tz().day_start(today - 6), tz().day_start(today + 1));
    let mut changed = Vec::new();
    let mut seen = 0;
    for mut b in store.range(from, to) {
        if b.kind != Kind::Active {
            continue;
        }
        seen += 1;
        if !b.domain.is_empty() && seen % 5 == 0 {
            b.domain = SITES[(seen / 5) % SITES.len()].to_string();
            b.title = "Untitled page".into();
        } else if b.domain.is_empty() && seen % 7 == 0 {
            let (exe, app) = PROGRAMS[(seen / 7) % PROGRAMS.len()];
            b.exe = exe.into();
            b.app = app.into();
            b.title = "Untitled".into();
        } else {
            continue;
        }
        changed.push(b);
    }
    let _ = store.put_many(&changed);
}

/// Fills the store with a few months of plausible invented activity ending at [`now`].
pub fn populate(store: &mut Store, _settings: &Settings) {
    let _ = store.put_many(&blocks());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OTHER;
    use crate::rules::Classifier;
    use crate::spans::{flatten, worked};

    #[test]
    fn deterministic() {
        let a = blocks();
        let b = blocks();
        assert_eq!(a, b);
        assert!(a.len() > 3000, "{} blocks", a.len());
        let mut ids: Vec<u64> = a.iter().map(|b| b.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), a.len());
    }

    #[test]
    fn nothing_after_now_and_no_overlaps() {
        let all = blocks();
        let now = now();
        for b in &all {
            assert!(b.end > b.start, "empty block {}", b.id);
            assert!(b.end <= now, "block {} crosses now", b.id);
        }
        let mut last_end = i64::MIN;
        for b in all
            .iter()
            .filter(|b| matches!(b.kind, Kind::Active | Kind::Away))
        {
            assert!(b.start >= last_end, "block {} overlaps", b.id);
            last_end = b.end;
        }
        // Today ends at the keyboard, right now.
        let last = all.iter().rev().find(|b| !b.is_overlay()).unwrap();
        assert_eq!(last.kind, Kind::Active);
        assert_eq!(last.end, now);
    }

    #[test]
    fn days_are_plausible_and_everything_appears() {
        let s = settings();
        let c = Classifier::new(&s);
        let all = blocks();
        let tz = tz();
        let today = tz.day_of(now());
        let mut days_with_work = 0;
        let mut seen_categories = vec![false; c.categories.len()];
        let mut seen_projects = vec![false; c.projects.len()];
        for day in today - (DAYS - 1)..=today {
            let (a, b) = tz.day_bounds(day);
            let spans = flatten(&all, a, b, &c);
            let w = worked(&spans);
            assert!(w < 14 * 3600, "day {day}: {w} s");
            if w > 0 {
                days_with_work += 1;
            }
            for span in spans.iter().filter(|s| s.work) {
                seen_categories[span.category as usize] = true;
                if let Some(p) = span.project {
                    seen_projects[p as usize] = true;
                }
            }
        }
        assert!(days_with_work > 60 && days_with_work < DAYS);
        for (i, seen) in seen_categories.iter().enumerate() {
            // Nothing invented is left uncategorised.
            assert_eq!(*seen, c.categories[i].id != OTHER, "{}", c.categories[i].id);
        }
        assert!(seen_projects.iter().all(|s| *s));
        // Today has a real morning behind it.
        let (a, b) = tz.day_bounds(today);
        let w = worked(&flatten(&all, a, b, &c));
        assert!(w > 4 * 3600 && w < 9 * 3600, "today: {w} s");
        assert!(all.iter().any(|b| b.kind == Kind::Timer));
        assert!(all.iter().any(|b| b.kind == Kind::Focus));
        assert_eq!(all.iter().filter(|b| b.kind == Kind::Manual).count(), 2);
        assert!(all
            .iter()
            .all(|b| b.domain.is_empty() || b.domain.ends_with(".example")));
    }

    #[test]
    fn showcases_change_what_they_say() {
        use crate::aggregate::Data;
        // Right-to-left titles and a project that matches some of them.
        let mut s = settings();
        let mut store = Store::in_memory();
        populate(&mut store, &s);
        let before = store.range(0, i64::MAX).len();
        showcase_rtl(&mut store, &mut s);
        showcase_rtl(&mut store, &mut s);
        assert_eq!(s.projects.len(), 5);
        let (from, to) = tz().day_bounds(tz().day_of(now()));
        let today = store.range(from, to);
        assert_eq!(store.range(0, i64::MAX).len(), before);
        let c = Classifier::new(&s);
        let rtl = today
            .iter()
            .filter(|b| {
                b.title
                    .chars()
                    .any(|ch| ('\u{600}'..='\u{6ff}').contains(&ch))
            })
            .count();
        assert!(rtl >= 5, "{rtl} right-to-left titles");
        assert!(today
            .iter()
            .any(|b| c.classify(b).project == c.project_index("malper")));

        // Unknown programs and websites for the Review page.
        let s = settings();
        let c = Classifier::new(&s);
        let mut store = Store::in_memory();
        populate(&mut store, &s);
        let today = tz().day_of(now());
        let (from, to) = (tz().day_start(today - 13), tz().day_start(today + 1));
        let review = |store: &mut Store| {
            let blocks = store.range(from, to);
            let spans = flatten(&blocks, from, to, &c);
            let zone = tz();
            let d = Data {
                blocks: &blocks,
                spans: &spans,
                classifier: &c,
                tz: &zone,
            };
            crate::digest::uncategorised(&d, 300)
        };
        // The plain demo has a rule for everything.
        assert!(review(&mut store).is_empty());
        showcase_review(&mut store);
        let rows = review(&mut store);
        assert!(rows.len() >= 4, "{rows:?}");
        assert!(rows.iter().any(|r| r.is_site) && rows.iter().any(|r| !r.is_site));
    }

    #[test]
    fn settings_are_consistent() {
        let mut s = settings();
        let before = s.clone();
        s.repair();
        assert_eq!(s, before);
        assert_eq!(s.projects.len(), 4);
        assert_eq!(s.goals.len(), 4);
        let c = Classifier::new(&s);
        let cat = |exe: &str, title: &str, domain: &str| {
            c.categories[c.category_for(exe, title, domain)].id.clone()
        };
        assert_eq!(cat("quillcode.exe", "", ""), "development");
        assert_eq!(cat("lumen.exe", "x", "videotube.example"), "media");
        assert_eq!(cat("lumen.exe", "x", ""), "browsing");
        assert_eq!(
            c.project_for("cart.rs - atlas-shop - Quill Code", ""),
            c.project_index("atlas")
        );
    }
}
