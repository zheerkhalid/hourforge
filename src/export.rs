//! Reports and timesheets as CSV, JSON and HTML, and the command-line export.
//!
//! Everything here is pure: it turns blocks and settings into text. The HTML
//! is one self-contained file with inline styles and inline SVG; it has no
//! scripts and refers to nothing outside itself.
//!
//! CSV files start with a UTF-8 byte-order mark and use CRLF line ends, so
//! that spreadsheet programs open titles in any script correctly. Text cells
//! that begin with `=`, `+`, `-` or `@` get a leading apostrophe, so a window
//! title can never be run as a formula.

use crate::model::{AwayLabel, Block, Kind, Rounding, Score};
use crate::rules::Classifier;
use crate::settings::Settings;
use crate::spans::{self, Span};
use crate::timeutil::{
    self, civil_from_days, days_from_civil, days_in_month, fmt_day, fmt_dur, fmt_hours, iso_date,
    month_start, parse_iso_date, week_start, Day, Tz, MONTHS_LONG,
};
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Csv,
    Json,
    Html,
}

impl Format {
    pub fn parse(text: &str) -> Option<Format> {
        match text.trim().to_lowercase().as_str() {
            "csv" => Some(Format::Csv),
            "json" => Some(Format::Json),
            "html" | "htm" => Some(Format::Html),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Html => "html",
        }
    }
}

/// Everything an export needs. `blocks` must cover `[from, to)` (UTC).
pub struct Input<'a> {
    pub blocks: &'a [Block],
    pub settings: &'a Settings,
    pub tz: &'a Tz,
    pub from: i64,
    pub to: i64,
    /// Shown in headings, e.g. `Week of 9 Nov 2026`.
    pub label: &'a str,
}

// ---------------------------------------------------------------------------
// Roll-up shared by the three report formats
// ---------------------------------------------------------------------------

struct DayRow {
    day: Day,
    worked: i64,
    away: i64,
    /// Worked seconds per category index.
    by_category: Vec<(usize, i64)>,
}

struct Summary<'a> {
    classifier: Classifier,
    /// Worked pieces, in time order.
    entries: Vec<(Span, &'a Block)>,
    worked: i64,
    away: i64,
    productive: i64,
    distracting: i64,
    by_category: Vec<(usize, i64)>,
    /// (name, exe, seconds)
    by_app: Vec<(String, String, i64)>,
    by_project: Vec<(usize, i64)>,
    no_project: i64,
    by_site: Vec<(String, i64)>,
    days: Vec<DayRow>,
}

/// Calls `f(day, seconds)` for every local day a stretch of time touches.
fn split_days(start: i64, end: i64, tz: &Tz, mut f: impl FnMut(Day, i64)) {
    let mut cursor = start;
    while cursor < end {
        let day = tz.day_of(cursor);
        let next = tz.day_start(day + 1);
        let piece_end = if next > cursor { end.min(next) } else { end };
        f(day, piece_end - cursor);
        cursor = piece_end;
    }
}

fn sorted_desc<K: Ord + Clone>(map: HashMap<K, i64>) -> Vec<(K, i64)> {
    let mut v: Vec<(K, i64)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

fn summarize<'a>(input: &Input<'a>) -> Summary<'a> {
    let classifier = Classifier::new(input.settings);
    let all = spans::flatten(input.blocks, input.from, input.to, &classifier);
    let mut by_category: HashMap<usize, i64> = HashMap::new();
    let mut by_app: HashMap<(String, String), i64> = HashMap::new();
    let mut by_project: HashMap<usize, i64> = HashMap::new();
    let mut by_site: HashMap<String, i64> = HashMap::new();
    let mut day_worked: HashMap<Day, i64> = HashMap::new();
    let mut day_away: HashMap<Day, i64> = HashMap::new();
    let mut day_category: HashMap<(Day, usize), i64> = HashMap::new();
    let mut entries = Vec::new();
    let (mut productive, mut distracting, mut no_project) = (0, 0, 0);

    for span in &all {
        let block = &input.blocks[span.src as usize];
        if !span.work {
            if span.kind == Kind::Away {
                split_days(span.start, span.end, input.tz, |d, s| {
                    *day_away.entry(d).or_default() += s;
                });
            }
            continue;
        }
        let secs = span.secs();
        let category = span.category as usize;
        *by_category.entry(category).or_default() += secs;
        match classifier.score(category) {
            Score::Productive => productive += secs,
            Score::Distracting => distracting += secs,
            Score::Neutral => {}
        }
        *by_app
            .entry((block.app_label().to_string(), block.exe.clone()))
            .or_default() += secs;
        match span.project {
            Some(p) => *by_project.entry(p as usize).or_default() += secs,
            None => no_project += secs,
        }
        if !block.domain.is_empty() {
            *by_site.entry(block.domain.clone()).or_default() += secs;
        }
        split_days(span.start, span.end, input.tz, |d, s| {
            *day_worked.entry(d).or_default() += s;
            *day_category.entry((d, category)).or_default() += s;
        });
        entries.push((*span, block));
    }

    // Every day of the range gets a row, unless the range is very long.
    let mut day_list: Vec<Day> = Vec::new();
    if input.to > input.from {
        let (first, last) = (input.tz.day_of(input.from), input.tz.day_of(input.to - 1));
        if (last as i64 - first as i64) < 400 {
            day_list.extend(first..=last);
        }
    }
    if day_list.is_empty() {
        day_list = day_worked.keys().chain(day_away.keys()).copied().collect();
        day_list.sort_unstable();
        day_list.dedup();
    }
    let days = day_list
        .into_iter()
        .map(|day| {
            let mut cats: Vec<(usize, i64)> = day_category
                .iter()
                .filter(|((d, _), _)| *d == day)
                .map(|((_, c), s)| (*c, *s))
                .collect();
            cats.sort_unstable();
            DayRow {
                day,
                worked: day_worked.get(&day).copied().unwrap_or(0),
                away: day_away.get(&day).copied().unwrap_or(0),
                by_category: cats,
            }
        })
        .collect();

    Summary {
        worked: spans::worked(&all),
        away: spans::away(&all),
        productive,
        distracting,
        by_category: sorted_desc(by_category),
        by_app: sorted_desc(by_app)
            .into_iter()
            .map(|((name, exe), s)| (name, exe, s))
            .collect(),
        by_project: sorted_desc(by_project),
        no_project,
        by_site: sorted_desc(by_site),
        days,
        entries,
        classifier,
    }
}

fn kind_name(b: &Block) -> &'static str {
    match (b.kind, b.away) {
        (Kind::Active, _) => "tracked",
        (Kind::Manual, _) => "manual",
        (Kind::Away, AwayLabel::Meeting) => "meeting",
        (Kind::Away, AwayLabel::Call) => "call",
        (Kind::Away, AwayLabel::Break) => "break",
        (Kind::Away, AwayLabel::None) => "away",
        (Kind::Timer, _) => "timer",
        (Kind::Focus, _) => "focus",
    }
}

fn hm(secs: i64) -> String {
    let s = secs.max(0);
    format!("{}:{:02}", s / 3600, s / 60 % 60)
}

fn time_of_day(utc: i64, tz: &Tz) -> String {
    let s = tz.second_of_day(utc);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

const BOM: &str = "\u{feff}";

/// A text cell: guarded against formula injection, quoted when needed.
pub fn csv_cell(text: &str) -> String {
    let risky = text.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let text = if risky {
        format!("'{text}")
    } else {
        text.to_string()
    };
    if text.contains([',', '"', '\n', '\r']) || text.starts_with(' ') || text.ends_with(' ') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn csv_row(out: &mut String, cells: &[String]) {
    out.push_str(&cells.join(","));
    out.push_str("\r\n");
}

fn report_csv(input: &Input, sum: &Summary) -> String {
    let mut out = String::from(BOM);
    out.push_str(
        "date,start,end,seconds,duration,kind,app,exe,title,domain,category,project,note,tags\r\n",
    );
    for (span, block) in &sum.entries {
        let category = &sum.classifier.categories[span.category as usize].name;
        let project = span
            .project
            .map(|p| sum.classifier.projects[p as usize].name.as_str())
            .unwrap_or("");
        csv_row(
            &mut out,
            &[
                iso_date(input.tz.day_of(span.start)),
                time_of_day(span.start, input.tz),
                time_of_day(span.end, input.tz),
                span.secs().to_string(),
                hm(span.secs()),
                kind_name(block).to_string(),
                csv_cell(block.app_label()),
                csv_cell(&block.exe),
                csv_cell(&block.title),
                csv_cell(&block.domain),
                csv_cell(category),
                csv_cell(project),
                csv_cell(&block.note),
                csv_cell(&block.tags.join("; ")),
            ],
        );
    }
    out
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn report_json(input: &Input, sum: &Summary) -> String {
    use serde_json::json;
    let c = &sum.classifier;
    let entries: Vec<serde_json::Value> = sum
        .entries
        .iter()
        .map(|(span, b)| {
            json!({
                "start": timeutil::fmt_datetime(span.start, input.tz),
                "end": timeutil::fmt_datetime(span.end, input.tz),
                "start_utc": span.start,
                "end_utc": span.end,
                "seconds": span.secs(),
                "kind": kind_name(b),
                "app": b.app_label(),
                "exe": b.exe,
                "title": b.title,
                "domain": b.domain,
                "category": c.categories[span.category as usize].name,
                "project": span.project.map(|p| c.projects[p as usize].name.clone()),
                "note": b.note,
                "tags": b.tags,
            })
        })
        .collect();
    let value = json!({
        "generator": "HourForge",
        "version": env!("CARGO_PKG_VERSION"),
        "range": {
            "label": input.label,
            "from": timeutil::fmt_datetime(input.from, input.tz),
            "to": timeutil::fmt_datetime(input.to, input.tz),
            "from_utc": input.from,
            "to_utc": input.to,
        },
        "totals": {
            "worked_seconds": sum.worked,
            "away_seconds": sum.away,
            "productive_seconds": sum.productive,
            "distracting_seconds": sum.distracting,
            "days_with_activity": sum.days.iter().filter(|d| d.worked > 0).count(),
        },
        "by_category": sum.by_category.iter().map(|(i, s)| json!({
            "id": c.categories[*i].id,
            "name": c.categories[*i].name,
            "score": c.categories[*i].score.name().to_lowercase(),
            "seconds": s,
        })).collect::<Vec<_>>(),
        "by_app": sum.by_app.iter().map(|(name, exe, s)| json!({
            "app": name, "exe": exe, "seconds": s,
        })).collect::<Vec<_>>(),
        "by_project": sum.by_project.iter().map(|(i, s)| json!({
            "id": c.projects[*i].id,
            "name": c.projects[*i].name,
            "client": c.projects[*i].client,
            "seconds": s,
        })).collect::<Vec<_>>(),
        "no_project_seconds": sum.no_project,
        "by_site": sum.by_site.iter().map(|(d, s)| json!({
            "domain": d, "seconds": s,
        })).collect::<Vec<_>>(),
        "by_day": sum.days.iter().map(|d| json!({
            "date": iso_date(d.day),
            "worked_seconds": d.worked,
            "away_seconds": d.away,
        })).collect::<Vec<_>>(),
        "entries": entries,
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

/// Escapes text for HTML content and attribute values.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

const STYLE: &str = "\
:root{--bg:#f4f5f9;--card:#fff;--text:#171a26;--muted:#575e72;--line:#dadde7;--accent:#c8560b;--track:#eceef4}
@media (prefers-color-scheme:dark){:root{--bg:#11121a;--card:#181a25;--text:#eceef5;--muted:#a0a6b8;--line:#2a2e3f;--accent:#ff9a4a;--track:#1f2230}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--text);font:15px/1.45 'Segoe UI',system-ui,sans-serif}
main{max-width:980px;margin:0 auto;padding:32px 20px 48px}
h1{font-size:26px;margin:0 0 2px}
h2{font-size:16px;margin:0 0 12px}
.sub{color:var(--muted);margin:0 0 22px}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px;margin-bottom:16px}
.tile,.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:16px}
.tile b{display:block;font-size:24px;font-weight:600}
.tile span{color:var(--muted);font-size:13px}
.card{margin-bottom:16px;break-inside:avoid}
table{width:100%;border-collapse:collapse}
th{text-align:left;color:var(--muted);font-weight:400;font-size:13px;padding:4px 8px 6px 0;border-bottom:1px solid var(--line)}
td{padding:6px 8px 6px 0;border-bottom:1px solid var(--line);vertical-align:middle}
tr:last-child td{border-bottom:0}
td.n,th.n{text-align:right;white-space:nowrap;font-variant-numeric:tabular-nums}
td.bar{width:34%}
td.bar i{display:block;height:8px;border-radius:4px;background:var(--track);overflow:hidden}
td.bar i b{display:block;height:8px;border-radius:4px}
.dot{display:inline-block;width:10px;height:10px;border-radius:50%;margin-right:8px}
.muted{color:var(--muted)}
svg{width:100%;height:auto;display:block}
svg text{fill:var(--muted);font-size:11px}
svg .axis{stroke:var(--line)}
tfoot td{font-weight:600;border-top:1px solid var(--line);border-bottom:0}
footer{color:var(--muted);font-size:12.5px;margin-top:24px}
@media print{body{background:#fff}main{padding:0}.tile,.card{border-color:#ccc}}
";

fn html_head(title: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>\n{STYLE}</style>\n</head>\n<body>\n<main>\n",
        html_escape(title)
    )
}

const HTML_FOOT: &str = "<footer>Made by HourForge on this PC. This file contains no scripts and loads nothing from anywhere.</footer>\n</main>\n</body>\n</html>\n";

fn tile(out: &mut String, value: &str, caption: &str) {
    let _ = writeln!(
        out,
        "<div class=\"tile\"><b>{}</b><span>{}</span></div>",
        html_escape(value),
        html_escape(caption)
    );
}

fn share(part: i64, whole: i64) -> f64 {
    if whole > 0 {
        (part as f64 * 100.0 / whole as f64).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// One table of "name, bar, time, share" rows.
fn bar_table(
    out: &mut String,
    title: &str,
    head: &str,
    rows: &[(String, String, i64)],
    total: i64,
) {
    if rows.is_empty() {
        return;
    }
    let top = rows.iter().map(|r| r.2).max().unwrap_or(1).max(1);
    let _ = writeln!(
        out,
        "<section class=\"card\"><h2>{}</h2><table><thead><tr><th>{}</th><th></th><th class=\"n\">Time</th><th class=\"n\">Share</th></tr></thead><tbody>",
        html_escape(title),
        html_escape(head)
    );
    for (name, color, secs) in rows {
        let _ = writeln!(
            out,
            "<tr><td><span class=\"dot\" style=\"background:{color}\"></span>{}</td><td class=\"bar\"><i><b style=\"width:{:.1}%;background:{color}\"></b></i></td><td class=\"n\">{}</td><td class=\"n\">{:.0}%</td></tr>",
            html_escape(name),
            share(*secs, top),
            fmt_dur(*secs),
            share(*secs, total)
        );
    }
    out.push_str("</tbody></table></section>\n");
}

fn day_chart(out: &mut String, sum: &Summary) {
    let days = &sum.days;
    if days.is_empty() || sum.worked == 0 {
        return;
    }
    let (w, h, left, bottom, top) = (920.0_f64, 220.0_f64, 40.0_f64, 24.0_f64, 10.0_f64);
    let max = days.iter().map(|d| d.worked).max().unwrap_or(1).max(3600);
    // Round the scale up to whole hours.
    let max_hours = (max as f64 / 3600.0).ceil().max(1.0);
    let plot_h = h - bottom - top;
    let slot = (w - left) / days.len() as f64;
    let bar_w = (slot * 0.7).clamp(2.0, 46.0);
    out.push_str("<section class=\"card\"><h2>Worked time per day</h2>\n");
    let _ = writeln!(
        out,
        "<svg viewBox=\"0 0 {w} {h}\" role=\"img\" aria-label=\"Worked time per day\">"
    );
    let lines = 4;
    for i in 0..=lines {
        let y = top + plot_h - plot_h * i as f64 / lines as f64;
        let hours = max_hours * i as f64 / lines as f64;
        let _ = writeln!(
            out,
            "<line class=\"axis\" x1=\"{left}\" y1=\"{y:.1}\" x2=\"{w}\" y2=\"{y:.1}\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}h</text>",
            left - 6.0,
            y + 4.0,
            if hours.fract() == 0.0 { format!("{hours:.0}") } else { format!("{hours:.1}") }
        );
    }
    let label_every = (days.len() as f64 / 16.0).ceil().max(1.0) as usize;
    for (i, d) in days.iter().enumerate() {
        let x = left + slot * i as f64 + (slot - bar_w) / 2.0;
        let mut y = top + plot_h;
        for (cat, secs) in &d.by_category {
            let bh = plot_h * (*secs as f64 / 3600.0) / max_hours;
            y -= bh;
            let _ = writeln!(
                out,
                "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{bar_w:.1}\" height=\"{bh:.1}\" fill=\"{}\"><title>{}: {} {}</title></rect>",
                hex(sum.classifier.categories[*cat].color),
                html_escape(&fmt_day(d.day)),
                html_escape(&sum.classifier.categories[*cat].name),
                fmt_dur(*secs)
            );
        }
        if i % label_every == 0 {
            let (_, m, dom) = civil_from_days(d.day);
            let _ = writeln!(
                out,
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{dom}/{m}</text>",
                x + bar_w / 2.0,
                h - 6.0
            );
        }
    }
    out.push_str("</svg></section>\n");
}

fn report_html(input: &Input, sum: &Summary) -> String {
    let c = &sum.classifier;
    let mut out = html_head(&format!("HourForge report: {}", input.label));
    let _ = writeln!(
        out,
        "<h1>{}</h1>\n<p class=\"sub\">Activity report, {} to {}</p>",
        html_escape(input.label),
        html_escape(&timeutil::fmt_datetime(input.from, input.tz)),
        html_escape(&timeutil::fmt_datetime(input.to, input.tz))
    );
    let active_days = sum.days.iter().filter(|d| d.worked > 0).count() as i64;
    out.push_str("<div class=\"tiles\">\n");
    tile(&mut out, &fmt_dur(sum.worked), "Worked");
    tile(
        &mut out,
        &fmt_dur(if active_days > 0 {
            sum.worked / active_days
        } else {
            0
        }),
        &format!(
            "Average over {active_days} active day{}",
            if active_days == 1 { "" } else { "s" }
        ),
    );
    tile(
        &mut out,
        &format!("{:.0}%", share(sum.productive, sum.worked)),
        "Productive share",
    );
    tile(&mut out, &fmt_dur(sum.away), "Away");
    out.push_str("</div>\n");

    day_chart(&mut out, sum);

    let cats: Vec<(String, String, i64)> = sum
        .by_category
        .iter()
        .map(|(i, s)| {
            (
                c.categories[*i].name.clone(),
                hex(c.categories[*i].color),
                *s,
            )
        })
        .collect();
    bar_table(&mut out, "Categories", "Category", &cats, sum.worked);

    let mut projects: Vec<(String, String, i64)> = sum
        .by_project
        .iter()
        .map(|(i, s)| {
            let p = &c.projects[*i];
            let name = if p.client.is_empty() {
                p.name.clone()
            } else {
                format!("{} ({})", p.name, p.client)
            };
            (name, hex(p.color), *s)
        })
        .collect();
    if !projects.is_empty() && sum.no_project > 0 {
        projects.push(("No project".into(), "#8b92a5".into(), sum.no_project));
    }
    bar_table(&mut out, "Projects", "Project", &projects, sum.worked);

    let apps: Vec<(String, String, i64)> = sum
        .by_app
        .iter()
        .take(25)
        .map(|(name, _, s)| (name.clone(), "#6c7dff".to_string(), *s))
        .collect();
    bar_table(&mut out, "Programs", "Program", &apps, sum.worked);

    let sites: Vec<(String, String, i64)> = sum
        .by_site
        .iter()
        .take(25)
        .map(|(d, s)| (d.clone(), "#22c3a6".to_string(), *s))
        .collect();
    bar_table(&mut out, "Websites", "Website", &sites, sum.worked);

    if sum.worked == 0 {
        out.push_str("<section class=\"card\"><p class=\"muted\">Nothing was recorded in this range.</p></section>\n");
    }
    out.push_str(HTML_FOOT);
    out
}

/// The activity report for the range in the asked format.
pub fn report(input: &Input, format: Format) -> String {
    let sum = summarize(input);
    match format {
        Format::Csv => report_csv(input, &sum),
        Format::Json => report_json(input, &sum),
        Format::Html => report_html(input, &sum),
    }
}

// ---------------------------------------------------------------------------
// Timesheets
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
pub struct TimesheetRow {
    pub day: Day,
    /// Project id and name.
    pub project: String,
    pub project_name: String,
    pub client: String,
    pub secs: i64,
    pub rounded_secs: i64,
    pub rate: f64,
    pub amount: f64,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Timesheet {
    pub rows: Vec<TimesheetRow>,
    pub total_secs: i64,
    pub total_rounded_secs: i64,
    pub total_amount: f64,
}

/// Rounds a duration to a multiple of `step_minutes`.
///
/// Nearest rounds half up. Zero stays zero; rounding up never turns worked
/// time into nothing.
pub fn round_secs(secs: i64, rounding: Rounding, step_minutes: u32) -> i64 {
    let step = step_minutes as i64 * 60;
    if secs <= 0 {
        return 0;
    }
    if step <= 0 {
        return secs;
    }
    match rounding {
        Rounding::None => secs,
        Rounding::Down => secs / step * step,
        Rounding::Up => (secs + step - 1) / step * step,
        Rounding::Nearest => (secs + step / 2) / step * step,
    }
}

/// One row per day and project, rounded by `settings.timesheet`.
/// `project`: only this project id; `None` = every project.
pub fn timesheet(input: &Input, project: Option<&str>) -> Timesheet {
    let classifier = Classifier::new(input.settings);
    let all = spans::flatten(input.blocks, input.from, input.to, &classifier);
    let mut cells: HashMap<(Day, usize), i64> = HashMap::new();
    for span in all.iter().filter(|s| s.work) {
        let Some(p) = span.project else { continue };
        let p = p as usize;
        if project.is_some_and(|id| classifier.projects[p].id != id) {
            continue;
        }
        split_days(span.start, span.end, input.tz, |day, secs| {
            *cells.entry((day, p)).or_default() += secs;
        });
    }
    let mut keys: Vec<(Day, usize)> = cells.keys().copied().collect();
    keys.sort_unstable();
    let ts = &input.settings.timesheet;
    let mut sheet = Timesheet::default();
    for key in keys {
        let secs = cells[&key];
        let p = &classifier.projects[key.1];
        let rounded_secs = round_secs(secs, ts.rounding, ts.step_minutes);
        let amount = rounded_secs as f64 / 3600.0 * p.rate;
        sheet.total_secs += secs;
        sheet.total_rounded_secs += rounded_secs;
        sheet.total_amount += amount;
        sheet.rows.push(TimesheetRow {
            day: key.0,
            project: p.id.clone(),
            project_name: p.name.clone(),
            client: p.client.clone(),
            secs,
            rounded_secs,
            rate: p.rate,
            amount,
        });
    }
    sheet
}

fn rounding_note(settings: &Settings) -> String {
    let ts = &settings.timesheet;
    match ts.rounding {
        Rounding::None => "Durations are not rounded.".to_string(),
        r => format!(
            "Each day's time per project: {} {} minutes.",
            match r {
                Rounding::Up => "rounded up to",
                Rounding::Down => "rounded down to",
                _ => "rounded to the nearest",
            },
            ts.step_minutes
        ),
    }
}

pub fn timesheet_text(sheet: &Timesheet, input: &Input, format: Format) -> String {
    let currency = input.settings.timesheet.currency.as_str();
    match format {
        Format::Csv => {
            let mut out = String::from(BOM);
            out.push_str("date,project,client,hours,rounded_hours,rate,amount,currency\r\n");
            for r in &sheet.rows {
                csv_row(
                    &mut out,
                    &[
                        iso_date(r.day),
                        csv_cell(&r.project_name),
                        csv_cell(&r.client),
                        fmt_hours(r.secs),
                        fmt_hours(r.rounded_secs),
                        format!("{:.2}", r.rate),
                        format!("{:.2}", r.amount),
                        csv_cell(currency),
                    ],
                );
            }
            csv_row(
                &mut out,
                &[
                    "Total".into(),
                    String::new(),
                    String::new(),
                    fmt_hours(sheet.total_secs),
                    fmt_hours(sheet.total_rounded_secs),
                    String::new(),
                    format!("{:.2}", sheet.total_amount),
                    csv_cell(currency),
                ],
            );
            out
        }
        Format::Json => {
            use serde_json::json;
            let ts = &input.settings.timesheet;
            let value = json!({
                "generator": "HourForge",
                "label": input.label,
                "currency": currency,
                "rounding": ts.rounding.name(),
                "step_minutes": ts.step_minutes,
                "rows": sheet.rows.iter().map(|r| json!({
                    "date": iso_date(r.day),
                    "project_id": r.project,
                    "project": r.project_name,
                    "client": r.client,
                    "seconds": r.secs,
                    "rounded_seconds": r.rounded_secs,
                    "hours": r.rounded_secs as f64 / 3600.0,
                    "rate": r.rate,
                    "amount": (r.amount * 100.0).round() / 100.0,
                })).collect::<Vec<_>>(),
                "totals": {
                    "seconds": sheet.total_secs,
                    "rounded_seconds": sheet.total_rounded_secs,
                    "hours": sheet.total_rounded_secs as f64 / 3600.0,
                    "amount": (sheet.total_amount * 100.0).round() / 100.0,
                },
            });
            serde_json::to_string_pretty(&value).unwrap_or_default()
        }
        Format::Html => {
            let mut out = html_head(&format!("HourForge timesheet: {}", input.label));
            let _ = writeln!(
                out,
                "<h1>Timesheet</h1>\n<p class=\"sub\">{}</p>",
                html_escape(input.label)
            );
            out.push_str("<div class=\"tiles\">\n");
            tile(&mut out, &fmt_hours(sheet.total_rounded_secs), "Hours");
            tile(
                &mut out,
                &format!("{:.2} {currency}", sheet.total_amount),
                "Amount",
            );
            tile(
                &mut out,
                &fmt_dur(sheet.total_secs),
                "Tracked before rounding",
            );
            out.push_str("</div>\n<section class=\"card\"><table><thead><tr><th>Date</th><th>Project</th><th>Client</th><th class=\"n\">Tracked</th><th class=\"n\">Hours</th><th class=\"n\">Rate</th><th class=\"n\">Amount</th></tr></thead><tbody>\n");
            for r in &sheet.rows {
                let _ = writeln!(
                    out,
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{:.2}</td><td class=\"n\">{:.2}</td></tr>",
                    html_escape(&fmt_day(r.day)),
                    html_escape(&r.project_name),
                    html_escape(&r.client),
                    hm(r.secs),
                    fmt_hours(r.rounded_secs),
                    r.rate,
                    r.amount
                );
            }
            if sheet.rows.is_empty() {
                out.push_str("<tr><td colspan=\"7\" class=\"muted\">No project time in this range.</td></tr>\n");
            }
            let _ = writeln!(
                out,
                "</tbody><tfoot><tr><td>Total</td><td></td><td></td><td class=\"n\">{}</td><td class=\"n\">{}</td><td></td><td class=\"n\">{:.2} {}</td></tr></tfoot></table></section>",
                hm(sheet.total_secs),
                fmt_hours(sheet.total_rounded_secs),
                sheet.total_amount,
                html_escape(currency)
            );
            let _ = writeln!(
                out,
                "<p class=\"muted\">{}</p>",
                html_escape(&rounding_note(input.settings))
            );
            out.push_str(HTML_FOOT);
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Ranges and file names
// ---------------------------------------------------------------------------

/// A heading for a range of local days (both included).
pub fn range_label(from: Day, to: Day) -> String {
    if from == to {
        fmt_day(from)
    } else {
        format!("{} to {}", iso_date(from), iso_date(to))
    }
}

/// A named or explicit range for the command line: `today`, `yesterday`,
/// `week`, `last-week`, `month`, `last-month`, `2026-11-12`,
/// `2026-11-01..2026-11-12` (both days included). Returns the UTC range and a
/// label.
///
/// `week` and `month` run from the start of the current week or month
/// through today (whole days). Also understood: `year`, `last-7-days`,
/// `last-30-days`.
pub fn parse_range(
    spec: &str,
    now: i64,
    tz: &Tz,
    monday_first: bool,
) -> Option<(i64, i64, String)> {
    let today = tz.day_of(now);
    let spec = spec.trim().to_lowercase().replace('_', "-");
    let month_label = |day: Day| {
        let (y, m, _) = civil_from_days(day);
        format!("{} {y}", MONTHS_LONG[m as usize - 1])
    };
    let week_label = |day: Day| {
        let (y, m, d) = civil_from_days(day);
        format!("Week of {d} {} {y}", timeutil::MONTHS[m as usize - 1])
    };
    let (from, to, label) = match spec.as_str() {
        "today" => (today, today, fmt_day(today)),
        "yesterday" => (today - 1, today - 1, fmt_day(today - 1)),
        "week" | "this-week" => {
            let start = week_start(today, monday_first);
            (start, today, week_label(start))
        }
        "last-week" => {
            let start = week_start(today, monday_first) - 7;
            (start, start + 6, week_label(start))
        }
        "month" | "this-month" => {
            let start = month_start(today);
            (start, today, month_label(start))
        }
        "last-month" => {
            let end = month_start(today) - 1;
            let start = month_start(end);
            (start, end, month_label(start))
        }
        "year" | "this-year" => {
            let (y, _, _) = civil_from_days(today);
            (days_from_civil(y, 1, 1), today, format!("Year {y}"))
        }
        "last-7-days" | "7d" => (today - 6, today, "Last 7 days".to_string()),
        "last-30-days" | "30d" => (today - 29, today, "Last 30 days".to_string()),
        other => {
            if let Some((a, b)) = other.split_once("..") {
                let (a, b) = (parse_iso_date(a)?, parse_iso_date(b)?);
                if b < a {
                    return None;
                }
                // A whole calendar month reads better by name.
                let (y, m, d) = civil_from_days(a);
                let whole_month = d == 1 && b == a + days_in_month(y, m) as Day - 1;
                let label = if whole_month {
                    month_label(a)
                } else {
                    range_label(a, b)
                };
                (a, b, label)
            } else {
                let day = parse_iso_date(other)?;
                (day, day, fmt_day(day))
            }
        }
    };
    Some((tz.day_start(from), tz.day_start(to + 1), label))
}

/// A safe file name such as `HourForge-report-Week-of-9-Nov-2026.csv`.
pub fn default_file_name(kind: &str, label: &str, format: Format) -> String {
    let clean = |text: &str| {
        let mut out = String::new();
        for c in text.chars() {
            if c.is_alphanumeric() {
                out.push(c);
            } else if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
        let trimmed = out.trim_end_matches('-');
        trimmed.chars().take(60).collect::<String>()
    };
    let mut name = String::from("HourForge");
    for part in [clean(kind), clean(label)] {
        if !part.is_empty() {
            name.push('-');
            name.push_str(part.trim_end_matches('-'));
        }
    }
    format!("{name}.{}", format.extension())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    fn tz() -> Tz {
        Tz::Fixed(3600)
    }

    fn day() -> Day {
        days_from_civil(2026, 11, 12)
    }

    /// Local wall-clock time on the test day, as UTC seconds.
    fn at(d: Day, h: u32, m: u32) -> i64 {
        tz().at(d, h, m)
    }

    fn settings() -> Settings {
        let mut s = Settings::default();
        s.projects.push(Project {
            id: "harbor".into(),
            name: "Harbor redesign".into(),
            client: "Northwind, Ltd".into(),
            color: [0x4f, 0x8c, 0xff],
            rate: 60.0,
            keywords: vec!["harbor".into()],
            archived: false,
        });
        s.projects.push(Project {
            id: "atlas".into(),
            name: "Atlas".into(),
            client: String::new(),
            color: [0x22, 0xc3, 0xa6],
            rate: 0.0,
            keywords: vec!["atlas".into()],
            archived: false,
        });
        s
    }

    fn active(id: u64, start: i64, end: i64, exe: &str, title: &str, domain: &str) -> Block {
        Block {
            id,
            start,
            end,
            exe: exe.into(),
            app: exe.trim_end_matches(".exe").to_string(),
            title: title.into(),
            domain: domain.into(),
            ..Default::default()
        }
    }

    fn sample_blocks() -> Vec<Block> {
        let d = day();
        let mut meeting = active(5, at(d, 13, 0), at(d, 13, 30), "", "", "");
        meeting.kind = Kind::Away;
        meeting.away = AwayLabel::Meeting;
        let mut lunch = active(6, at(d, 12, 0), at(d, 13, 0), "", "", "");
        lunch.kind = Kind::Away;
        let mut manual = active(
            7,
            at(d - 1, 18, 0),
            at(d - 1, 19, 10),
            "",
            "Site visit, harbor",
            "",
        );
        manual.kind = Kind::Manual;
        manual.note = "on foot".into();
        manual.tags = vec!["offline".into(), "billable".into()];
        let mut timer = active(8, at(d, 9, 0), at(d, 9, 30), "", "Atlas planning", "");
        timer.kind = Kind::Timer;
        timer.project = Some("atlas".into());
        vec![
            active(1, at(d, 9, 0), at(d, 10, 0), "code.exe", "plan.md", ""),
            active(
                2,
                at(d, 10, 0),
                at(d, 10, 40),
                "code.exe",
                "pier.rs - harbor",
                "",
            ),
            active(
                3,
                at(d, 10, 40),
                at(d, 11, 0),
                "chrome.exe",
                "=SUM(1,2) \"quoted\"",
                "videos.example",
            ),
            active(
                4,
                at(d, 11, 0),
                at(d, 11, 7),
                "chrome.exe",
                "Pull requests",
                "github.com",
            ),
            meeting,
            lunch,
            manual,
            timer,
        ]
    }

    fn input<'a>(blocks: &'a [Block], settings: &'a Settings, tz: &'a Tz) -> Input<'a> {
        Input {
            blocks,
            settings,
            tz,
            from: tz.day_start(day() - 1),
            to: tz.day_start(day() + 1),
            label: "11 Nov to 12 Nov 2026",
        }
    }

    #[test]
    fn formats() {
        assert_eq!(Format::parse(" CSV "), Some(Format::Csv));
        assert_eq!(Format::parse("htm"), Some(Format::Html));
        assert_eq!(Format::parse("json"), Some(Format::Json));
        assert_eq!(Format::parse("pdf"), None);
        assert_eq!(Format::Html.extension(), "html");
    }

    #[test]
    fn rounding_table() {
        use Rounding::*;
        let m = 60;
        let cases = [
            // (seconds, rounding, step minutes, expected seconds)
            (0, Up, 15, 0),
            (0, Nearest, 15, 0),
            (-5, Up, 15, 0),
            (1, Up, 15, 15 * m),
            (15 * m, Up, 15, 15 * m),
            (15 * m + 1, Up, 15, 30 * m),
            (1, Down, 15, 0),
            (29 * m + 59, Down, 15, 15 * m),
            (30 * m, Down, 15, 30 * m),
            (7 * m + 29, Nearest, 15, 0),
            (7 * m + 30, Nearest, 15, 15 * m),
            (22 * m + 29, Nearest, 15, 15 * m),
            (22 * m + 30, Nearest, 15, 30 * m),
            (3 * m, Nearest, 6, 6 * m),
            (2 * m + 59, Nearest, 6, 0),
            (61 * m, Up, 60, 120 * m),
            (61 * m, Nearest, 60, 60 * m),
            (1234, None, 15, 1234),
            (1234, Up, 0, 1234),
            (59, Up, 1, 60),
            (61, Down, 1, 60),
        ];
        for (secs, rounding, step, want) in cases {
            assert_eq!(
                round_secs(secs, rounding, step),
                want,
                "{secs} {rounding:?} {step}"
            );
        }
    }

    #[test]
    fn csv_cells_are_quoted_and_guarded() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell(""), "");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_cell(" padded "), "\" padded \"");
        assert_eq!(csv_cell("=1+1"), "'=1+1");
        assert_eq!(csv_cell("+44 20 7946 0000"), "'+44 20 7946 0000");
        assert_eq!(csv_cell("-draft-"), "'-draft-");
        assert_eq!(csv_cell("@handle"), "'@handle");
        assert_eq!(
            csv_cell("=HYPERLINK(\"x\",\"y\")"),
            "\"'=HYPERLINK(\"\"x\"\",\"\"y\"\")\""
        );
        assert_eq!(csv_cell("a=b"), "a=b");
    }

    /// A minimal CSV reader for the tests (quotes, doubled quotes, CRLF).
    fn parse_csv(text: &str) -> Vec<Vec<String>> {
        let text = text.strip_prefix(BOM).unwrap_or(text);
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut cell = String::new();
        let mut quoted = false;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if quoted {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        cell.push('"');
                    } else {
                        quoted = false;
                    }
                } else {
                    cell.push(c);
                }
            } else {
                match c {
                    '"' => quoted = true,
                    ',' => row.push(std::mem::take(&mut cell)),
                    '\r' => {}
                    '\n' => {
                        row.push(std::mem::take(&mut cell));
                        rows.push(std::mem::take(&mut row));
                    }
                    _ => cell.push(c),
                }
            }
        }
        rows
    }

    #[test]
    fn csv_report_rows_and_totals() {
        let (blocks, s, tz) = (sample_blocks(), settings(), tz());
        let inp = input(&blocks, &s, &tz);
        let text = report(&inp, Format::Csv);
        assert!(text.starts_with(BOM));
        let rows = parse_csv(&text);
        assert_eq!(rows[0].len(), 14);
        assert_eq!(rows[0][0], "date");
        assert!(rows.iter().all(|r| r.len() == 14));
        // Totals equal what the rest of the program computes.
        let classifier = Classifier::new(&s);
        let spans = spans::flatten(&blocks, inp.from, inp.to, &classifier);
        let total: i64 = rows[1..].iter().map(|r| r[3].parse::<i64>().unwrap()).sum();
        assert_eq!(total, spans::worked(&spans));
        assert_eq!(total, (70 + 60 + 40 + 20 + 7 + 30) * 60);
        // The manual entry of the day before comes first.
        let first = &rows[1];
        assert_eq!(first[0], "2026-11-11");
        assert_eq!(
            (first[1].as_str(), first[2].as_str()),
            ("18:00:00", "19:10:00")
        );
        assert_eq!((first[4].as_str(), first[5].as_str()), ("1:10", "manual"));
        assert_eq!(first[8], "Site visit, harbor");
        assert_eq!(first[11], "Harbor redesign");
        assert_eq!(
            (first[12].as_str(), first[13].as_str()),
            ("on foot", "offline; billable")
        );
        // The timer claims its half hour of the first block for its project.
        assert_eq!(rows[2][11], "Atlas");
        assert_eq!(rows[2][3], "1800");
        assert_eq!(rows[3][11], "");
        // The formula-like title was defused and survived quoting.
        let risky = rows.iter().find(|r| r[9] == "videos.example").unwrap();
        assert_eq!(risky[8], "'=SUM(1,2) \"quoted\"");
        assert_eq!(risky[10], "Browsing");
        let git = rows.iter().find(|r| r[9] == "github.com").unwrap();
        assert_eq!(git[10], "Development");
        let meeting = rows.iter().find(|r| r[5] == "meeting").unwrap();
        assert_eq!(
            (meeting[6].as_str(), meeting[10].as_str()),
            ("Meeting", "Communication")
        );
        // Plain away time is not a row.
        assert!(rows.iter().all(|r| r[5] != "away"));
    }

    #[test]
    fn json_report_parses_back() {
        let (blocks, s, tz) = (sample_blocks(), settings(), tz());
        let inp = input(&blocks, &s, &tz);
        let v: serde_json::Value = serde_json::from_str(&report(&inp, Format::Json)).unwrap();
        let worked = (70 + 60 + 40 + 20 + 7 + 30) * 60;
        assert_eq!(v["totals"]["worked_seconds"], worked);
        assert_eq!(v["totals"]["away_seconds"], 3600);
        assert_eq!(v["totals"]["days_with_activity"], 2);
        assert_eq!(v["range"]["label"], "11 Nov to 12 Nov 2026");
        assert_eq!(v["range"]["from"], "2026-11-11 00:00:00");
        let sum = |key: &str| -> i64 {
            v[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["seconds"].as_i64().unwrap())
                .sum()
        };
        assert_eq!(sum("by_category"), worked);
        assert_eq!(sum("by_app"), worked);
        assert_eq!(sum("entries"), worked);
        assert_eq!(
            sum("by_project") + v["no_project_seconds"].as_i64().unwrap(),
            worked
        );
        assert_eq!(sum("by_site"), 27 * 60);
        let days = v["by_day"].as_array().unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[0]["date"], "2026-11-11");
        assert_eq!(days[0]["worked_seconds"], 70 * 60);
        assert_eq!(days[1]["away_seconds"], 3600);
        // Biggest first.
        assert_eq!(v["by_category"][0]["id"], "development");
        assert_eq!(v["by_app"][0]["exe"], "code.exe");
        assert_eq!(v["entries"][0]["tags"][1], "billable");
        // An empty range is still valid JSON.
        let empty = Input {
            from: 10,
            to: 10,
            ..input(&[], &s, &tz)
        };
        let v: serde_json::Value = serde_json::from_str(&report(&empty, Format::Json)).unwrap();
        assert_eq!(v["totals"]["worked_seconds"], 0);
        assert_eq!(v["entries"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn html_report_is_self_contained_and_escaped() {
        let (mut blocks, s, tz) = (sample_blocks(), settings(), tz());
        blocks.push(active(
            20,
            at(day(), 15, 0),
            at(day(), 15, 30),
            "odd.exe",
            "t",
            "<script>alert(1)</script>.example",
        ));
        blocks[8].app = "R&D <Tool>".into();
        let inp = input(&blocks, &s, &tz);
        let html = report(&inp, Format::Html);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.trim_end().ends_with("</html>"));
        // Nothing that could load from, or link to, anywhere.
        let lower = html.to_lowercase();
        for banned in ["http", "<script", "src=", "href=", "@import", "url("] {
            assert!(!lower.contains(banned), "{banned}");
        }
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;.example"));
        assert!(html.contains("R&amp;D &lt;Tool&gt;"));
        assert!(html.contains("Harbor redesign (Northwind, Ltd)"));
        assert!(html.contains("<svg"));
        assert!(html.contains("Development"));
        assert!(html.contains("Worked time per day"));
        // An empty range still gives a page.
        let empty = Input {
            from: 10,
            to: 10,
            ..input(&[], &s, &tz)
        };
        let html = report(&empty, Format::Html);
        assert!(html.contains("Nothing was recorded"));
        assert!(!html.contains("<svg"));
        assert_eq!(
            html_escape("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn timesheet_rows_rounding_and_rates() {
        let (blocks, mut s, tz) = (sample_blocks(), settings(), tz());
        let inp_settings = s.clone();
        let inp = input(&blocks, &inp_settings, &tz);
        // Default: nearest 15 minutes.
        let sheet = timesheet(&inp, None);
        let rows: Vec<(String, &str, i64, i64)> = sheet
            .rows
            .iter()
            .map(|r| {
                (
                    iso_date(r.day),
                    r.project.as_str(),
                    r.secs / 60,
                    r.rounded_secs / 60,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("2026-11-11".to_string(), "harbor", 70, 75),
                ("2026-11-12".to_string(), "harbor", 40, 45),
                ("2026-11-12".to_string(), "atlas", 30, 30),
            ]
        );
        assert_eq!(sheet.total_secs, 140 * 60);
        assert_eq!(sheet.total_rounded_secs, 150 * 60);
        // 2 hours of Harbor at 60 an hour; Atlas is not billed.
        assert!((sheet.total_amount - 120.0).abs() < 1e-9);
        assert_eq!(sheet.rows[0].client, "Northwind, Ltd");
        assert!((sheet.rows[0].amount - 75.0).abs() < 1e-9);
        assert_eq!(sheet.rows[2].amount, 0.0);
        // One project only.
        let only = timesheet(&inp, Some("atlas"));
        assert_eq!(only.rows.len(), 1);
        assert_eq!(only.total_rounded_secs, 30 * 60);
        assert!(timesheet(&inp, Some("nowhere")).rows.is_empty());
        // Round up to the hour.
        s.timesheet.rounding = Rounding::Up;
        s.timesheet.step_minutes = 60;
        let up = timesheet(&input(&blocks, &s, &tz), None);
        assert_eq!(up.total_rounded_secs, (120 + 60 + 60) * 60);
        assert!((up.total_amount - 180.0).abs() < 1e-9);
        // No rounding.
        s.timesheet.rounding = Rounding::None;
        let exact = timesheet(&input(&blocks, &s, &tz), None);
        assert_eq!(exact.total_rounded_secs, exact.total_secs);
    }

    #[test]
    fn timesheet_splits_days_at_local_midnight() {
        let (s, tz) = (settings(), tz());
        let d = day();
        let mut late = active(
            1,
            at(d, 23, 0),
            at(d, 23, 59) + 60 + 3600,
            "",
            "harbor night shift",
            "",
        );
        late.kind = Kind::Manual;
        let blocks = vec![late];
        let inp = Input {
            from: tz.day_start(d),
            to: tz.day_start(d + 2),
            ..input(&blocks, &s, &tz)
        };
        let sheet = timesheet(&inp, None);
        assert_eq!(sheet.rows.len(), 2);
        assert_eq!((sheet.rows[0].day, sheet.rows[0].secs), (d, 3600));
        assert_eq!((sheet.rows[1].day, sheet.rows[1].secs), (d + 1, 3600));
    }

    #[test]
    fn timesheet_text_formats() {
        let (blocks, s, tz) = (sample_blocks(), settings(), tz());
        let inp = input(&blocks, &s, &tz);
        let sheet = timesheet(&inp, None);
        let csv = timesheet_text(&sheet, &inp, Format::Csv);
        let rows = parse_csv(&csv);
        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows[1],
            vec![
                "2026-11-11",
                "Harbor redesign",
                "Northwind, Ltd",
                "1.17",
                "1.25",
                "60.00",
                "75.00",
                "USD"
            ]
        );
        assert_eq!(rows[4][0], "Total");
        assert_eq!(
            (rows[4][4].as_str(), rows[4][6].as_str()),
            ("2.50", "120.00")
        );
        let v: serde_json::Value =
            serde_json::from_str(&timesheet_text(&sheet, &inp, Format::Json)).unwrap();
        assert_eq!(v["rows"].as_array().unwrap().len(), 3);
        assert_eq!(v["totals"]["rounded_seconds"], 150 * 60);
        assert_eq!(v["totals"]["amount"], 120.0);
        assert_eq!(v["step_minutes"], 15);
        let html = timesheet_text(&sheet, &inp, Format::Html);
        assert!(!html.to_lowercase().contains("http"));
        assert!(html.contains("120.00 USD"));
        assert!(html.contains("rounded to the nearest 15 minutes"));
        assert!(html.contains("Northwind, Ltd"));
        let none = timesheet_text(&Timesheet::default(), &inp, Format::Html);
        assert!(none.contains("No project time"));
    }

    #[test]
    fn ranges() {
        let tz = tz();
        // Thursday 12 November 2026, mid-afternoon.
        let now = at(day(), 16, 42);
        let days = |spec: &str, monday: bool| {
            parse_range(spec, now, &tz, monday)
                .map(|(a, b, label)| (iso_date(tz.day_of(a)), iso_date(tz.day_of(b - 1)), label))
        };
        let r = |a: &str, b: &str, l: &str| Some((a.to_string(), b.to_string(), l.to_string()));
        assert_eq!(
            days("today", true),
            r("2026-11-12", "2026-11-12", "Thu 12 Nov 2026")
        );
        assert_eq!(
            days("Yesterday", true),
            r("2026-11-11", "2026-11-11", "Wed 11 Nov 2026")
        );
        assert_eq!(
            days("week", true),
            r("2026-11-09", "2026-11-12", "Week of 9 Nov 2026")
        );
        assert_eq!(
            days("week", false),
            r("2026-11-08", "2026-11-12", "Week of 8 Nov 2026")
        );
        assert_eq!(
            days("last-week", true),
            r("2026-11-02", "2026-11-08", "Week of 2 Nov 2026")
        );
        assert_eq!(
            days("last_week", false),
            r("2026-11-01", "2026-11-07", "Week of 1 Nov 2026")
        );
        assert_eq!(
            days("month", true),
            r("2026-11-01", "2026-11-12", "November 2026")
        );
        assert_eq!(
            days("last-month", true),
            r("2026-10-01", "2026-10-31", "October 2026")
        );
        assert_eq!(
            days("year", true),
            r("2026-01-01", "2026-11-12", "Year 2026")
        );
        assert_eq!(
            days("last-7-days", true),
            r("2026-11-06", "2026-11-12", "Last 7 days")
        );
        assert_eq!(
            days("2026-02-28", true),
            r("2026-02-28", "2026-02-28", "Sat 28 Feb 2026")
        );
        assert_eq!(
            days("2026-11-01..2026-11-12", true),
            r("2026-11-01", "2026-11-12", "2026-11-01 to 2026-11-12")
        );
        assert_eq!(
            days("2024-02-01..2024-02-29", true),
            r("2024-02-01", "2024-02-29", "February 2024")
        );
        assert_eq!(days("2026-11-12..2026-11-01", true), None);
        assert_eq!(days("2026-02-30", true), None);
        assert_eq!(days("someday", true), None);
        assert_eq!(days("", true), None);
        // Exact UTC edges: local midnight to local midnight.
        let (a, b, _) = parse_range("today", now, &tz, true).unwrap();
        assert_eq!((a, b), tz.day_bounds(day()));
        // Month and year edges.
        let jan1 = tz.at(days_from_civil(2027, 1, 1), 0, 30);
        let d = |spec: &str| {
            parse_range(spec, jan1, &tz, true)
                .map(|(a, b, label)| (iso_date(tz.day_of(a)), iso_date(tz.day_of(b - 1)), label))
        };
        assert_eq!(
            d("last-month"),
            r("2026-12-01", "2026-12-31", "December 2026")
        );
        assert_eq!(d("month"), r("2027-01-01", "2027-01-01", "January 2027"));
        assert_eq!(
            d("week"),
            r("2026-12-28", "2027-01-01", "Week of 28 Dec 2026")
        );
        assert_eq!(
            d("yesterday"),
            r("2026-12-31", "2026-12-31", "Thu 31 Dec 2026")
        );
        let mar1 = tz.at(days_from_civil(2024, 3, 1), 9, 0);
        let (a, b, label) = parse_range("last-month", mar1, &tz, true).unwrap();
        assert_eq!(b - a, 29 * 86_400);
        assert_eq!(label, "February 2024");
        assert_eq!(range_label(day(), day()), "Thu 12 Nov 2026");
        assert_eq!(range_label(day() - 1, day()), "2026-11-11 to 2026-11-12");
    }

    #[test]
    fn ranges_follow_daylight_saving() {
        let fwd = days_from_civil(2026, 3, 29) as i64 * 86_400 + 3600;
        let back = days_from_civil(2026, 10, 25) as i64 * 86_400 + 3600;
        let tz = Tz::with_dst(3600, 7200, fwd, back);
        let (a, b, _) = parse_range("2026-03-29", 0, &tz, true).unwrap();
        assert_eq!(b - a, 23 * 3600);
        let (a, b, _) = parse_range("2026-10-25", 0, &tz, true).unwrap();
        assert_eq!(b - a, 25 * 3600);
    }

    #[test]
    fn file_names() {
        assert_eq!(
            default_file_name("report", "Week of 9 Nov 2026", Format::Csv),
            "HourForge-report-Week-of-9-Nov-2026.csv"
        );
        assert_eq!(
            default_file_name("timesheet", "2026-11-01 to 2026-11-12", Format::Html),
            "HourForge-timesheet-2026-11-01-to-2026-11-12.html"
        );
        assert_eq!(
            default_file_name("report", "a/b\\c:d*e?\"<>| ..", Format::Json),
            "HourForge-report-a-b-c-d-e.json"
        );
        assert_eq!(default_file_name("", "", Format::Csv), "HourForge.csv");
        let long = default_file_name("report", &"x".repeat(500), Format::Csv);
        assert!(long.len() < 90);
    }
}
