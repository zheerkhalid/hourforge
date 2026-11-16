//! The Reports page: a day, week, month or custom range, with totals and
//! trends against the period before, time per day, a breakdown that drills
//! down from category to program to window title to the raw timeline, a
//! calendar of the last six months and the hours of the week.

use super::app::{page_header, Action, Cx};
use super::charts::{self, DeltaUnit, RankOptions, GAP};
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::aggregate::{self, Comparison, Data, DayRow, Filter, GroupBy, HourMatrix, RawRow, Row};
use crate::model::{Block, Score};
use crate::spans::{flatten, Span};
use crate::timeutil::{self, Day};
use eframe::egui::{
    self, pos2, vec2, Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, Ui,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Mode {
    Day,
    #[default]
    Week,
    Month,
    Custom,
}

/// One level of the drill-down path.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Step {
    Category(usize, String),
    Project(Option<usize>, String),
    App(String, String),
    Site(String),
    Title(String),
}

impl Step {
    fn name(&self) -> &str {
        match self {
            Step::Category(_, n) | Step::Project(_, n) | Step::App(_, n) => n,
            Step::Site(n) | Step::Title(n) => n,
        }
    }
}

/// What the breakdown card lists at the current drill-down level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Group(GroupBy),
    Raw,
}

struct Cache {
    from: Day,
    to: Day,
    rev: u64,
    minute: i64,
    /// Bumped on every recomputation; the breakdown list depends on it.
    generation: u64,
    blocks: Vec<Block>,
    spans: Vec<Span>,
    rows: Vec<DayRow>,
    cmp: Comparison,
    hours: HourMatrix,
    /// The last day that is not in the future.
    last_day: Day,
    prev: (Day, Day),
}

enum Listing {
    Rows(Vec<Row>),
    Raw(Vec<RawRow>),
}

struct ListCache {
    generation: u64,
    group: GroupBy,
    path: Vec<Step>,
    listing: Listing,
}

struct HeatCache {
    today: Day,
    rev: u64,
    monday_first: bool,
    days: Vec<(Day, i64)>,
}

pub struct ReportsView {
    mode: Mode,
    /// A day inside the shown period; `None` = today.
    anchor: Option<Day>,
    custom: Option<(Day, Day)>,
    from_text: String,
    to_text: String,
    date_error: bool,
    group: GroupBy,
    path: Vec<Step>,
    cache: Option<Cache>,
    list: Option<ListCache>,
    heat: Option<HeatCache>,
    generation: u64,
    pending: Option<String>,
    /// Frames for which the page is held at its lower end (screenshots).
    hold_bottom: u32,
    hold_offset: f32,
}

impl Default for ReportsView {
    fn default() -> Self {
        ReportsView {
            mode: Mode::Week,
            anchor: None,
            custom: None,
            from_text: String::new(),
            to_text: String::new(),
            date_error: false,
            group: GroupBy::Category,
            path: Vec::new(),
            cache: None,
            list: None,
            heat: None,
            generation: 0,
            pending: None,
            hold_bottom: 0,
            hold_offset: 0.0,
        }
    }
}

fn range_label(from: Day, to: Day) -> String {
    if from == to {
        return timeutil::fmt_day(from);
    }
    let (y0, _, _) = timeutil::civil_from_days(from);
    let (y1, _, _) = timeutil::civil_from_days(to);
    if y0 == y1 {
        format!(
            "{} – {} {y1}",
            timeutil::fmt_day_short(from),
            timeutil::fmt_day_short(to)
        )
    } else {
        format!(
            "{} {y0} – {} {y1}",
            timeutil::fmt_day_short(from),
            timeutil::fmt_day_short(to)
        )
    }
}

impl ReportsView {
    /// Opens a sub-state named on the command line: `reports` (this week),
    /// `reports-day`, `reports-month`, `reports-custom`, `reports-drill` (a
    /// drill-down two levels deep), `reports-heatmap` (the lower half of the
    /// page: calendar and hours).
    pub fn apply_view(&mut self, view: &str) {
        match view {
            "reports-day" => self.mode = Mode::Day,
            "reports-month" | "reports-heatmap" => self.mode = Mode::Month,
            "reports-custom" => self.mode = Mode::Custom,
            _ => self.mode = Mode::Week,
        }
        if view == "reports-heatmap" {
            self.hold_bottom = 30;
            self.hold_offset = 100_000.0;
        }
        if view == "reports-drill" {
            self.hold_bottom = 30;
            self.hold_offset = 310.0;
        }
        self.pending = Some(view.to_string());
        self.path.clear();
    }

    /// Shows this range of local days (both included).
    pub fn open_range(&mut self, from: Day, to: Day) {
        let (from, to) = (from.min(to), from.max(to));
        self.path.clear();
        if from == to {
            self.mode = Mode::Day;
            self.anchor = Some(from);
        } else {
            self.mode = Mode::Custom;
            self.custom = Some((from, to));
            self.from_text = timeutil::iso_date(from);
            self.to_text = timeutil::iso_date(to);
            self.date_error = false;
        }
    }

    fn range(&self, cx: &Cx) -> (Day, Day) {
        let anchor = self.anchor.unwrap_or(cx.today);
        match self.mode {
            Mode::Day => (anchor, anchor),
            Mode::Week => {
                let start = timeutil::week_start(anchor, cx.settings.week_starts_monday);
                (start, start + 6)
            }
            Mode::Month => {
                let start = timeutil::month_start(anchor);
                let (y, m, _) = timeutil::civil_from_days(start);
                (start, start + timeutil::days_in_month(y, m) as Day - 1)
            }
            Mode::Custom => self.custom.unwrap_or((cx.today - 29, cx.today)),
        }
    }

    fn step(&mut self, cx: &Cx, forward: bool) {
        let (from, to) = self.range(cx);
        let sign = if forward { 1 } else { -1 };
        match self.mode {
            Mode::Day => self.anchor = Some(from + sign),
            Mode::Week => self.anchor = Some(from + 7 * sign),
            Mode::Month => self.anchor = Some(if forward { to + 1 } else { from - 1 }),
            Mode::Custom => {
                let len = to - from + 1;
                let (f, t) = (from + len * sign, to + len * sign);
                self.custom = Some((f, t));
                self.from_text = timeutil::iso_date(f);
                self.to_text = timeutil::iso_date(t);
            }
        }
        if self.anchor == Some(cx.today) {
            self.anchor = None;
        }
        self.path.clear();
    }

    /// The period to compare with: the same days one period earlier.
    fn previous(&self, from: Day, to: Day, last_day: Day) -> (Day, Day) {
        match self.mode {
            Mode::Week => (from - 7, last_day - 7),
            Mode::Month => {
                let prev_from = timeutil::month_start(from - 1);
                (prev_from, (prev_from + (last_day - from)).min(from - 1))
            }
            _ => aggregate::previous_period(from, to),
        }
    }

    fn load(&mut self, cx: &Cx) {
        let (from, to) = self.range(cx);
        let touches_today = from <= cx.today && to >= cx.today;
        let minute = if touches_today { cx.now / 60 } else { 0 };
        if let Some(c) = &self.cache {
            if c.from == from && c.to == to && c.rev == cx.rev && c.minute == minute {
                return;
            }
        }
        // Days that have not happened yet are shown, but not averaged over.
        let last_day = if from <= cx.today {
            to.min(cx.today)
        } else {
            to
        };
        let (a, b) = (cx.tz.day_start(from), cx.tz.day_start(to + 1));
        let blocks = cx.blocks(a, b);
        let spans = flatten(&blocks, a, b, cx.classifier);
        let d = Data {
            blocks: &blocks,
            spans: &spans,
            classifier: cx.classifier,
            tz: cx.tz,
        };
        let rows = aggregate::daily(&d, from, to);
        let hours = aggregate::hour_matrix(&d);
        let prev = self.previous(from, to, last_day);
        let (pa, pb) = (cx.tz.day_start(prev.0), cx.tz.day_start(prev.1 + 1));
        let prev_blocks = cx.blocks(pa, pb);
        let prev_spans = flatten(&prev_blocks, pa, pb, cx.classifier);
        let prev_rows = aggregate::daily(
            &Data {
                blocks: &prev_blocks,
                spans: &prev_spans,
                classifier: cx.classifier,
                tz: cx.tz,
            },
            prev.0,
            prev.1,
        );
        let counted = &rows[..((last_day - from + 1).max(0) as usize).min(rows.len())];
        let cmp = aggregate::compare(counted, &prev_rows);
        self.generation += 1;
        self.cache = Some(Cache {
            from,
            to,
            rev: cx.rev,
            minute,
            generation: self.generation,
            blocks,
            spans,
            rows,
            cmp,
            hours,
            last_day,
            prev,
        });
    }

    fn load_heat(&mut self, cx: &Cx) {
        let monday_first = cx.settings.week_starts_monday;
        if let Some(h) = &self.heat {
            if h.today == cx.today && h.rev == cx.rev && h.monday_first == monday_first {
                return;
            }
        }
        // 26 weeks, ending with the current one.
        let from = timeutil::week_start(cx.today, monday_first) - 25 * 7;
        let (a, b) = (cx.tz.day_start(from), cx.tz.day_start(cx.today + 1));
        let blocks = cx.blocks(a, b);
        let spans = flatten(&blocks, a, b, cx.classifier);
        let rows = aggregate::daily(
            &Data {
                blocks: &blocks,
                spans: &spans,
                classifier: cx.classifier,
                tz: cx.tz,
            },
            from,
            cx.today,
        );
        self.heat = Some(HeatCache {
            today: cx.today,
            rev: cx.rev,
            monday_first,
            days: rows.iter().map(|r| (r.day, r.worked)).collect(),
        });
    }

    fn filter(&self) -> Filter {
        let mut f = Filter::default();
        for step in &self.path {
            match step {
                Step::Category(c, _) => f.category = Some(*c),
                Step::Project(p, _) => f.project = Some(*p),
                Step::App(key, _) => f.app = Some(key.clone()),
                Step::Site(s) => f.site = Some(s.clone()),
                Step::Title(t) => f.title = Some(t.clone()),
            }
        }
        f
    }

    fn level(&self) -> Level {
        match self.path.last() {
            None => Level::Group(self.group),
            Some(Step::Category(..)) | Some(Step::Project(..)) => Level::Group(GroupBy::App),
            Some(Step::App(..)) | Some(Step::Site(..)) => Level::Group(GroupBy::Title),
            Some(Step::Title(..)) => Level::Raw,
        }
    }

    fn load_list(&mut self, cx: &Cx) {
        let Some(cache) = &self.cache else { return };
        if let Some(l) = &self.list {
            if l.generation == cache.generation && l.group == self.group && l.path == self.path {
                return;
            }
        }
        let d = Data {
            blocks: &cache.blocks,
            spans: &cache.spans,
            classifier: cx.classifier,
            tz: cx.tz,
        };
        let filter = self.filter();
        let listing = match self.level() {
            Level::Group(by) => Listing::Rows(aggregate::ranked(&d, by, &filter)),
            Level::Raw => Listing::Raw(aggregate::raw(&d, &filter)),
        };
        self.list = Some(ListCache {
            generation: cache.generation,
            group: self.group,
            path: self.path.clone(),
            listing,
        });
    }

    /// For the `reports-drill` screenshot: opens the biggest category and the
    /// biggest program in it.
    fn auto_drill(&mut self, cx: &Cx) {
        let Some(cache) = &self.cache else { return };
        let d = Data {
            blocks: &cache.blocks,
            spans: &cache.spans,
            classifier: cx.classifier,
            tz: cx.tz,
        };
        self.group = GroupBy::Category;
        self.path.clear();
        let cats = aggregate::ranked(&d, GroupBy::Category, &Filter::default());
        let Some(cat) = cats.first() else { return };
        let Ok(index) = cat.key.parse::<usize>() else {
            return;
        };
        self.path.push(Step::Category(index, cat.name.clone()));
        let apps = aggregate::ranked(&d, GroupBy::App, &self.filter());
        if let Some(app) = apps.first() {
            self.path.push(Step::App(app.key.clone(), app.name.clone()));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if self.mode == Mode::Custom && self.custom.is_none() {
            let (from, to) = (cx.today - 29, cx.today);
            self.custom = Some((from, to));
            self.from_text = timeutil::iso_date(from);
            self.to_text = timeutil::iso_date(to);
        }
        let (from, to) = self.range(cx);

        // Header: range picker and export.
        let mut new_mode = None;
        let mut step = None;
        let subtitle = match self.mode {
            Mode::Month => {
                let (y, m, _) = timeutil::civil_from_days(from);
                format!("{} {y}", timeutil::MONTHS_LONG[m as usize - 1])
            }
            _ => range_label(from, to),
        };
        let current = match self.mode {
            Mode::Day => 0,
            Mode::Week => 1,
            Mode::Month => 2,
            Mode::Custom => 3,
        };
        page_header(ui, "Reports", &subtitle, |ui| {
            if widgets::ghost_button(ui, &Icon::EXPORT.with("Export")).clicked() {
                cx.actions.push(Action::Export { from, to });
            }
            ui.add_space(6.0);
            if widgets::icon_button(ui, Icon::CHEVRON_RIGHT, "Next period").clicked() {
                step = Some(true);
            }
            if widgets::icon_button(ui, Icon::BACK, "Previous period").clicked() {
                step = Some(false);
            }
            ui.add_space(6.0);
            if let Some(i) = widgets::segmented(ui, &["Day", "Week", "Month", "Custom"], current) {
                new_mode = Some([Mode::Day, Mode::Week, Mode::Month, Mode::Custom][i]);
            }
        });
        if let Some(mode) = new_mode {
            if mode == Mode::Custom {
                self.custom = Some((from, to));
                self.from_text = timeutil::iso_date(from);
                self.to_text = timeutil::iso_date(to);
                self.date_error = false;
            } else if self.mode == Mode::Custom {
                self.anchor = None;
            }
            self.mode = mode;
            self.path.clear();
            return;
        }
        if let Some(forward) = step {
            self.step(cx, forward);
            return;
        }
        if self.mode == Mode::Custom {
            self.custom_row(ui);
        }

        self.load(cx);
        if self.pending.take().as_deref() == Some("reports-drill") {
            self.auto_drill(cx);
        }
        self.load_list(cx);
        self.load_heat(cx);

        // The caches are taken out while the page is drawn, so that clicks
        // can change the view state, and put back afterwards.
        let (Some(cache), Some(list), Some(heat)) =
            (self.cache.take(), self.list.take(), self.heat.take())
        else {
            return;
        };
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if self.hold_bottom > 0 {
            self.hold_bottom -= 1;
            scroll = scroll.vertical_scroll_offset(self.hold_offset);
        }
        scroll.show(ui, |ui| {
            let width = ui.available_width();
            summary_card(ui, cx, &cache);
            ui.add_space(GAP - 8.0);
            if cache.from != cache.to {
                self.days_card(ui, cx, &cache);
                ui.add_space(GAP - 8.0);
            }
            let left = ((width - GAP) * 0.58).floor();
            let right = width - GAP - left;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                charts::card_sized(ui, left, 410.0, |ui| {
                    self.breakdown_card(ui, cx, &cache, &list);
                });
                charts::card_sized(ui, right, 410.0, |ui| compare_card(ui, cx, &cache));
            });
            ui.add_space(GAP - 8.0);
            // Calendar: 26 weeks of 17 px plus labels and the card's padding.
            let calendar = (26.0 * 17.0 + 32.0 + 34.0 + 8.0_f32).min((width - GAP) * 0.56);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                charts::card_sized(ui, calendar, 236.0, |ui| {
                    charts::card_title(ui, "Last six months", "click a day to open it");
                    ui.add_space(2.0);
                    let clicked = charts::calendar_heatmap(
                        ui,
                        &heat.days,
                        cx.settings.week_starts_monday,
                        cx.today,
                    );
                    if let Some(day) = clicked {
                        self.mode = Mode::Day;
                        self.anchor = Some(day).filter(|d| *d != cx.today);
                        self.path.clear();
                    }
                });
                charts::card_sized(ui, width - GAP - calendar, 236.0, |ui| {
                    let remark = match cache.hours.best_hour() {
                        Some(h) => format!("most productive around {h:02}:00"),
                        None => String::new(),
                    };
                    charts::card_title(ui, "Hours of the week", &remark);
                    ui.add_space(2.0);
                    charts::hour_heatmap(ui, &cache.hours, cx.settings.week_starts_monday);
                });
            });
            ui.add_space(8.0);
        });
        self.cache = Some(cache);
        self.list = Some(list);
        self.heat = Some(heat);
    }

    fn custom_row(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let mut apply = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("From").color(p.muted));
            let a = ui.add(
                egui::TextEdit::singleline(&mut self.from_text)
                    .hint_text("YYYY-MM-DD")
                    .desired_width(104.0),
            );
            ui.label(RichText::new("to").color(p.muted));
            let b = ui.add(
                egui::TextEdit::singleline(&mut self.to_text)
                    .hint_text("YYYY-MM-DD")
                    .desired_width(104.0),
            );
            let enter =
                (a.lost_focus() || b.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if widgets::ghost_button(ui, "Show").clicked() || enter {
                apply = true;
            }
            if self.date_error {
                ui.label(
                    RichText::new(
                        "Dates look like 2026-11-12, and the range may span at most two years.",
                    )
                    .color(p.danger),
                );
            }
        });
        ui.add_space(4.0);
        if apply {
            let parsed = (
                timeutil::parse_iso_date(&self.from_text),
                timeutil::parse_iso_date(&self.to_text),
            );
            match parsed {
                (Some(a), Some(b)) if (a - b).abs() <= 731 => {
                    self.custom = Some((a.min(b), a.max(b)));
                    self.from_text = timeutil::iso_date(a.min(b));
                    self.to_text = timeutil::iso_date(a.max(b));
                    self.date_error = false;
                    self.path.clear();
                }
                _ => self.date_error = true,
            }
        }
    }

    fn days_card(&mut self, ui: &mut Ui, cx: &mut Cx, cache: &Cache) {
        charts::card(ui, |ui| {
            charts::card_title(ui, "Time per day", "by category  ·  click a day to open it");
            let colors: Vec<Color32> = cx
                .classifier
                .categories
                .iter()
                .map(|c| widgets::rgb(c.color))
                .collect();
            let names: Vec<String> = cx
                .classifier
                .categories
                .iter()
                .map(|c| c.name.clone())
                .collect();
            let clicked = charts::stacked_days(ui, &cache.rows, &colors, &names, 190.0, cx.today);
            if let Some(day) = clicked {
                self.mode = Mode::Day;
                self.anchor = Some(day).filter(|d| *d != cx.today);
                self.path.clear();
            }
            let items: Vec<(Color32, String)> = cache
                .cmp
                .categories
                .iter()
                .filter(|(_, d)| d.cur > 0)
                .map(|(c, _)| (colors[*c], names[*c].clone()))
                .collect();
            charts::legend(ui, &items);
        });
    }

    fn breakdown_card(&mut self, ui: &mut Ui, cx: &mut Cx, cache: &Cache, list: &ListCache) {
        let p = pal(ui);
        // Title row: what is listed, and how the top level is grouped.
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 34.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(RichText::new("Breakdown").font(semibold(15.0)));
                ui.label(RichText::new("by").size(12.5).color(p.faint));
                let current = GroupBy::ALL
                    .iter()
                    .position(|g| *g == self.group)
                    .unwrap_or(0);
                let names: Vec<&str> = GroupBy::ALL.iter().map(|g| g.name()).collect();
                if let Some(i) = widgets::segmented(ui, &names, current) {
                    self.group = GroupBy::ALL[i];
                    self.path.clear();
                }
            },
        );
        // Breadcrumb.
        let mut cut_to = None;
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 24.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let crumb = |ui: &mut Ui, text: &str, last: bool| -> bool {
                    let shown: String = if text.chars().count() > 30 {
                        format!("{}…", text.chars().take(29).collect::<String>())
                    } else {
                        text.to_string()
                    };
                    let rich = if last {
                        RichText::new(shown).font(semibold(13.0)).color(p.text)
                    } else {
                        RichText::new(shown).size(13.0).color(p.accent)
                    };
                    let resp = ui.add(egui::Label::new(rich).sense(if last {
                        Sense::hover()
                    } else {
                        Sense::click()
                    }));
                    if !last && resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    resp.clicked()
                };
                let root = match self.group {
                    GroupBy::Category => "All categories",
                    GroupBy::App => "All apps",
                    GroupBy::Project => "All projects",
                    GroupBy::Title => "All titles",
                    GroupBy::Site => "All sites",
                };
                if crumb(ui, root, self.path.is_empty()) {
                    cut_to = Some(0);
                }
                for (i, step) in self.path.iter().enumerate() {
                    ui.label(
                        RichText::new(Icon::CHEVRON_RIGHT.glyph())
                            .size(9.0)
                            .color(p.faint),
                    );
                    if crumb(ui, step.name(), i + 1 == self.path.len()) {
                        cut_to = Some(i + 1);
                    }
                }
                if self.path.is_empty() {
                    ui.label(
                        RichText::new("  click a row to look inside")
                            .size(12.5)
                            .color(p.faint),
                    );
                }
            },
        );
        ui.add_space(2.0);

        let mut push = None;
        egui::ScrollArea::vertical()
            .id_salt("breakdown")
            .auto_shrink([false, false])
            .show(ui, |ui| match &list.listing {
                Listing::Rows(rows) if rows.is_empty() => {
                    charts::nothing(ui, "Nothing recorded in this period.");
                }
                Listing::Rows(rows) => {
                    let by = match self.level() {
                        Level::Group(by) => by,
                        Level::Raw => GroupBy::Title,
                    };
                    let clicked = charts::ranked_list(
                        ui,
                        rows,
                        &RankOptions {
                            icons: matches!(by, GroupBy::App | GroupBy::Title).then_some(cx.icons),
                            accent: p.accent,
                            clickable: true,
                            max_rows: 200,
                        },
                    );
                    if let Some(i) = clicked {
                        let row = &rows[i];
                        push = Some(match by {
                            GroupBy::Category => {
                                Step::Category(row.key.parse().unwrap_or(0), row.name.clone())
                            }
                            GroupBy::Project => {
                                Step::Project(row.key.parse().ok(), row.name.clone())
                            }
                            GroupBy::App => Step::App(row.key.clone(), row.name.clone()),
                            GroupBy::Site => Step::Site(row.key.clone()),
                            GroupBy::Title => Step::Title(row.key.clone()),
                        });
                    }
                }
                Listing::Raw(rows) => raw_rows(ui, cx, cache, rows),
            });
        if let Some(n) = cut_to {
            self.path.truncate(n);
        }
        if let Some(step) = push {
            self.path.push(step);
        }
    }
}

fn score_color(ui: &Ui, score: u32) -> Color32 {
    let p = pal(ui);
    if score >= 70 {
        p.good
    } else if score >= 40 {
        p.warn
    } else {
        p.danger
    }
}

fn summary_card(ui: &mut Ui, cx: &Cx, cache: &Cache) {
    let c = &cache.cmp;
    charts::card(ui, |ui| {
        let p = pal(ui);
        let cell = (ui.available_width() / 6.0).floor() - 8.0;
        let score = score_color(ui, c.cur.score);
        let single = cache.from == cache.to;
        let versus = format!("Compared with {}", range_label(cache.prev.0, cache.prev.1));
        let small = |ui: &mut Ui, text: String| {
            ui.label(RichText::new(text).size(12.0).color(p.faint));
        };
        ui.horizontal_top(|ui| {
            charts::stat_cell(
                ui,
                cell,
                &timeutil::fmt_dur(c.cur.worked),
                if cache.last_day < cache.to {
                    "Worked so far"
                } else {
                    "Worked"
                },
                Some(p.accent),
                |ui| {
                    charts::delta_badge(ui, &c.worked, DeltaUnit::Percent, Some(true))
                        .on_hover_text(format!(
                            "{versus}: {} then",
                            timeutil::fmt_dur(c.prev.worked)
                        ));
                },
            );
            if single {
                charts::stat_cell(
                    ui,
                    cell,
                    &timeutil::fmt_dur(c.cur.by_score[0]),
                    "Productive",
                    None,
                    |ui| {
                        small(
                            ui,
                            format!("{} distracting", timeutil::fmt_dur(c.cur.by_score[2])),
                        )
                    },
                );
            } else {
                charts::stat_cell(
                    ui,
                    cell,
                    &timeutil::fmt_dur(c.cur.avg_active_day()),
                    "Daily average",
                    None,
                    |ui| {
                        small(
                            ui,
                            format!(
                                "on {} active {}",
                                c.cur.days_with_data,
                                if c.cur.days_with_data == 1 {
                                    "day"
                                } else {
                                    "days"
                                }
                            ),
                        );
                    },
                )
            }
            charts::stat_cell(
                ui,
                cell,
                &c.cur.score.to_string(),
                "Productivity score",
                Some(score),
                |ui| {
                    charts::delta_badge(ui, &c.score, DeltaUnit::Points, Some(true))
                        .on_hover_text(format!("{versus}: {} then", c.prev.score));
                },
            );
            charts::stat_cell(
                ui,
                cell,
                &timeutil::fmt_dur(c.cur.away),
                "Away",
                None,
                |ui| {
                    charts::delta_badge(ui, &c.away, DeltaUnit::Percent, None).on_hover_text(
                        format!("{versus}: {} then", timeutil::fmt_dur(c.prev.away)),
                    );
                },
            );
            let best = cache.hours.best_hour();
            charts::stat_cell(
                ui,
                cell,
                &best.map_or("–".to_string(), |h| format!("{h:02}:00")),
                "Most productive hour",
                None,
                |ui| {
                    if best.is_some() {
                        small(ui, "by productive time".into());
                    }
                },
            );
            if single {
                let first = cache.rows.first().and_then(|r| r.first);
                let last = cache.rows.first().and_then(|r| r.last);
                let text = match (first, last) {
                    (Some(a), Some(b)) => format!(
                        "{} – {}",
                        timeutil::fmt_hm(a, cx.tz),
                        timeutil::fmt_hm(b, cx.tz)
                    ),
                    _ => "–".into(),
                };
                charts::stat_cell(ui, cell, &text, "First to last activity", None, |_| {});
            } else {
                let busiest = cache
                    .rows
                    .iter()
                    .rev()
                    .max_by_key(|r| r.worked)
                    .filter(|r| r.worked > 0);
                charts::stat_cell(
                    ui,
                    cell,
                    &busiest.map_or("–".to_string(), |r| timeutil::fmt_day_short(r.day)),
                    "Busiest day",
                    None,
                    |ui| {
                        if let Some(r) = busiest {
                            small(
                                ui,
                                format!(
                                    "{}, {}",
                                    timeutil::WEEKDAYS[timeutil::weekday(r.day) as usize],
                                    timeutil::fmt_dur(r.worked)
                                ),
                            );
                        }
                    },
                );
            }
        });
    });
}

fn compare_card(ui: &mut Ui, cx: &Cx, cache: &Cache) {
    let p = pal(ui);
    let c = &cache.cmp;
    charts::card_title(ui, "Against the period before", "");
    ui.label(
        RichText::new(format!(
            "{}  ·  {} then, {} now",
            range_label(cache.prev.0, cache.prev.1),
            timeutil::fmt_dur(c.prev.worked),
            timeutil::fmt_dur(c.cur.worked)
        ))
        .size(12.5)
        .color(p.faint),
    );
    ui.add_space(2.0);
    // Daily averages, both ways of counting.
    for (label, delta) in [
        ("Average per active day", &c.avg_active_day),
        ("Average over all days", &c.avg_day),
    ] {
        compare_row(ui, None, label, delta, Some(true), DeltaUnit::Duration);
    }
    widgets::divider(ui);
    if c.categories.is_empty() {
        charts::nothing(ui, "Nothing recorded in either period.");
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("compare")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, delta) in &c.categories {
                let Some(category) = cx.classifier.categories.get(*index) else {
                    continue;
                };
                // More productive time is good, more distracting time is not.
                let up_is_good = match category.score {
                    Score::Productive => Some(true),
                    Score::Distracting => Some(false),
                    Score::Neutral => None,
                };
                compare_row(
                    ui,
                    Some(widgets::rgb(category.color)),
                    &category.name,
                    delta,
                    up_is_good,
                    DeltaUnit::Percent,
                );
            }
        });
}

fn compare_row(
    ui: &mut Ui,
    color: Option<Color32>,
    name: &str,
    delta: &aggregate::Delta,
    up_is_good: Option<bool>,
    unit: DeltaUnit,
) {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let mut left = rect.left();
    if let Some(color) = color {
        ui.painter()
            .circle_filled(pos2(left + 7.0, rect.center().y), 5.0, color);
        left += 22.0;
    }
    let right = rect.right();
    widgets::text_in(
        ui,
        Rect::from_min_max(pos2(left, rect.top()), pos2(right - 170.0, rect.bottom())),
        name,
        regular(14.0),
        p.text,
    );
    widgets::text_in_right(
        ui,
        Rect::from_min_max(
            pos2(right - 166.0, rect.top()),
            pos2(right - 92.0, rect.bottom()),
        ),
        &timeutil::fmt_dur(delta.cur),
        semibold(13.5),
        p.text,
    );
    let diff = delta.diff();
    let badge_color = match up_is_good {
        _ if diff == 0 => p.muted,
        Some(good_up) if (diff > 0) == good_up => p.good,
        Some(_) => p.danger,
        None => p.muted,
    };
    widgets::paint_badge(
        ui,
        Rect::from_min_max(pos2(right - 84.0, rect.top()), rect.max),
        &charts::delta_text(delta, unit),
        badge_color,
    );
    resp.on_hover_text(format!(
        "{name}\nBefore: {}\nNow: {}",
        timeutil::fmt_dur(delta.prev),
        timeutil::fmt_dur(delta.cur)
    ));
}

/// The last drill-down level: every piece of worked time behind a title.
fn raw_rows(ui: &mut Ui, cx: &mut Cx, cache: &Cache, rows: &[RawRow]) {
    let p = pal(ui);
    if rows.is_empty() {
        charts::nothing(ui, "Nothing recorded.");
        return;
    }
    let multi_day = cache.from != cache.to;
    let total: i64 = rows.iter().map(|r| r.end - r.start).sum();
    ui.label(
        RichText::new(format!(
            "{} {} in the timeline, {} in total. Click one to open it there.",
            rows.len(),
            if rows.len() == 1 { "piece" } else { "pieces" },
            timeutil::fmt_dur(total)
        ))
        .size(12.5)
        .color(p.faint),
    );
    ui.spacing_mut().item_spacing.y = 1.0;
    for row in rows.iter().take(500) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect.expand2(vec2(6.0, 0.0)), CornerRadius::same(7), p.hover);
        }
        let day = cx.tz.day_of(row.start);
        let mut x = rect.left();
        if multi_day {
            widgets::text_in(
                ui,
                Rect::from_min_max(pos2(x, rect.top()), pos2(x + 86.0, rect.bottom())),
                &format!(
                    "{} {}",
                    timeutil::WEEKDAYS[timeutil::weekday(day) as usize],
                    timeutil::fmt_day_short(day)
                ),
                regular(13.5),
                p.muted,
            );
            x += 92.0;
        }
        widgets::text_in(
            ui,
            Rect::from_min_max(pos2(x, rect.top()), pos2(x + 104.0, rect.bottom())),
            &format!(
                "{} – {}",
                timeutil::fmt_hm(row.start, cx.tz),
                timeutil::fmt_hm(row.end, cx.tz)
            ),
            regular(13.5),
            p.text,
        );
        x += 110.0;
        widgets::text_in_right(
            ui,
            Rect::from_min_max(pos2(rect.right() - 70.0, rect.top()), rect.max),
            &timeutil::fmt_dur(row.end - row.start),
            semibold(13.5),
            p.text,
        );
        let title = if row.title.is_empty() {
            &row.app
        } else {
            &row.title
        };
        widgets::text_in(
            ui,
            Rect::from_min_max(
                pos2(x, rect.top()),
                pos2(rect.right() - 76.0, rect.bottom()),
            ),
            title,
            regular(13.5),
            p.muted,
        );
        if resp.clicked() {
            cx.actions.push(Action::OpenTimeline {
                day,
                block: Some(row.block),
            });
        }
    }
    if rows.len() > 500 {
        ui.label(
            RichText::new(format!("and {} more", rows.len() - 500))
                .size(12.5)
                .color(p.faint),
        );
    }
}
