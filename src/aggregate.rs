//! Totals and roll-ups for the Today view and the reports.
//!
//! Everything here works on `spans::flatten` output and is pure (no clock, no
//! disk). A [`Data`] bundles what the functions need: the blocks, the spans
//! made from them, the compiled rules and the time zone.
//!
//! Local-day edges are always taken from `Tz::day_bounds`, so days that are
//! 23 or 25 hours long (daylight saving) are split correctly.

use crate::model::{AwayLabel, Block, BlockId, Kind, Score};
use crate::rules::Classifier;
use crate::spans::Span;
use crate::timeutil::{Day, Tz};
use std::collections::HashMap;

/// The input of every roll-up. `spans` must come from
/// `spans::flatten(blocks, ..)` with the same `blocks` slice and classifier.
#[derive(Clone, Copy)]
pub struct Data<'a> {
    pub blocks: &'a [Block],
    pub spans: &'a [Span],
    pub classifier: &'a Classifier,
    pub tz: &'a Tz,
}

impl<'a> Data<'a> {
    pub fn block(&self, span: &Span) -> &'a Block {
        &self.blocks[span.src as usize]
    }

    fn score(&self, span: &Span) -> Score {
        self.classifier.score(span.category as usize)
    }
}

fn score_index(score: Score) -> usize {
    match score {
        Score::Productive => 0,
        Score::Neutral => 1,
        Score::Distracting => 2,
    }
}

/// 0..=100: productive time counts fully, neutral time half, distracting
/// time not at all. 0 when nothing was worked.
pub fn productivity_score(productive: i64, neutral: i64, distracting: i64) -> u32 {
    let total = productive + neutral + distracting;
    if total <= 0 {
        return 0;
    }
    (((productive as f64 + neutral as f64 * 0.5) / total as f64) * 100.0).round() as u32
}

/// The program a worked span belongs to: the file name for tracked time, the
/// label (`Manual entry`, `Meeting`, ...) for the rest.
fn app_key(b: &Block) -> &str {
    if b.kind == Kind::Active && !b.exe.is_empty() {
        &b.exe
    } else {
        b.app_label()
    }
}

/// Calls `f(day, start, end)` for each local-day piece of `[start, end)`.
pub fn for_each_day(tz: &Tz, start: i64, end: i64, mut f: impl FnMut(Day, i64, i64)) {
    if end <= start {
        return;
    }
    let first = tz.day_of(start);
    if tz.day_of(end - 1) == first {
        f(first, start, end);
        return;
    }
    let mut cursor = start;
    let mut day = first;
    while cursor < end {
        let next = tz.day_start(day + 1).max(cursor + 1);
        let piece_end = next.min(end);
        f(day, cursor, piece_end);
        cursor = piece_end;
        day = tz.day_of(cursor);
    }
}

// --------------------------------------------------------------------------------------
// Summary
// --------------------------------------------------------------------------------------

/// Spans shorter than this are not counted as a context switch: they are
/// windows flashing by (a dialog, Alt+Tab passing through).
pub const SWITCH_MIN_SECS: i64 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Summary {
    /// Worked seconds (tracked, manual, meetings and calls).
    pub worked: i64,
    /// Away seconds not labelled as work (idle, locked, breaks).
    pub away: i64,
    /// Start of the first and end of the last worked span.
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub productive: i64,
    pub neutral: i64,
    pub distracting: i64,
    /// See [`productivity_score`].
    pub score: u32,
    /// How often the program in front changed: counted between consecutive
    /// tracked spans of at least [`SWITCH_MIN_SECS`] whose programs differ,
    /// however long the pause between them was.
    pub switches: u32,
}

pub fn summary(d: &Data) -> Summary {
    let mut s = Summary::default();
    let mut last_app: Option<&str> = None;
    for span in d.spans {
        if span.kind == Kind::Away && !span.work {
            s.away += span.secs();
        }
        if !span.work {
            continue;
        }
        let secs = span.secs();
        s.worked += secs;
        s.first = Some(s.first.map_or(span.start, |f: i64| f.min(span.start)));
        s.last = Some(s.last.map_or(span.end, |l: i64| l.max(span.end)));
        match d.score(span) {
            Score::Productive => s.productive += secs,
            Score::Neutral => s.neutral += secs,
            Score::Distracting => s.distracting += secs,
        }
        if span.kind == Kind::Active && secs >= SWITCH_MIN_SECS {
            let key = app_key(d.block(span));
            if let Some(prev) = last_app {
                if prev != key {
                    s.switches += 1;
                }
            }
            last_app = Some(key);
        }
    }
    s.score = productivity_score(s.productive, s.neutral, s.distracting);
    s
}

// --------------------------------------------------------------------------------------
// Ranked totals and drill-down
// --------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GroupBy {
    Category,
    App,
    Project,
    Title,
    Site,
}

impl GroupBy {
    pub const ALL: [GroupBy; 5] = [
        GroupBy::Category,
        GroupBy::App,
        GroupBy::Project,
        GroupBy::Title,
        GroupBy::Site,
    ];

    pub fn name(self) -> &'static str {
        match self {
            GroupBy::Category => "Category",
            GroupBy::App => "App",
            GroupBy::Project => "Project",
            GroupBy::Title => "Title",
            GroupBy::Site => "Site",
        }
    }
}

/// Narrows worked spans down; every set field must match.
#[derive(Clone, PartialEq, Eq, Debug, Default, Hash)]
pub struct Filter {
    /// Index into `Classifier::categories`.
    pub category: Option<usize>,
    /// Program key: the lower-case file name, or the label of non-program time.
    pub app: Option<String>,
    /// `Some(None)` = time without a project.
    pub project: Option<Option<usize>>,
    pub title: Option<String>,
    pub site: Option<String>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    pub fn matches(&self, d: &Data, span: &Span) -> bool {
        if !span.work {
            return false;
        }
        if let Some(c) = self.category {
            if span.category as usize != c {
                return false;
            }
        }
        if let Some(p) = self.project {
            if span.project.map(|x| x as usize) != p {
                return false;
            }
        }
        let b = d.block(span);
        if let Some(app) = &self.app {
            if app_key(b) != app {
                return false;
            }
        }
        if let Some(title) = &self.title {
            if &b.title != title {
                return false;
            }
        }
        if let Some(site) = &self.site {
            if &b.domain != site {
                return false;
            }
        }
        true
    }
}

/// One line of a ranked list.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Row {
    /// What to put into a [`Filter`] to drill into this row: category index,
    /// program key, project index (empty = no project), title or domain.
    pub key: String,
    pub name: String,
    pub secs: i64,
    /// Part of the listed total, 0..=1.
    pub share: f32,
    /// Category or project colour; `None` for apps, titles and sites.
    pub color: Option<[u8; 3]>,
    /// Program file name for the icon (apps and titles); may be empty.
    pub exe: String,
    /// The block of the longest span in the row (to jump to the timeline).
    pub block: BlockId,
    /// Start of that span.
    pub at: i64,
    /// The row made by [`with_other`].
    pub other: bool,
}

struct Acc {
    row: Row,
    longest: i64,
}

/// Worked time grouped and sorted, longest first. Equal totals keep the order
/// in which the groups first appear. Titles and sites that are empty are left
/// out of those two groupings.
pub fn ranked(d: &Data, by: GroupBy, filter: &Filter) -> Vec<Row> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<Acc> = Vec::new();
    for span in d.spans {
        if !filter.matches(d, span) {
            continue;
        }
        let b = d.block(span);
        let (key, name, color, exe): (String, String, Option<[u8; 3]>, &str) = match by {
            GroupBy::Category => {
                let c = &d.classifier.categories[span.category as usize];
                (
                    (span.category as usize).to_string(),
                    c.name.clone(),
                    Some(c.color),
                    "",
                )
            }
            GroupBy::App => (
                app_key(b).to_string(),
                b.app_label().to_string(),
                None,
                if b.kind == Kind::Active { &b.exe } else { "" },
            ),
            GroupBy::Project => match span.project {
                Some(p) => {
                    let project = &d.classifier.projects[p as usize];
                    (
                        (p as usize).to_string(),
                        project.name.clone(),
                        Some(project.color),
                        "",
                    )
                }
                None => (String::new(), "No project".to_string(), None, ""),
            },
            GroupBy::Title => {
                if b.title.is_empty() {
                    continue;
                }
                (b.title.clone(), b.title.clone(), None, &b.exe)
            }
            GroupBy::Site => {
                if b.domain.is_empty() {
                    continue;
                }
                (b.domain.clone(), b.domain.clone(), None, "")
            }
        };
        let at = *index.entry(key.clone()).or_insert_with(|| {
            rows.push(Acc {
                row: Row {
                    key,
                    name,
                    color,
                    exe: exe.to_string(),
                    ..Default::default()
                },
                longest: 0,
            });
            rows.len() - 1
        });
        let acc = &mut rows[at];
        let secs = span.secs();
        acc.row.secs += secs;
        if secs > acc.longest {
            acc.longest = secs;
            acc.row.block = b.id;
            acc.row.at = span.start;
        }
    }
    let total: i64 = rows.iter().map(|a| a.row.secs).sum();
    let mut out: Vec<Row> = rows.into_iter().map(|a| a.row).collect();
    for r in &mut out {
        r.share = if total > 0 {
            r.secs as f32 / total as f32
        } else {
            0.0
        };
    }
    // Stable: ties stay in order of first appearance.
    out.sort_by_key(|r| std::cmp::Reverse(r.secs));
    out
}

/// Keeps the first `max` rows and folds the rest into one "Other" row (only
/// when at least two rows would be folded).
pub fn with_other(mut rows: Vec<Row>, max: usize) -> Vec<Row> {
    if max == 0 || rows.len() <= max + 1 {
        return rows;
    }
    let tail = rows.split_off(max);
    let secs: i64 = tail.iter().map(|r| r.secs).sum();
    let share: f32 = tail.iter().map(|r| r.share).sum();
    rows.push(Row {
        name: format!("Other ({})", tail.len()),
        secs,
        share,
        other: true,
        ..Default::default()
    });
    rows
}

/// One raw piece of worked time, for the last drill-down level.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawRow {
    pub block: BlockId,
    pub start: i64,
    pub end: i64,
    pub app: String,
    pub title: String,
}

/// The worked spans matching a filter, in time order.
pub fn raw(d: &Data, filter: &Filter) -> Vec<RawRow> {
    d.spans
        .iter()
        .filter(|s| filter.matches(d, s))
        .map(|s| {
            let b = d.block(s);
            RawRow {
                block: b.id,
                start: s.start,
                end: s.end,
                app: b.app_label().to_string(),
                title: b.title.clone(),
            }
        })
        .collect()
}

// --------------------------------------------------------------------------------------
// Day timeline
// --------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SegKind {
    /// Worked time in this category (index into `Classifier::categories`).
    Work(usize),
    /// Away time that is not work.
    Away(AwayLabel),
}

/// A stretch of the day bar. Time covered by no segment is a gap: nothing
/// was recorded (PC off, tracking paused, a never-record program).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Segment {
    pub start: i64,
    pub end: i64,
    pub kind: SegKind,
    /// The program with the most time in the segment (empty for away).
    pub app: String,
    /// The first block in the segment.
    pub block: BlockId,
}

/// Pieces closer together than this are drawn as one segment when they have
/// the same category.
pub const MERGE_GAP_SECS: i64 = 5;

/// Worked and away spans merged into the segments of a day bar.
pub fn timeline(d: &Data) -> Vec<Segment> {
    struct Open {
        seg: Segment,
        apps: Vec<(String, i64)>,
    }
    fn close(open: Open, out: &mut Vec<Segment>) {
        let mut seg = open.seg;
        // First of the longest (stable for ties).
        let mut best: Option<&(String, i64)> = None;
        for a in &open.apps {
            if best.is_none_or(|b| a.1 > b.1) {
                best = Some(a);
            }
        }
        seg.app = best.map(|a| a.0.clone()).unwrap_or_default();
        out.push(seg);
    }
    let mut out = Vec::new();
    let mut open: Option<Open> = None;
    for span in d.spans {
        let kind = if span.work {
            SegKind::Work(span.category as usize)
        } else if span.kind == Kind::Away {
            SegKind::Away(d.block(span).away)
        } else {
            continue;
        };
        let b = d.block(span);
        let label = if span.work { b.app_label() } else { "" };
        if let Some(o) = &mut open {
            if o.seg.kind == kind && span.start - o.seg.end <= MERGE_GAP_SECS {
                o.seg.end = o.seg.end.max(span.end);
                if !label.is_empty() {
                    match o.apps.iter_mut().find(|(n, _)| n == label) {
                        Some(a) => a.1 += span.secs(),
                        None => o.apps.push((label.to_string(), span.secs())),
                    }
                }
                continue;
            }
        }
        if let Some(o) = open.take() {
            close(o, &mut out);
        }
        open = Some(Open {
            seg: Segment {
                start: span.start,
                end: span.end,
                kind,
                app: String::new(),
                block: b.id,
            },
            apps: if label.is_empty() {
                Vec::new()
            } else {
                vec![(label.to_string(), span.secs())]
            },
        });
    }
    if let Some(o) = open {
        close(o, &mut out);
    }
    out
}

// --------------------------------------------------------------------------------------
// Focus sessions
// --------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FocusParams {
    /// A stretch must last at least this long to count.
    pub min_secs: i64,
    /// Time not spent on productive work (another category, a pause, a gap)
    /// that a stretch survives. Longer than this ends it.
    pub max_interruption_secs: i64,
}

impl Default for FocusParams {
    fn default() -> Self {
        FocusParams {
            min_secs: 25 * 60,
            max_interruption_secs: 2 * 60,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FocusSession {
    pub start: i64,
    pub end: i64,
    /// Seconds of productive work inside the stretch.
    pub productive: i64,
    /// `true`: a focus-mode session the user started; `false`: found in the
    /// tracked time.
    pub recorded: bool,
    /// The first block of the stretch (the focus block for recorded ones).
    pub block: BlockId,
}

/// Uninterrupted stretches of productive work found in the tracked time, plus
/// the focus-mode sessions that were recorded. Sorted by start.
pub fn focus_sessions(d: &Data, params: &FocusParams) -> Vec<FocusSession> {
    let mut out = Vec::new();
    let mut open: Option<FocusSession> = None;
    let finish = |s: FocusSession, out: &mut Vec<FocusSession>| {
        if s.end - s.start >= params.min_secs {
            out.push(s);
        }
    };
    for span in d.spans {
        if span.kind == Kind::Focus {
            out.push(FocusSession {
                start: span.start,
                end: span.end,
                productive: 0,
                recorded: true,
                block: d.block(span).id,
            });
            continue;
        }
        if !span.work || d.score(span) != Score::Productive {
            continue;
        }
        match &mut open {
            Some(s) if span.start - s.end <= params.max_interruption_secs => {
                s.end = s.end.max(span.end);
                s.productive += span.secs();
            }
            _ => {
                if let Some(s) = open.take() {
                    finish(s, &mut out);
                }
                open = Some(FocusSession {
                    start: span.start,
                    end: span.end,
                    productive: span.secs(),
                    recorded: false,
                    block: d.block(span).id,
                });
            }
        }
    }
    if let Some(s) = open {
        finish(s, &mut out);
    }
    // Productive seconds under a recorded session.
    for s in out.iter_mut().filter(|s| s.recorded) {
        s.productive = d
            .spans
            .iter()
            .filter(|x| x.work && d.score(x) == Score::Productive)
            .map(|x| (x.end.min(s.end) - x.start.max(s.start)).max(0))
            .sum();
    }
    out.sort_by_key(|s| (s.start, !s.recorded));
    out
}

// --------------------------------------------------------------------------------------
// Per-day series, comparison
// --------------------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DayRow {
    pub day: Day,
    pub worked: i64,
    pub away: i64,
    /// Worked seconds per category (same order as `Classifier::categories`).
    pub by_category: Vec<i64>,
    /// Productive, neutral, distracting.
    pub by_score: [i64; 3],
    pub first: Option<i64>,
    pub last: Option<i64>,
}

impl DayRow {
    pub fn score(&self) -> u32 {
        productivity_score(self.by_score[0], self.by_score[1], self.by_score[2])
    }
}

/// One row per local day `from..=to`, including days without data. Spans
/// reaching across midnight are split at the local day edge.
pub fn daily(d: &Data, from: Day, to: Day) -> Vec<DayRow> {
    let cats = d.classifier.categories.len();
    let mut rows: Vec<DayRow> = (from..=to)
        .map(|day| DayRow {
            day,
            worked: 0,
            away: 0,
            by_category: vec![0; cats],
            by_score: [0; 3],
            first: None,
            last: None,
        })
        .collect();
    for span in d.spans {
        let is_away = span.kind == Kind::Away && !span.work;
        if !span.work && !is_away {
            continue;
        }
        let score = score_index(d.score(span));
        for_each_day(d.tz, span.start, span.end, |day, s, e| {
            if day < from || day > to {
                return;
            }
            let row = &mut rows[(day - from) as usize];
            let secs = e - s;
            if is_away {
                row.away += secs;
                return;
            }
            row.worked += secs;
            if let Some(c) = row.by_category.get_mut(span.category as usize) {
                *c += secs;
            }
            row.by_score[score] += secs;
            row.first = Some(row.first.map_or(s, |f| f.min(s)));
            row.last = Some(row.last.map_or(e, |l| l.max(e)));
        });
    }
    rows
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Totals {
    pub worked: i64,
    pub away: i64,
    pub by_category: Vec<i64>,
    pub by_score: [i64; 3],
    pub score: u32,
    /// Days in the period, and days on which anything was worked.
    pub days: u32,
    pub days_with_data: u32,
}

impl Totals {
    /// Worked seconds per day over the days that have data.
    pub fn avg_active_day(&self) -> i64 {
        if self.days_with_data == 0 {
            0
        } else {
            self.worked / self.days_with_data as i64
        }
    }

    /// Worked seconds per day over every day of the period.
    pub fn avg_day(&self) -> i64 {
        if self.days == 0 {
            0
        } else {
            self.worked / self.days as i64
        }
    }
}

pub fn totals(rows: &[DayRow]) -> Totals {
    let mut t = Totals {
        days: rows.len() as u32,
        ..Default::default()
    };
    for r in rows {
        t.worked += r.worked;
        t.away += r.away;
        if r.worked > 0 {
            t.days_with_data += 1;
        }
        if t.by_category.len() < r.by_category.len() {
            t.by_category.resize(r.by_category.len(), 0);
        }
        for (i, v) in r.by_category.iter().enumerate() {
            t.by_category[i] += v;
        }
        for i in 0..3 {
            t.by_score[i] += r.by_score[i];
        }
    }
    t.score = productivity_score(t.by_score[0], t.by_score[1], t.by_score[2]);
    t
}

/// A value now and in the period before.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Delta {
    pub cur: i64,
    pub prev: i64,
}

impl Delta {
    pub fn diff(&self) -> i64 {
        self.cur - self.prev
    }

    /// Change in percent of the previous value; `None` when there was
    /// nothing before (a percentage of zero means nothing).
    pub fn pct(&self) -> Option<f32> {
        (self.prev > 0).then(|| (self.cur - self.prev) as f32 * 100.0 / self.prev as f32)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Comparison {
    pub cur: Totals,
    pub prev: Totals,
    pub worked: Delta,
    pub away: Delta,
    /// Productivity score, in points.
    pub score: Delta,
    pub avg_active_day: Delta,
    pub avg_day: Delta,
    /// (category index, change), for categories with time in either period,
    /// biggest current total first.
    pub categories: Vec<(usize, Delta)>,
}

/// Compares a period with the one before it (see [`previous_period`]).
pub fn compare(cur: &[DayRow], prev: &[DayRow]) -> Comparison {
    let c = totals(cur);
    let p = totals(prev);
    let n = c.by_category.len().max(p.by_category.len());
    let mut categories: Vec<(usize, Delta)> = (0..n)
        .map(|i| {
            (
                i,
                Delta {
                    cur: c.by_category.get(i).copied().unwrap_or(0),
                    prev: p.by_category.get(i).copied().unwrap_or(0),
                },
            )
        })
        .filter(|(_, d)| d.cur > 0 || d.prev > 0)
        .collect();
    categories.sort_by_key(|(_, d)| std::cmp::Reverse(d.cur));
    Comparison {
        worked: Delta {
            cur: c.worked,
            prev: p.worked,
        },
        away: Delta {
            cur: c.away,
            prev: p.away,
        },
        score: Delta {
            cur: c.score as i64,
            prev: p.score as i64,
        },
        avg_active_day: Delta {
            cur: c.avg_active_day(),
            prev: p.avg_active_day(),
        },
        avg_day: Delta {
            cur: c.avg_day(),
            prev: p.avg_day(),
        },
        categories,
        cur: c,
        prev: p,
    }
}

/// The period of equal length right before `from..=to`.
pub fn previous_period(from: Day, to: Day) -> (Day, Day) {
    let len = to - from + 1;
    (from - len, from - 1)
}

// --------------------------------------------------------------------------------------
// Hours
// --------------------------------------------------------------------------------------

/// Worked seconds by weekday (0 = Monday) and hour of the local day.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HourMatrix {
    pub worked: [[i64; 24]; 7],
    pub productive: [[i64; 24]; 7],
}

impl HourMatrix {
    /// Worked seconds per hour of day over all weekdays.
    pub fn by_hour(&self) -> [i64; 24] {
        let mut out = [0; 24];
        for row in &self.worked {
            for (h, v) in row.iter().enumerate() {
                out[h] += v;
            }
        }
        out
    }

    /// The hour of day with the most productive time, if any.
    pub fn best_hour(&self) -> Option<u32> {
        let mut sums = [0i64; 24];
        for row in &self.productive {
            for (h, v) in row.iter().enumerate() {
                sums[h] += v;
            }
        }
        let (hour, best) = sums
            .iter()
            .enumerate()
            .rev()
            .max_by_key(|(_, v)| **v)
            .map(|(h, v)| (h as u32, *v))?;
        (best > 0).then_some(hour)
    }

    pub fn max(&self) -> i64 {
        self.worked
            .iter()
            .flat_map(|r| r.iter())
            .copied()
            .max()
            .unwrap_or(0)
    }
}

pub fn hour_matrix(d: &Data) -> HourMatrix {
    let mut m = HourMatrix::default();
    for span in d.spans {
        if !span.work {
            continue;
        }
        let productive = d.score(span) == Score::Productive;
        let mut t = span.start;
        while t < span.end {
            let local = d.tz.local(t);
            let into_hour = (local.minute * 60 + local.second) as i64;
            let end = (t + 3600 - into_hour).min(span.end);
            let (w, h) = (local.weekday as usize, local.hour as usize);
            m.worked[w][h] += end - t;
            if productive {
                m.productive[w][h] += end - t;
            }
            t = end;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;
    use crate::settings::Settings;
    use crate::spans::{flatten, worked};
    use crate::timeutil::days_from_civil;

    fn classifier() -> Classifier {
        let mut s = Settings::default();
        s.projects.push(Project {
            id: "apollo".into(),
            name: "Apollo".into(),
            keywords: vec!["apollo".into()],
            ..Default::default()
        });
        Classifier::new(&s)
    }

    fn active(id: u64, start: i64, end: i64, exe: &str, title: &str) -> Block {
        Block {
            id,
            start,
            end,
            exe: exe.into(),
            app: exe.trim_end_matches(".exe").into(),
            title: title.into(),
            ..Default::default()
        }
    }

    fn away(id: u64, start: i64, end: i64, label: AwayLabel) -> Block {
        Block {
            id,
            start,
            end,
            kind: Kind::Away,
            away: label,
            ..Default::default()
        }
    }

    struct Fixture {
        blocks: Vec<Block>,
        spans: Vec<Span>,
        c: Classifier,
        tz: Tz,
    }

    impl Fixture {
        fn new(blocks: Vec<Block>, from: i64, to: i64, tz: Tz) -> Fixture {
            let c = classifier();
            let spans = flatten(&blocks, from, to, &c);
            Fixture {
                blocks,
                spans,
                c,
                tz,
            }
        }

        fn data(&self) -> Data<'_> {
            Data {
                blocks: &self.blocks,
                spans: &self.spans,
                classifier: &self.c,
                tz: &self.tz,
            }
        }
    }

    const H: i64 = 3600;

    /// A day in UTC: code 2h, browser 30m, vlc 30m, lunch break 1h,
    /// meeting 1h, code 1h, a 2-second flash of a dialog, word 30m.
    fn sample_day() -> Fixture {
        let d0 = days_from_civil(2026, 11, 10) as i64 * 86_400;
        let t = |h: f64| d0 + (h * 3600.0) as i64;
        let blocks = vec![
            active(1, t(9.0), t(11.0), "code.exe", "main.rs - apollo"),
            active(2, t(11.0), t(11.5), "chrome.exe", "News"),
            active(3, t(11.5), t(12.0), "vlc.exe", "Film"),
            away(4, t(12.0), t(13.0), AwayLabel::Break),
            away(5, t(13.0), t(14.0), AwayLabel::Meeting),
            active(6, t(14.0), t(15.0), "code.exe", "lib.rs - apollo"),
            active(7, t(15.0), t(15.0) + 2, "dialog.exe", ""),
            active(8, t(15.0) + 2, t(15.5), "winword.exe", "Report"),
        ];
        Fixture::new(blocks, d0, d0 + 86_400, Tz::Fixed(0))
    }

    #[test]
    fn summary_adds_up() {
        let f = sample_day();
        let d = f.data();
        let s = summary(&d);
        assert_eq!(s.worked, worked(&f.spans));
        assert_eq!(s.worked, 5 * H + H / 2);
        assert_eq!(s.away, H);
        assert_eq!(s.productive + s.neutral + s.distracting, s.worked);
        // code 3h + word 30m (minus the 2 s flash) productive; vlc distracting.
        assert_eq!(s.productive, 3 * H + H / 2 - 2);
        assert_eq!(s.distracting, H / 2);
        let d0 = days_from_civil(2026, 11, 10) as i64 * 86_400;
        assert_eq!(s.first, Some(d0 + 9 * H));
        assert_eq!(s.last, Some(d0 + 15 * H + H / 2));
        // code -> chrome -> vlc -> code -> word; the flash and the meeting do not count.
        assert_eq!(s.switches, 4);
        assert_eq!(
            s.score,
            productivity_score(s.productive, s.neutral, s.distracting)
        );
        assert!(s.score > 50 && s.score < 100);
    }

    #[test]
    fn score_edges() {
        assert_eq!(productivity_score(0, 0, 0), 0);
        assert_eq!(productivity_score(10, 0, 0), 100);
        assert_eq!(productivity_score(0, 10, 0), 50);
        assert_eq!(productivity_score(0, 0, 10), 0);
        assert_eq!(productivity_score(5, 0, 5), 50);
    }

    #[test]
    fn empty_input() {
        let f = Fixture::new(Vec::new(), 0, 86_400, Tz::Fixed(0));
        let d = f.data();
        assert_eq!(summary(&d), Summary::default());
        assert!(ranked(&d, GroupBy::App, &Filter::default()).is_empty());
        assert!(timeline(&d).is_empty());
        assert!(focus_sessions(&d, &FocusParams::default()).is_empty());
        let rows = daily(&d, 0, 6);
        assert_eq!(rows.len(), 7);
        let t = totals(&rows);
        assert_eq!((t.worked, t.days, t.days_with_data), (0, 7, 0));
        assert_eq!((t.avg_day(), t.avg_active_day(), t.score), (0, 0, 0));
        let m = hour_matrix(&d);
        assert_eq!(m.max(), 0);
        assert_eq!(m.best_hour(), None);
        assert!(raw(&d, &Filter::default()).is_empty());
        assert_eq!(totals(&[]).days, 0);
    }

    #[test]
    fn ranked_totals_and_shares() {
        let f = sample_day();
        let d = f.data();
        for by in [GroupBy::Category, GroupBy::App, GroupBy::Project] {
            let rows = ranked(&d, by, &Filter::default());
            assert_eq!(rows.iter().map(|r| r.secs).sum::<i64>(), worked(&f.spans));
            let share: f32 = rows.iter().map(|r| r.share).sum();
            assert!((share - 1.0).abs() < 1e-4);
            assert!(rows.windows(2).all(|w| w[0].secs >= w[1].secs));
        }
        let apps = ranked(&d, GroupBy::App, &Filter::default());
        assert_eq!(apps[0].key, "code.exe");
        assert_eq!(apps[0].secs, 3 * H);
        assert_eq!(apps[0].exe, "code.exe");
        // The longest code span is the first block.
        assert_eq!(apps[0].block, 1);
        assert_eq!(apps[1].name, "Meeting");
        let cats = ranked(&d, GroupBy::Category, &Filter::default());
        assert_eq!(cats[0].name, "Development");
        assert!(cats[0].color.is_some());
        let projects = ranked(&d, GroupBy::Project, &Filter::default());
        assert_eq!(projects[0].name, "Apollo");
        assert_eq!(projects[0].secs, 3 * H);
        assert_eq!(projects[1].name, "No project");
        // Empty titles are left out of the title ranking.
        let titles = ranked(&d, GroupBy::Title, &Filter::default());
        assert!(titles.iter().all(|r| !r.name.is_empty()));
        assert_eq!(titles[0].name, "main.rs - apollo");
        assert!(ranked(&d, GroupBy::Site, &Filter::default()).is_empty());
    }

    #[test]
    fn ranking_ties_keep_first_appearance() {
        let blocks = vec![
            active(1, 0, 100, "b.exe", "x"),
            active(2, 100, 200, "a.exe", "y"),
            active(3, 200, 300, "c.exe", "z"),
            active(4, 300, 350, "d.exe", "w"),
        ];
        let f = Fixture::new(blocks, 0, 1000, Tz::Fixed(0));
        let rows = ranked(&f.data(), GroupBy::App, &Filter::default());
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["b.exe", "a.exe", "c.exe", "d.exe"]);
    }

    #[test]
    fn other_row() {
        let f = sample_day();
        let rows = ranked(&f.data(), GroupBy::App, &Filter::default());
        let n = rows.len();
        assert!(n >= 5);
        let total: i64 = rows.iter().map(|r| r.secs).sum();
        let folded = with_other(rows.clone(), 2);
        assert_eq!(folded.len(), 3);
        assert!(folded[2].other);
        assert_eq!(folded[2].name, format!("Other ({})", n - 2));
        assert_eq!(folded.iter().map(|r| r.secs).sum::<i64>(), total);
        // Folding a single row would gain nothing.
        assert_eq!(with_other(rows.clone(), n - 1), rows);
        assert_eq!(with_other(rows.clone(), 0), rows);
    }

    #[test]
    fn drill_down() {
        let f = sample_day();
        let d = f.data();
        let dev = f.c.category_index("development").unwrap();
        let mut filter = Filter {
            category: Some(dev),
            ..Default::default()
        };
        let apps = ranked(&d, GroupBy::App, &filter);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].key, "code.exe");
        filter.app = Some("code.exe".into());
        let titles = ranked(&d, GroupBy::Title, &filter);
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0].secs, 2 * H);
        filter.title = Some(titles[1].key.clone());
        let rows = raw(&d, &filter);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].block, 6);
        assert_eq!(rows[0].end - rows[0].start, H);
        // Project filters, including "no project".
        let none = Filter {
            project: Some(None),
            ..Default::default()
        };
        assert_eq!(
            raw(&d, &none).iter().map(|r| r.end - r.start).sum::<i64>(),
            2 * H + H / 2
        );
        let apollo = Filter {
            project: Some(Some(0)),
            ..Default::default()
        };
        assert_eq!(raw(&d, &apollo).len(), 2);
        assert!(Filter::default().is_empty() && !apollo.is_empty());
    }

    #[test]
    fn timeline_merges_same_category() {
        let d0 = 0;
        let blocks = vec![
            active(1, d0, d0 + 600, "code.exe", "a"),
            // Same category, 3 s later: merged.
            active(2, d0 + 603, d0 + 1200, "cmd.exe", "b"),
            active(3, d0 + 1200, d0 + 1500, "vlc.exe", "c"),
            away(4, d0 + 1500, d0 + 2000, AwayLabel::None),
            // A real gap, then the same category again: not merged.
            active(5, d0 + 5000, d0 + 5100, "code.exe", "a"),
            Block {
                kind: Kind::Timer,
                ..active(6, d0, d0 + 3000, "", "task")
            },
        ];
        let f = Fixture::new(blocks, 0, 86_400, Tz::Fixed(0));
        let segs = timeline(&f.data());
        let dev = f.c.category_index("development").unwrap();
        let media = f.c.category_index("media").unwrap();
        assert_eq!(segs.len(), 4);
        assert_eq!(
            (segs[0].start, segs[0].end, segs[0].kind),
            (0, 1200, SegKind::Work(dev))
        );
        assert_eq!(segs[0].app, "code");
        assert_eq!(segs[0].block, 1);
        assert_eq!(segs[1].kind, SegKind::Work(media));
        assert_eq!(segs[2].kind, SegKind::Away(AwayLabel::None));
        assert_eq!(segs[2].app, "");
        assert_eq!((segs[3].start, segs[3].end), (5000, 5100));
    }

    #[test]
    fn focus_sessions_found_and_recorded() {
        let m = 60;
        let blocks = vec![
            // 20 min + 1 min of chat + 15 min: one 36-minute stretch.
            active(1, 0, 20 * m, "code.exe", "a"),
            active(2, 20 * m, 21 * m, "slack.exe", "chat"),
            active(3, 21 * m, 36 * m, "code.exe", "b"),
            // Ten minutes of video end it.
            active(4, 36 * m, 46 * m, "vlc.exe", "film"),
            // Too short to count.
            active(5, 46 * m, 60 * m, "code.exe", "c"),
            // A long pause, then a long stretch.
            active(6, 120 * m, 170 * m, "winword.exe", "doc"),
            Block {
                kind: Kind::Focus,
                ..active(7, 125 * m, 150 * m, "", "")
            },
        ];
        let f = Fixture::new(blocks, 0, 86_400, Tz::Fixed(0));
        let sessions = focus_sessions(&f.data(), &FocusParams::default());
        assert_eq!(sessions.len(), 3);
        assert_eq!((sessions[0].start, sessions[0].end), (0, 36 * m));
        assert_eq!(sessions[0].productive, 35 * m);
        assert!(!sessions[0].recorded);
        assert_eq!((sessions[1].start, sessions[1].end), (120 * m, 170 * m));
        assert!(sessions[2].recorded);
        assert_eq!(sessions[2].block, 7);
        assert_eq!(sessions[2].productive, 25 * m);
        // Stricter parameters.
        let strict = FocusParams {
            min_secs: 40 * m,
            max_interruption_secs: 30,
        };
        let sessions = focus_sessions(&f.data(), &strict);
        assert_eq!(sessions.iter().filter(|s| !s.recorded).count(), 1);
    }

    #[test]
    fn daily_splits_at_local_midnight() {
        // UTC+2: local midnight is 22:00 UTC.
        let tz = Tz::Fixed(2 * 3600);
        let day = days_from_civil(2026, 11, 10);
        let (a, b) = tz.day_bounds(day);
        let blocks = vec![
            // 23:00 - 01:30 local: one hour today, 90 minutes tomorrow.
            active(1, b - H, b + H + H / 2, "code.exe", "late"),
            active(2, a + 9 * H, a + 10 * H, "vlc.exe", "film"),
            away(3, b - 2 * H, b - H, AwayLabel::None),
        ];
        let f = Fixture::new(blocks, a, tz.day_start(day + 2), tz);
        let rows = daily(&f.data(), day - 1, day + 2);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].worked, 0);
        assert_eq!(rows[1].worked, 2 * H);
        assert_eq!(rows[1].away, H);
        assert_eq!(rows[2].worked, H + H / 2);
        assert_eq!(rows[3].worked, 0);
        assert_eq!(rows[1].first, Some(a + 9 * H));
        assert_eq!(rows[1].last, Some(b));
        assert_eq!(rows[2].first, Some(b));
        let dev = f.c.category_index("development").unwrap();
        assert_eq!(rows[1].by_category[dev], H);
        assert_eq!(rows[1].by_score, [H, 0, H]);
        assert_eq!(rows[1].score(), 50);
        assert_eq!(rows.iter().map(|r| r.worked).sum::<i64>(), worked(&f.spans));
        for r in &rows {
            assert_eq!(r.by_category.iter().sum::<i64>(), r.worked);
            assert_eq!(r.by_score.iter().sum::<i64>(), r.worked);
        }
    }

    #[test]
    fn dst_days_keep_their_real_length() {
        // Central European 2026: forward 29 March 01:00 UTC, back 25 October 01:00 UTC.
        let fwd = days_from_civil(2026, 3, 29) as i64 * 86_400 + 3600;
        let back = days_from_civil(2026, 10, 25) as i64 * 86_400 + 3600;
        let tz = Tz::with_dst(3600, 7200, fwd, back);
        for (y, m, dom, hours) in [(2026, 3, 29, 23), (2026, 10, 25, 25)] {
            let day = days_from_civil(y, m, dom);
            let (a, b) = tz.day_bounds(day);
            assert_eq!(b - a, hours * H);
            // One block from the evening before to the morning after.
            let blocks = vec![active(1, a - 2 * H, b + 3 * H, "code.exe", "all night")];
            let f = Fixture::new(blocks, a - 2 * H, b + 3 * H, tz.clone());
            let rows = daily(&f.data(), day - 1, day + 1);
            assert_eq!(rows[0].worked, 2 * H);
            assert_eq!(rows[1].worked, hours * H);
            assert_eq!(rows[2].worked, 3 * H);
            // The hour matrix holds every second once.
            let m = hour_matrix(&f.data());
            let total: i64 = m.worked.iter().flat_map(|r| r.iter()).sum();
            assert_eq!(total, (hours + 5) * H);
            // On the 25-hour day one wall-clock hour is lived twice.
            let weekday = crate::timeutil::weekday(day) as usize;
            let max = m.worked[weekday].iter().copied().max().unwrap();
            assert_eq!(max, if hours == 25 { 2 * H } else { H });
        }
    }

    #[test]
    fn hours() {
        let f = sample_day();
        let m = hour_matrix(&f.data());
        // 10 November 2026 is a Tuesday.
        assert_eq!(m.worked[1][9], H);
        assert_eq!(m.worked[1][11], H);
        assert_eq!(m.productive[1][11], 0);
        assert_eq!(m.worked[1][12], 0);
        assert_eq!(m.worked[1][15], H / 2);
        assert_eq!(m.by_hour().iter().sum::<i64>(), worked(&f.spans));
        assert_eq!(m.max(), H);
        // 9, 10 and 14 are all fully productive: the earliest wins.
        assert_eq!(m.best_hour(), Some(9));
    }

    #[test]
    fn trends() {
        let row = |day: Day, worked: i64, productive: i64| DayRow {
            day,
            worked,
            away: 0,
            by_category: vec![productive, worked - productive, 0],
            by_score: [productive, 0, worked - productive],
            first: None,
            last: None,
        };
        let prev = vec![row(0, 4 * H, 2 * H), row(1, 0, 0), row(2, 4 * H, 4 * H)];
        let cur = vec![row(3, 6 * H, 6 * H), row(4, 3 * H, 3 * H), row(5, 3 * H, 0)];
        let c = compare(&cur, &prev);
        assert_eq!(c.worked.diff(), 4 * H);
        assert_eq!(c.worked.pct(), Some(50.0));
        assert_eq!(
            (c.prev.days, c.prev.days_with_data, c.cur.days_with_data),
            (3, 2, 3)
        );
        assert_eq!(c.avg_active_day.prev, 4 * H);
        assert_eq!(c.avg_active_day.cur, 4 * H);
        assert_eq!(c.avg_day.prev, 8 * H / 3);
        assert_eq!(c.score.cur, 75);
        assert_eq!(c.score.prev, 75);
        assert_eq!(c.categories.len(), 2);
        assert_eq!(
            c.categories[0],
            (
                0,
                Delta {
                    cur: 9 * H,
                    prev: 6 * H
                }
            )
        );
        assert_eq!(c.categories[1].1.pct(), Some(50.0));
        // Nothing before: no percentage.
        let c = compare(&cur, &[]);
        assert_eq!(c.worked.pct(), None);
        assert_eq!(c.worked.diff(), 12 * H);
        assert_eq!(previous_period(10, 16), (3, 9));
        assert_eq!(previous_period(10, 10), (9, 9));
    }

    #[test]
    fn day_pieces() {
        let tz = Tz::Fixed(0);
        let mut got = Vec::new();
        for_each_day(&tz, 86_400 - 10, 3 * 86_400 + 5, |d, s, e| {
            got.push((d, e - s))
        });
        assert_eq!(got, vec![(0, 10), (1, 86_400), (2, 86_400), (3, 5)]);
        got.clear();
        for_each_day(&tz, 5, 5, |d, s, e| got.push((d, e - s)));
        assert!(got.is_empty());
        for_each_day(&tz, 86_400 - 10, 86_400, |d, s, e| got.push((d, e - s)));
        assert_eq!(got, vec![(0, 10)]);
    }
}
