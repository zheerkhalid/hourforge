//! `--selftest`: a synthetic day through the whole pipeline.
//!
//! Invented events (focus changes, idling, lock, sleep, pause, a private
//! program, a redacted title) go through the privacy filter and the session
//! builder into a store in a temporary folder; the store is reopened,
//! compacted, backed up and restored; the result is classified, rolled up and
//! exported, and every number is checked against what the day was built to
//! contain. Nothing on this PC is observed and the folder is removed.

use crate::aggregate::{self, Data, Filter, GroupBy};
use crate::export::{self, Format};
use crate::model::{
    new_id, AwayLabel, Block, Kind, PrivateApp, PrivateMode, Project, Redaction, Rounding,
};
use crate::rules::{Classifier, PrivacyFilter, Sanitized};
use crate::sessions::{Config, Event, Out, Sample, SessionBuilder};
use crate::settings::Settings;
use crate::store::Store;
use crate::timeutil::{days_from_civil, fmt_dur, Tz};
use crate::{domain, edit, spans};
use std::path::Path;

fn check(log: &mut Vec<String>, what: &str, ok: bool) -> Result<(), String> {
    if ok {
        log.push(format!("ok    {what}"));
        Ok(())
    } else {
        Err(format!("FAIL  {what}"))
    }
}

fn eq<T: PartialEq + std::fmt::Debug>(
    log: &mut Vec<String>,
    what: &str,
    got: T,
    want: T,
) -> Result<(), String> {
    if got == want {
        log.push(format!("ok    {what}: {got:?}"));
        Ok(())
    } else {
        Err(format!("FAIL  {what}: got {got:?}, expected {want:?}"))
    }
}

#[allow(clippy::field_reassign_with_default)]
fn settings() -> Settings {
    let mut s = Settings::default();
    s.projects = vec![
        Project {
            id: "apollo".into(),
            name: "Apollo".into(),
            client: "Northwind".into(),
            color: [0x4f, 0x8c, 0xff],
            rate: 80.0,
            keywords: vec!["apollo".into()],
            archived: false,
            non_billable: false,
        },
        Project {
            id: "zeus".into(),
            name: "Zeus".into(),
            client: "Contoso".into(),
            color: [0x22, 0xc3, 0xa6],
            rate: 100.0,
            keywords: Vec::new(),
            archived: false,
            non_billable: false,
        },
    ];
    s.privacy.private_apps = vec![
        PrivateApp {
            exe: "bank.exe".into(),
            mode: PrivateMode::Skip,
        },
        PrivateApp {
            exe: "diary.exe".into(),
            mode: PrivateMode::NoTitle,
        },
    ];
    s.privacy.redactions = vec![Redaction {
        pattern: r"\b\d{4} \d{4} \d{4} \d{4}\b".into(),
        replace: "####".into(),
        enabled: true,
    }];
    s.timesheet.rounding = Rounding::Up;
    s.timesheet.step_minutes = 60;
    s
}

struct Day<'a> {
    start: i64,
    builder: SessionBuilder,
    privacy: &'a PrivacyFilter,
    store: &'a mut Store,
    returned: Vec<(u64, i64, i64)>,
}

impl Day<'_> {
    fn at(&self, h: i64, m: i64) -> i64 {
        self.start + h * 3600 + m * 60
    }

    fn push(&mut self, event: Event) -> Result<(), String> {
        for out in self.builder.push(event) {
            match out {
                Out::Put(b) => self.store.put(&b).map_err(|e| e.to_string())?,
                Out::Delete(id) => {
                    self.store.delete(id).map_err(|e| e.to_string())?;
                }
                Out::Returned { id, start, end } => self.returned.push((id, start, end)),
            }
        }
        Ok(())
    }

    /// The same steps the tracker takes for a window: address to domain,
    /// then the privacy rules.
    fn sample(&self, exe: &str, app: &str, title: &str, address: &str) -> Option<Sample> {
        let site = domain::domain_from_url(address).unwrap_or_default();
        match self.privacy.apply(exe, title, &site, !address.is_empty()) {
            Sanitized::Skip => None,
            Sanitized::Keep { title, domain } => Some(Sample {
                exe: exe.into(),
                app: app.into(),
                title,
                domain,
            }),
        }
    }

    /// Ticks every five seconds over `[from, to)` with the user active.
    fn work(&mut self, from: i64, to: i64, fg: Option<Sample>) -> Result<(), String> {
        let mut t = from;
        while t < to {
            self.push(Event::Tick {
                t,
                fg: fg.clone(),
                idle_secs: 0,
                media: false,
            })?;
            t += 5;
        }
        Ok(())
    }
}

/// Runs the self-test and returns its log, or the first failure.
pub fn run() -> Result<Vec<String>, String> {
    let dir = std::env::temp_dir().join(format!("hourforge-selftest-{}", new_id()));
    let result = run_in(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let mut log = result?;
    check(&mut log, "temporary folder removed", !dir.exists())?;
    Ok(log)
}

fn run_in(dir: &Path) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let settings = settings();
    let tz = Tz::Fixed(2 * 3600);
    let day = days_from_civil(2026, 3, 10);
    let (day_start, day_end) = tz.day_bounds(day);
    let privacy = PrivacyFilter::new(&settings);
    let classifier = Classifier::new(&settings);

    // ---- 1. Record the day ---------------------------------------------------------
    let mut store = Store::open(dir).map_err(|e| e.to_string())?;
    let returned;
    {
        let mut d = Day {
            start: day_start,
            builder: SessionBuilder::new(Config::default(), tz.clone()),
            privacy: &privacy,
            store: &mut store,
            returned: Vec::new(),
        };
        let code = d.sample("code.exe", "Code", "main.rs - apollo", "");
        d.work(d.at(9, 0), d.at(10, 30), code)?;
        let docs = d.sample(
            "chrome.exe",
            "Chrome",
            "Regex docs",
            "https://docs.rs/regex/latest/regex/",
        );
        check(
            &mut log,
            "only the domain of an address is kept",
            docs.as_ref().is_some_and(|s| s.domain == "docs.rs"),
        )?;
        d.work(d.at(10, 30), d.at(10, 45), docs)?;
        let video = d.sample(
            "chrome.exe",
            "Chrome",
            "Funny clips",
            "www.youtube.com/watch?v=abc",
        );
        d.work(d.at(10, 45), d.at(11, 0), video.clone())?;
        // 11:00 to 11:20: the window stays in front but nobody touches the PC.
        let mut t = d.at(11, 0);
        while t < d.at(11, 20) {
            d.push(Event::Tick {
                t,
                fg: video.clone(),
                idle_secs: (t - d.at(11, 0)) as u32,
                media: false,
            })?;
            t += 5;
        }
        let word = d.sample("winword.exe", "Word", "Report.docx", "");
        d.work(d.at(11, 20), d.at(12, 0), word)?;
        d.push(Event::Lock { t: d.at(12, 0) })?;
        d.push(Event::Unlock { t: d.at(12, 30) })?;
        // A program on the never-record list: a gap.
        let bank = d.sample("bank.exe", "Bank", "Balance 12,345", "");
        check(
            &mut log,
            "a never-record program gives no sample",
            bank.is_none(),
        )?;
        d.work(d.at(12, 30), d.at(13, 0), bank)?;
        let diary = d.sample("diary.exe", "Diary", "Dear diary", "");
        d.work(d.at(13, 0), d.at(13, 30), diary)?;
        let card = d.sample("code.exe", "Code", "Card 1234 5678 9012 3456 notes", "");
        d.work(d.at(13, 30), d.at(14, 0), card)?;
        d.push(Event::Pause { t: d.at(14, 0) })?;
        d.push(Event::Unpause { t: d.at(14, 15) })?;
        let lib = d.sample("code.exe", "Code", "lib.rs - apollo", "");
        d.work(d.at(14, 15), d.at(15, 0), lib)?;
        d.push(Event::Suspend { t: d.at(15, 0) })?;
        d.push(Event::Resume { t: d.at(16, 0) })?;
        let chat = d.sample("slack.exe", "Slack", "general", "");
        d.work(d.at(16, 0), d.at(16, 30), chat)?;
        d.push(Event::Stop { t: d.at(16, 30) })?;
        returned = d.returned;
    }
    let at = |h: i64, m: i64| day_start + h * 3600 + m * 60;
    eq(&mut log, "absences reported on return", returned.len(), 3)?;
    eq(
        &mut log,
        "the idle absence starts at the last input",
        (returned[0].1, returned[0].2),
        (at(11, 0), at(11, 20)),
    )?;
    // The user says the first absence was a meeting.
    let mut meeting = store.get(returned[0].0).ok_or("away block missing")?;
    meeting.away = AwayLabel::Meeting;
    store.put(&meeting).map_err(|e| e.to_string())?;
    // A timer for another project ran over the last coding stretch, and an
    // hour of offline work is added by hand.
    store
        .put(&Block {
            id: new_id(),
            start: at(14, 15),
            end: at(15, 0),
            kind: Kind::Timer,
            title: "Estimate".into(),
            project: Some("zeus".into()),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let mut workshop = edit::manual(at(17, 0), at(18, 0), "Workshop")?;
    workshop.category = Some("design".into());
    store.put(&workshop).map_err(|e| e.to_string())?;
    store.flush().map_err(|e| e.to_string())?;
    let before = store.range(day_start, day_end);
    drop(store);

    // ---- 2. Storage: reopen, compact, back up, restore ------------------------------
    let mut store = Store::open(dir).map_err(|e| e.to_string())?;
    let blocks = store.range(day_start, day_end);
    check(
        &mut log,
        "the store reads back what was written",
        blocks == before,
    )?;
    let report = store.compact().map_err(|e| e.to_string())?;
    check(
        &mut log,
        &format!(
            "compaction drops superseded records ({} dropped, {} to {} bytes)",
            report.records_dropped, report.bytes_before, report.bytes_after
        ),
        report.records_dropped > 0 && report.bytes_after < report.bytes_before,
    )?;
    check(
        &mut log,
        "compaction keeps the content",
        store.range(day_start, day_end) == before,
    )?;
    let backup = dir.join("day.hfbackup");
    let n = store.backup(&backup).map_err(|e| e.to_string())?;
    eq(&mut log, "blocks in the backup", n, before.len())?;
    let mut copy = Store::in_memory();
    let imported = copy.import(&backup).map_err(|e| e.to_string())?;
    eq(
        &mut log,
        "blocks restored from the backup",
        imported.added,
        before.len(),
    )?;
    check(
        &mut log,
        "the restored copy is identical",
        copy.range(day_start, day_end) == before,
    )?;
    let read_only = Store::open_read_only(dir).map_err(|e| e.to_string());
    check(
        &mut log,
        "a read-only view sees the same and refuses writes",
        read_only
            .is_ok_and(|mut s| s.range(day_start, day_end) == before && s.put(&before[0]).is_err()),
    )?;

    // ---- 3. Privacy -----------------------------------------------------------------
    check(
        &mut log,
        "nothing of the never-record program was stored",
        !blocks
            .iter()
            .any(|b| b.exe == "bank.exe" || b.title.contains("Balance")),
    )?;
    check(
        &mut log,
        "the no-title program has time but no title",
        blocks
            .iter()
            .any(|b| b.exe == "diary.exe" && b.title.is_empty() && b.secs() == 1800),
    )?;
    check(
        &mut log,
        "the redaction rule was applied before storing",
        blocks.iter().any(|b| b.title == "Card #### notes")
            && !blocks.iter().any(|b| b.title.contains("1234")),
    )?;
    check(
        &mut log,
        "no full address was stored",
        !blocks
            .iter()
            .any(|b| b.domain.contains('/') || b.title.contains("watch?v")),
    )?;

    // ---- 4. Totals ------------------------------------------------------------------
    let flat = spans::flatten(&blocks, day_start, day_end, &classifier);
    eq(
        &mut log,
        "worked",
        fmt_dur(spans::worked(&flat)),
        "6h 15m".to_string(),
    )?;
    eq(
        &mut log,
        "away (lock and sleep)",
        fmt_dur(spans::away(&flat)),
        "1h 30m".to_string(),
    )?;
    let data = Data {
        blocks: &blocks,
        spans: &flat,
        classifier: &classifier,
        tz: &tz,
    };
    let summary = aggregate::summary(&data);
    eq(&mut log, "summary: worked seconds", summary.worked, 22_500)?;
    eq(
        &mut log,
        "summary: first and last activity",
        (summary.first, summary.last),
        (Some(at(9, 0)), Some(at(18, 0))),
    )?;
    let by_category = aggregate::ranked(&data, GroupBy::Category, &Filter::default());
    let minutes = |name: &str| {
        by_category
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.secs / 60)
            .unwrap_or(0)
    };
    eq(
        &mut log,
        "minutes by category (Development, Design, Communication, Writing, Other, Media)",
        [
            minutes("Development"),
            minutes("Design"),
            minutes("Communication"),
            minutes("Writing"),
            minutes("Other"),
            minutes("Media"),
        ],
        [180, 60, 50, 40, 30, 15],
    )?;
    eq(
        &mut log,
        "categories add up to the total",
        by_category.iter().map(|r| r.secs).sum::<i64>(),
        22_500,
    )?;
    let by_project = aggregate::ranked(&data, GroupBy::Project, &Filter::default());
    let project = |name: &str| {
        by_project
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.secs / 60)
            .unwrap_or(0)
    };
    eq(
        &mut log,
        "minutes by project (keyword rule, then the timer wins)",
        (project("Apollo"), project("Zeus")),
        (90, 45),
    )?;
    let sites = aggregate::ranked(&data, GroupBy::Site, &Filter::default());
    eq(
        &mut log,
        "sites",
        sites.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["docs.rs", "youtube.com"],
    )?;
    let days = aggregate::daily(&data, day, day);
    eq(
        &mut log,
        "daily roll-up",
        days.iter().map(|d| d.worked).sum::<i64>(),
        22_500,
    )?;

    // ---- 5. Export ------------------------------------------------------------------
    let input = export::Input {
        blocks: &blocks,
        settings: &settings,
        tz: &tz,
        from: day_start,
        to: day_end,
        label: "Tue 10 Mar 2026",
    };
    let json: serde_json::Value = serde_json::from_str(&export::report(&input, Format::Json))
        .map_err(|e| format!("FAIL  the JSON report does not parse: {e}"))?;
    eq(
        &mut log,
        "JSON report: worked seconds",
        json["totals"]["worked_seconds"].as_i64(),
        Some(22_500),
    )?;
    let csv = export::report(&input, Format::Csv);
    eq(
        &mut log,
        "CSV report: one row per worked piece",
        csv.lines().count() - 1,
        flat.iter().filter(|s| s.work).count(),
    )?;
    let html = export::report(&input, Format::Html);
    check(
        &mut log,
        "HTML report is self-contained (no addresses, no scripts)",
        html.contains("Tue 10 Mar 2026") && !html.contains("http") && !html.contains("<script"),
    )?;
    let ics = export::report(&input, Format::Ics);
    check(
        &mut log,
        "calendar file: events for the sittings, no window titles in it",
        ics.starts_with("BEGIN:VCALENDAR\r\n")
            && ics.ends_with("END:VCALENDAR\r\n")
            && ics.matches("BEGIN:VEVENT").count() >= 2
            && ics.matches("BEGIN:VEVENT").count() == ics.matches("END:VEVENT").count()
            && !blocks
                .iter()
                .any(|b| b.title.len() > 3 && ics.contains(b.title.as_str())),
    )?;
    let sheet = export::timesheet(&input, None);
    let invoice = export::invoice_html(
        &sheet,
        &input,
        &export::InvoiceMeta {
            number: "INV-TEST",
            issued: day,
            bill_to: "",
        },
    );
    eq(
        &mut log,
        "invoice: one line per billable project, 260 due",
        (
            export::invoice_lines(&sheet).len(),
            invoice.contains("Amount due</span><b>260.00 USD</b>"),
            invoice.contains("http") || invoice.contains("<script"),
        ),
        (2, true, false),
    )?;
    eq(
        &mut log,
        "timesheet: 1h 30m and 45m rounded up to the hour",
        (sheet.total_secs, sheet.total_rounded_secs),
        (135 * 60, 3 * 3600),
    )?;
    eq(
        &mut log,
        "timesheet: 2 h at 80 plus 1 h at 100",
        sheet.total_amount,
        260.0,
    )?;

    // ---- 6. Deleting ----------------------------------------------------------------
    store
        .delete_range(at(13, 15), at(13, 45))
        .map_err(|e| e.to_string())?;
    let after = store.range(day_start, day_end);
    let flat = spans::flatten(&after, day_start, day_end, &classifier);
    eq(
        &mut log,
        "worked after deleting half an hour",
        fmt_dur(spans::worked(&flat)),
        "5h 45m".to_string(),
    )?;
    check(
        &mut log,
        "nothing is left inside the deleted range",
        !after
            .iter()
            .any(|b| b.start < at(13, 45) && b.end > at(13, 15)),
    )?;
    Ok(log)
}

/// `--probe`: what the tracker can read on this PC right now. Looks at the
/// windows that are open but prints counts only: no titles, no program names,
/// no domains. Nothing is stored.
pub fn probe() -> Vec<String> {
    use crate::{uia, winfo};
    let mut log = Vec::new();
    let windows = winfo::visible_windows();
    let browsers: Vec<String> = crate::settings::default_browsers()
        .into_iter()
        .map(|b| b.exe)
        .collect();
    let (mut described, mut titled, mut named) = (0, 0, 0);
    let mut browser_windows = Vec::new();
    for &w in &windows {
        let title = winfo::window_title(w);
        if title.is_empty() {
            continue;
        }
        titled += 1;
        if let Some(fg) = winfo::describe(w) {
            if !fg.exe.is_empty() {
                described += 1;
                if winfo::file_description(&fg.path).is_some() {
                    named += 1;
                }
                if browsers.contains(&fg.exe) {
                    browser_windows.push((w, title));
                }
            }
        }
    }
    log.push(format!("visible windows with a title: {titled}"));
    log.push(format!("  program identified: {described}"));
    log.push(format!("  friendly name available: {named}"));
    log.push(format!(
        "foreground window readable: {}",
        winfo::foreground().is_some_and(|f| !f.exe.is_empty())
    ));
    log.push(format!(
        "idle seconds readable: {}",
        winfo::idle_seconds() < 86_400 * 30
    ));
    log.push(format!(
        "something keeps the display on: {}   fullscreen in front: {}",
        winfo::display_required(),
        winfo::fullscreen_in_front()
    ));
    log.push(format!("browser windows open: {}", browser_windows.len()));
    match uia::AddressBar::new() {
        None => log.push("address bar reading (UI Automation): not available".into()),
        Some(mut bar) => {
            let (mut read, mut domains, mut from_title, mut ms) = (0, 0, 0, 0u128);
            for (w, title) in &browser_windows {
                let started = std::time::Instant::now();
                let text = bar.read(*w);
                ms = ms.max(started.elapsed().as_millis());
                if let Some(text) = text {
                    read += 1;
                    if domain::domain_from_url(&text).is_some() {
                        domains += 1;
                    }
                }
                if domain::domain_from_title(domain::strip_browser_suffix(title)).is_some() {
                    from_title += 1;
                }
            }
            log.push(format!("  address bar read: {read}"));
            log.push(format!("  domain from the address bar: {domains}"));
            log.push(format!(
                "  domain guessed from the title alone: {from_title}"
            ));
            log.push(format!("  slowest address bar read: {ms} ms"));
        }
    }
    log
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_passes() {
        match super::run() {
            Ok(log) => assert!(log.len() > 25),
            Err(e) => panic!("{e}"),
        }
    }
}
