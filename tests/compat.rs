//! Stored data stays readable: `tests/fixtures/v1.0.0` is a data folder
//! written by HourForge 1.0.0 itself (settings, two months of blocks, the
//! index and the tracker's state file). Every later version must open it
//! unchanged, give the options it added their defaults, and be able to go on
//! writing to it.

use hourforge::export::{self, Input};
use hourforge::model::{AwayLabel, Block, GoalTarget, Kind};
use hourforge::rules::Classifier;
use hourforge::settings::{ReturnPrompt, Settings};
use hourforge::spans;
use hourforge::store::Store;
use hourforge::timeutil::{days_from_civil, Tz};
use std::path::{Path, PathBuf};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A scratch copy of the fixture: opening a store may write to it.
fn fixture(name: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1.0.0");
    let dir = std::env::temp_dir().join(format!(
        "hourforge-compat-{name}-{}",
        hourforge::model::new_id()
    ));
    copy_dir(&source, &dir);
    dir
}

/// 08:00 UTC on 10 November 2026, where the fixture's day begins.
fn t0() -> i64 {
    days_from_civil(2026, 11, 10) as i64 * 86_400 + 8 * 3600
}

#[test]
fn settings_of_1_0_0_load_with_new_options_at_their_defaults() {
    let dir = fixture("settings");
    let s = Settings::load(&dir);
    assert!(
        !Settings::damaged_path(&dir).exists(),
        "the 1.0.0 settings file was not understood"
    );
    // What the user had set.
    assert_eq!(s.idle_minutes, 7);
    assert_eq!(s.hotkey, "Ctrl+Alt+H");
    assert!(!s.week_starts_monday);
    assert_eq!(s.return_prompt, ReturnPrompt::Ask);
    assert_eq!(s.timesheet.currency, "EUR");
    assert_eq!(s.categories.len(), 9);
    assert!(s.rules.len() > 80);
    assert_eq!(s.projects.len(), 2);
    assert_eq!(s.projects[0].name, "Atlas");
    assert_eq!(s.projects[0].rate, 50.0);
    assert_eq!(s.projects[1].name, "ماڵپەڕی کۆمپانیا");
    assert!(s.projects[1].archived);
    assert_eq!(s.goals.len(), 1);
    assert_eq!(s.goals[0].target, GoalTarget::Project("atlas".into()));
    // What 1.1 added.
    assert!(s.weekly_summary);
    assert!(s.timesheet.business.is_empty() && s.timesheet.invoice_note.is_empty());
    assert!(s.projects.iter().all(|p| !p.non_billable));
    // Saved by this version, it loads the same again.
    s.save(&dir).unwrap();
    assert_eq!(Settings::load(&dir), s);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn blocks_of_1_0_0_read_back_exactly_and_roll_up_the_same() {
    let dir = fixture("blocks");
    let settings = Settings::load(&dir);
    let mut store = Store::open(&dir).unwrap();
    assert_eq!(store.damaged_lines(), 0);
    let all = store.range(0, i64::MAX);
    let mut ids: Vec<u64> = all.iter().map(|b| b.id).collect();
    ids.sort_unstable();
    // 107 was deleted in 1.0.0 and stays deleted.
    assert_eq!(ids, [90, 101, 102, 103, 104, 105, 106]);
    let get = |id: u64| all.iter().find(|b| b.id == id).unwrap().clone();
    assert_eq!(
        get(105),
        Block {
            id: 105,
            start: t0() + 9000,
            end: t0() + 12_600,
            kind: Kind::Manual,
            title: "Workshop".into(),
            project: Some("atlas".into()),
            category: Some("communication".into()),
            note: "on site".into(),
            tags: vec!["client".into(), "billable".into()],
            ..Default::default()
        }
    );
    assert_eq!(get(102).title, "پلانی پڕۆژەی ماڵپەڕ.docx");
    assert_eq!(get(103).domain, "docs.rs");
    assert_eq!(
        (get(104).kind, get(104).away),
        (Kind::Away, AwayLabel::Meeting)
    );
    assert_eq!((get(106).kind, get(106).secs()), (Kind::Timer, 1800));
    assert_eq!(get(90).start, t0() - 20 * 86_400);

    // The same totals as the day was built to have.
    let tz = Tz::Fixed(0);
    let day = days_from_civil(2026, 11, 10);
    let (from, to) = tz.day_bounds(day);
    let blocks = store.range(from, to);
    let classifier = Classifier::new(&settings);
    let flat = spans::flatten(&blocks, from, to, &classifier);
    // 60 + 30 + 15 minutes tracked, a 30 minute meeting, 60 minutes by hand.
    assert_eq!(spans::worked(&flat), 195 * 60);
    let input = Input {
        blocks: &blocks,
        settings: &settings,
        tz: &tz,
        from,
        to,
        label: "Tue 10 Nov 2026",
    };
    // Atlas: an hour in the editor and the workshop, billable as before.
    let sheet = export::timesheet(&input, Some("atlas"));
    assert_eq!(sheet.total_rounded_secs, 2 * 3600);
    assert_eq!(sheet.total_amount, 100.0);
    assert!(sheet.rows.iter().all(|r| r.billable));
    // The new exports work on old data.
    assert_eq!(export::invoice_lines(&sheet).len(), 1);
    let ics = export::report(&input, export::Format::Ics);
    assert_eq!(ics.matches("BEGIN:VEVENT").count(), 2, "{ics}");

    // This version goes on writing to the same folder.
    let added = Block {
        id: 200,
        start: t0() + 30_000,
        end: t0() + 30_600,
        exe: "code.exe".into(),
        title: "new.rs".into(),
        tags: vec!["later".into()],
        ..Default::default()
    };
    store.put(&added).unwrap();
    store.flush().unwrap();
    drop(store);
    let mut store = Store::open(&dir).unwrap();
    assert_eq!(store.damaged_lines(), 0);
    assert_eq!(store.range(0, i64::MAX).len(), 8);
    assert_eq!(store.get(200), Some(added));
    assert_eq!(store.get(105), Some(get(105)));
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
