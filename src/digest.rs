//! Two small digests of recorded time: last week in a sentence (the weekly
//! summary notification) and the list of time no rule has sorted yet (the
//! Review page). Both are pure: blocks and rules in, numbers out.

use crate::aggregate::Data;
use crate::model::{Block, Kind, Score};
use crate::rules::Classifier;
use crate::spans;
use crate::timeutil::{self, Day, Tz};
use std::collections::HashMap;

/// One finished week next to the week before it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Week {
    /// First day of the week; it has seven days.
    pub from: Day,
    pub worked: i64,
    /// Worked time of the week before, for the comparison.
    pub prev_worked: i64,
    /// Days of the week with any worked time.
    pub active_days: u32,
    pub productive: i64,
    pub top_category: Option<(String, i64)>,
    pub top_project: Option<(String, i64)>,
}

/// Which week the summary is due for on `today`, if any: the week before the
/// current one, once per week. `last_shown` is the first day of the week in
/// which a summary was last shown.
pub fn due(today: Day, monday_first: bool, last_shown: Option<Day>) -> Option<Day> {
    let this_week = timeutil::week_start(today, monday_first);
    match last_shown {
        Some(shown) if shown >= this_week => None,
        _ => Some(this_week - 7),
    }
}

/// Rolls up the week starting on `from`. `blocks` must cover that week and
/// the one before it.
pub fn week(blocks: &[Block], classifier: &Classifier, tz: &Tz, from: Day) -> Week {
    let (start, end) = (tz.day_start(from), tz.day_start(from + 7));
    let flat = spans::flatten(blocks, start, end, classifier);
    let mut days: HashMap<Day, i64> = HashMap::new();
    let mut categories: HashMap<usize, i64> = HashMap::new();
    let mut projects: HashMap<usize, i64> = HashMap::new();
    let mut productive = 0;
    for span in flat.iter().filter(|s| s.work) {
        let secs = span.secs();
        crate::aggregate::for_each_day(tz, span.start, span.end, |day, a, b| {
            *days.entry(day).or_default() += b - a;
        });
        let category = span.category as usize;
        *categories.entry(category).or_default() += secs;
        if classifier.score(category) == Score::Productive {
            productive += secs;
        }
        if let Some(p) = span.project {
            *projects.entry(p as usize).or_default() += secs;
        }
    }
    // The most time; the earlier one in the settings wins a tie.
    let top = |map: &HashMap<usize, i64>| {
        map.iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(i, s)| (*i, *s))
    };
    let prev_start = tz.day_start(from - 7);
    let prev = spans::flatten(blocks, prev_start, start, classifier);
    Week {
        from,
        worked: spans::worked(&flat),
        prev_worked: spans::worked(&prev),
        active_days: days.values().filter(|s| **s > 0).count() as u32,
        productive,
        top_category: top(&categories).map(|(i, s)| (classifier.categories[i].name.clone(), s)),
        top_project: top(&projects).map(|(i, s)| (classifier.projects[i].name.clone(), s)),
    }
}

/// Title and text of the weekly summary notification; `None` when nothing
/// was recorded that week. Names categories and projects, never window
/// titles or websites.
pub fn week_notification(week: &Week) -> Option<(String, String)> {
    if week.worked < 60 {
        return None;
    }
    let title = format!("Last week: {} worked", timeutil::fmt_dur(week.worked));
    let mut text = format!(
        "{} active day{}",
        week.active_days,
        if week.active_days == 1 { "" } else { "s" }
    );
    if week.prev_worked >= 60 {
        let diff = week.worked - week.prev_worked;
        if diff.abs() >= 60 {
            text.push_str(&format!(
                ", {} {} than the week before",
                timeutil::fmt_dur(diff.abs()),
                if diff > 0 { "more" } else { "less" }
            ));
        } else {
            text.push_str(", the same as the week before");
        }
    }
    text.push('.');
    if let Some((name, secs)) = &week.top_category {
        text.push_str(&format!(
            " Most time: {name} ({}).",
            timeutil::fmt_dur(*secs)
        ));
    }
    if let Some((name, secs)) = &week.top_project {
        text.push_str(&format!(
            " Top project: {name} ({}).",
            timeutil::fmt_dur(*secs)
        ));
    }
    Some((title, text))
}

/// A program or website whose time no rule of its own has sorted.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReviewRow {
    /// A website (the key is its domain) or a program (the key is its file name).
    pub is_site: bool,
    pub key: String,
    /// The name to show: the domain, or the program's friendly name.
    pub label: String,
    /// The program the time was recorded in (the browser, for a website).
    pub exe: String,
    /// Where the time counts for now (index into `Classifier::categories`):
    /// "Other" for a program, usually the browser's category for a website.
    pub category: usize,
    pub secs: i64,
    /// How many recorded blocks are behind it.
    pub blocks: usize,
}

/// Tracked time waiting to be sorted, most time first:
/// - programs that fell into "Other" because no rule matched (a program
///   with a rule that says "Other" has been decided and is not asked about);
/// - websites without a website rule. Their time counts under whatever the
///   browser counts as, which says little about a site.
///
/// Time whose category was set by hand is sorted already and left out, and
/// so is anything shorter than `min_secs` in total.
pub fn uncategorised(d: &Data, min_secs: i64) -> Vec<ReviewRow> {
    let other = d.classifier.other();
    let mut rows: HashMap<(bool, String), ReviewRow> = HashMap::new();
    let mut seen: HashMap<(bool, String), Vec<u32>> = HashMap::new();
    for span in d.spans {
        if !span.work || span.kind != Kind::Active {
            continue;
        }
        let block = d.block(span);
        if block.category.is_some() {
            continue;
        }
        let (is_site, key) = if !block.domain.is_empty() {
            if d.classifier.has_site_rule(&block.domain) {
                continue;
            }
            (true, crate::rules::normalize_domain(&block.domain))
        } else if span.category as usize == other
            && !block.exe.is_empty()
            && !d.classifier.has_program_rule(&block.exe)
        {
            (false, block.exe.to_lowercase())
        } else {
            continue;
        };
        let id = (is_site, key.clone());
        let row = rows.entry(id.clone()).or_insert_with(|| ReviewRow {
            is_site,
            label: if is_site {
                key.clone()
            } else {
                block.app_label().to_string()
            },
            key,
            exe: block.exe.clone(),
            category: span.category as usize,
            secs: 0,
            blocks: 0,
        });
        row.secs += span.secs();
        let sources = seen.entry(id).or_default();
        if !sources.contains(&span.src) {
            sources.push(span.src);
            row.blocks += 1;
        }
    }
    let mut out: Vec<ReviewRow> = rows.into_values().filter(|r| r.secs >= min_secs).collect();
    out.sort_by(|a, b| b.secs.cmp(&a.secs).then_with(|| a.key.cmp(&b.key)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AwayLabel, Field, Project, Rule};
    use crate::settings::Settings;
    use crate::timeutil::days_from_civil;

    fn tz() -> Tz {
        Tz::Fixed(3 * 3600)
    }

    /// Monday 9 November 2026.
    fn monday() -> Day {
        days_from_civil(2026, 11, 9)
    }

    fn settings() -> Settings {
        let mut s = Settings::default();
        s.projects.push(Project {
            id: "atlas".into(),
            name: "Atlas".into(),
            keywords: vec!["atlas".into()],
            ..Default::default()
        });
        s
    }

    fn active(id: u64, day: Day, hour: u32, minutes: i64, exe: &str, title: &str) -> Block {
        let start = tz().at(day, hour, 0);
        Block {
            id,
            start,
            end: start + minutes * 60,
            exe: exe.into(),
            app: exe.trim_end_matches(".exe").to_string(),
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn summary_is_due_once_per_week() {
        let m = monday();
        // Never shown: due for the week before, on any day of the week.
        assert_eq!(due(m, true, None), Some(m - 7));
        assert_eq!(due(m + 3, true, None), Some(m - 7));
        // Shown this week: not again, not even on Sunday.
        assert_eq!(due(m + 6, true, Some(m)), None);
        // The next Monday it is due again, for the week that just ended.
        assert_eq!(due(m + 7, true, Some(m)), Some(m));
        // Weeks that start on Sunday.
        assert_eq!(due(m - 1, false, Some(m - 8)), Some(m - 8));
        assert_eq!(due(m, false, Some(m - 1)), None);
        // A clock set back does not bring the notification back.
        assert_eq!(due(m, true, Some(m + 7)), None);
    }

    #[test]
    fn week_totals_and_text() {
        let s = settings();
        let c = Classifier::new(&s);
        let m = monday();
        let mut blocks = vec![
            // The week before: 2 hours.
            active(1, m - 7, 9, 120, "code.exe", "main.rs"),
            // The week itself: 3 h development (2 of them on Atlas), 1 h media,
            active(2, m, 9, 120, "code.exe", "cart.rs - atlas"),
            active(3, m + 1, 9, 60, "code.exe", "main.rs"),
            active(4, m + 1, 14, 60, "vlc.exe", "film"),
            // a meeting that counts, a break that does not,
            Block {
                id: 5,
                start: tz().at(m + 2, 10, 0),
                end: tz().at(m + 2, 10, 30),
                kind: Kind::Away,
                away: AwayLabel::Meeting,
                ..Default::default()
            },
            Block {
                id: 6,
                start: tz().at(m + 2, 12, 0),
                end: tz().at(m + 2, 13, 0),
                kind: Kind::Away,
                away: AwayLabel::Break,
                ..Default::default()
            },
            // and the week after, which must not leak in.
            active(7, m + 7, 9, 600, "code.exe", "later"),
        ];
        blocks.sort_by_key(|b| b.start);
        let w = week(&blocks, &c, &tz(), m);
        assert_eq!(w.from, m);
        assert_eq!(w.worked, 4 * 3600 + 1800);
        assert_eq!(w.prev_worked, 2 * 3600);
        assert_eq!(w.active_days, 3);
        assert_eq!(w.productive, 3 * 3600);
        assert_eq!(w.top_category, Some(("Development".to_string(), 3 * 3600)));
        assert_eq!(w.top_project, Some(("Atlas".to_string(), 2 * 3600)));

        let (title, text) = week_notification(&w).unwrap();
        assert_eq!(title, "Last week: 4h 30m worked");
        assert_eq!(
            text,
            "3 active days, 2h 30m more than the week before. Most time: Development (3h 00m). Top project: Atlas (2h 00m)."
        );
        // Window titles never reach a notification.
        assert!(!text.contains("cart.rs") && !text.contains("film"));
    }

    #[test]
    fn an_empty_week_says_nothing() {
        let s = settings();
        let c = Classifier::new(&s);
        let w = week(&[], &c, &tz(), monday());
        assert_eq!(w.worked, 0);
        assert_eq!(week_notification(&w), None);
        // No week before to compare with: no comparison.
        let only = [active(1, monday(), 9, 60, "code.exe", "x")];
        let w = week(&only, &c, &tz(), monday());
        let (_, text) = week_notification(&w).unwrap();
        assert_eq!(text, "1 active day. Most time: Development (1h 00m).");
    }

    #[test]
    fn review_lists_only_what_no_rule_sorted() {
        let mut s = settings();
        s.rules.insert(
            0,
            Rule {
                field: Field::Domain,
                pattern: "known.example".into(),
                category: "writing".into(),
                enabled: true,
            },
        );
        let c = Classifier::new(&s);
        let m = monday();
        let site = |id: u64, hour: u32, minutes: i64, domain: &str| Block {
            domain: domain.into(),
            ..active(id, m, hour, minutes, "chrome.exe", "a page")
        };
        let mut blocks = vec![
            active(1, m, 8, 30, "novatool.exe", "one"),
            active(2, m, 9, 20, "NovaTool.exe", "two"),
            active(3, m, 10, 45, "code.exe", "sorted by a starter rule"),
            // An unknown site in a known browser: the site is what needs a rule.
            site(4, 11, 40, "Fresh.Example"),
            site(5, 12, 10, "known.example"),
            // Sorted by hand already.
            Block {
                category: Some("design".into()),
                ..active(6, m, 13, 50, "other.exe", "by hand")
            },
            // Too little to bother with.
            active(7, m, 14, 1, "blip.exe", "short"),
            // Put in Other by hand: sorted too.
            Block {
                category: Some("other".into()),
                ..active(8, m, 15, 30, "misc.exe", "x")
            },
        ];
        blocks.sort_by_key(|b| b.start);
        let (from, to) = (tz().day_start(m), tz().day_start(m + 1));
        let flat = spans::flatten(&blocks, from, to, &c);
        let tz = tz();
        let d = Data {
            blocks: &blocks,
            spans: &flat,
            classifier: &c,
            tz: &tz,
        };
        let rows = uncategorised(&d, 120);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].key, "novatool.exe");
        assert!(!rows[0].is_site);
        assert_eq!(rows[0].secs, 50 * 60);
        assert_eq!(rows[0].blocks, 2);
        assert_eq!(rows[0].label, "novatool");
        assert_eq!(rows[1].key, "fresh.example");
        assert!(rows[1].is_site);
        assert_eq!(rows[1].exe, "chrome.exe");
        assert_eq!(rows[1].secs, 40 * 60);
        // Until it has a rule, the site counts as whatever the browser does.
        assert_eq!(c.categories[rows[1].category].id, "browsing");
        assert_eq!(rows[0].category, c.other());
        // With a lower bar the short one shows up as well.
        assert_eq!(uncategorised(&d, 0).len(), 3);

        // "Leave it in Other" is an answer too: with a rule that says so,
        // the program is not asked about again.
        s.rules.insert(
            0,
            Rule {
                field: Field::Exe,
                pattern: "novatool.exe".into(),
                category: "other".into(),
                enabled: true,
            },
        );
        let c = Classifier::new(&s);
        let flat = spans::flatten(&blocks, from, to, &c);
        let d = Data {
            blocks: &blocks,
            spans: &flat,
            classifier: &c,
            tz: &tz,
        };
        let rows = uncategorised(&d, 120);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "fresh.example");
    }
}
