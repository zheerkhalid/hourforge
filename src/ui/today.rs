//! The Today page: one day at a glance. Totals, the day bar coloured by
//! category, the programs, window titles and sites with the most time,
//! categories, projects and focus sessions.

use super::app::{page_header, Action, Cx};
use super::charts::{self, RankOptions, GAP};
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::aggregate::{
    self, Data, Filter, FocusParams, FocusSession, GroupBy, Row, SegKind, Segment, Summary,
};
use crate::model::TrackState;
use crate::spans::flatten;
use crate::timeutil::{self, Day};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Rect, RichText, Sense, Ui};

/// Rows shown in each "top" card.
const TOP_ROWS: usize = 6;
const CARD_HEIGHT: f32 = 300.0;

/// Everything the page shows for one day, computed once per change.
struct DayData {
    day: Day,
    rev: u64,
    minute: i64,
    bounds: (i64, i64),
    summary: Summary,
    segments: Vec<Segment>,
    apps: Vec<Row>,
    titles: Vec<Row>,
    sites: Vec<Row>,
    categories: Vec<Row>,
    projects: Vec<Row>,
    focus: Vec<FocusSession>,
    /// Timers and focus-mode sessions recorded on the day.
    timers: usize,
}

#[derive(Default)]
pub struct TodayView {
    /// The day shown; `None` follows today.
    day: Option<Day>,
    data: Option<DayData>,
    /// A sub-state asked for on the command line, applied on the first frame.
    pending: Option<String>,
}

impl TodayView {
    /// Opens a sub-state named on the command line: `today`,
    /// `today-yesterday`, `today-empty` (a day without data).
    pub fn apply_view(&mut self, view: &str) {
        self.pending = Some(view.to_string());
    }

    fn load(&mut self, cx: &Cx, day: Day) {
        let minute = if day == cx.today { cx.now / 60 } else { 0 };
        if let Some(d) = &self.data {
            if d.day == day && d.rev == cx.rev && d.minute == minute {
                return;
            }
        }
        let bounds = cx.tz.day_bounds(day);
        let blocks = cx.blocks(bounds.0, bounds.1);
        let spans = flatten(&blocks, bounds.0, bounds.1, cx.classifier);
        let d = Data {
            blocks: &blocks,
            spans: &spans,
            classifier: cx.classifier,
            tz: cx.tz,
        };
        let all = Filter::default();
        self.data = Some(DayData {
            day,
            rev: cx.rev,
            minute,
            bounds,
            summary: aggregate::summary(&d),
            segments: aggregate::timeline(&d),
            apps: aggregate::with_other(aggregate::ranked(&d, GroupBy::App, &all), TOP_ROWS - 1),
            titles: aggregate::ranked(&d, GroupBy::Title, &all),
            sites: aggregate::ranked(&d, GroupBy::Site, &all),
            categories: aggregate::ranked(&d, GroupBy::Category, &all),
            projects: aggregate::ranked(&d, GroupBy::Project, &all),
            focus: aggregate::focus_sessions(&d, &FocusParams::default()),
            timers: blocks.iter().filter(|b| b.is_overlay()).count(),
        });
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if let Some(view) = self.pending.take() {
            match view.as_str() {
                "today-yesterday" => self.day = Some(cx.today - 1),
                "today-empty" => self.day = Some(cx.today + 2),
                _ => {}
            }
        }
        let day = self.day.unwrap_or(cx.today);
        self.load(cx, day);

        let title = if day == cx.today {
            "Today"
        } else if day == cx.today - 1 {
            "Yesterday"
        } else {
            "Day"
        };
        let mut go: Option<Option<Day>> = None;
        page_header(ui, title, &timeutil::fmt_day(day), |ui| {
            if widgets::ghost_button(ui, &Icon::EXPORT.with("Export")).clicked() {
                cx.actions.push(Action::Export { from: day, to: day });
            }
            if widgets::ghost_button(ui, &Icon::LIST.with("Open timeline"))
                .on_hover_text("Every recorded block of this day, to edit, split or label")
                .clicked()
            {
                cx.actions.push(Action::OpenTimeline { day, block: None });
            }
            ui.add_space(6.0);
            if widgets::icon_button(ui, Icon::CHEVRON_RIGHT, "Next day").clicked() {
                go = Some(Some(day + 1).filter(|d| *d != cx.today));
            }
            if widgets::ghost_button_enabled(ui, "Today", day != cx.today).clicked() {
                go = Some(None);
            }
            if widgets::icon_button(ui, Icon::BACK, "Previous day").clicked() {
                go = Some(Some(day - 1).filter(|d| *d != cx.today));
            }
        });
        if let Some(target) = go {
            self.day = target;
            return;
        }
        let Some(data) = &self.data else { return };

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if data.summary.worked == 0 && data.segments.is_empty() {
                    let p = pal(ui);
                    let (headline, text) = if day == cx.today {
                        (
                            "Nothing recorded yet today",
                            "Time shows up here as you work. HourForge notes which program and window is in front, on this PC only.",
                        )
                    } else if day > cx.today {
                        (
                            "This day has not happened yet",
                            "Use the arrows above to go back to a day with recorded time.",
                        )
                    } else {
                        (
                            "Nothing was recorded on this day",
                            "The PC was off, tracking was paused, or the data was deleted. Offline work can be added by hand in the timeline.",
                        )
                    };
                    widgets::empty_state(ui, Icon::CLOCK, p.faint, headline, text);
                    return;
                }
                let width = ui.available_width();
                summary_card(ui, cx, data);
                ui.add_space(GAP - 8.0);
                timeline_card(ui, cx, data);
                ui.add_space(GAP - 8.0);

                let third = ((width - 2.0 * GAP) / 3.0).floor();
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    charts::card_sized(ui, third, CARD_HEIGHT, |ui| {
                        charts::card_title(ui, "Top apps", "");
                        top_list(ui, cx, data, &data.apps, true, "No program time on this day.");
                    });
                    charts::card_sized(ui, third, CARD_HEIGHT, |ui| {
                        charts::card_title(ui, "Top windows and documents", "");
                        top_list(
                            ui,
                            cx,
                            data,
                            &data.titles,
                            true,
                            "No window titles were recorded.",
                        );
                    });
                    charts::card_sized(ui, third, CARD_HEIGHT, |ui| {
                        charts::card_title(ui, "Top sites", "domain only");
                        top_list(ui, cx, data, &data.sites, false, "No websites on this day.");
                    });
                });
                ui.add_space(GAP - 8.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    charts::card_sized(ui, third, CARD_HEIGHT, |ui| categories_card(ui, cx, data));
                    charts::card_sized(ui, third, CARD_HEIGHT, |ui| projects_card(ui, cx, data));
                    charts::card_sized(ui, third, CARD_HEIGHT, |ui| focus_card(ui, cx, data));
                });
                ui.add_space(8.0);
            });
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

fn summary_card(ui: &mut Ui, cx: &Cx, data: &DayData) {
    let s = &data.summary;
    charts::card(ui, |ui| {
        let p = pal(ui);
        let cell = (ui.available_width() / 6.0).floor() - 8.0;
        let score = score_color(ui, s.score);
        let small = |ui: &mut Ui, text: String| {
            ui.label(RichText::new(text).size(12.0).color(p.faint));
        };
        ui.horizontal_top(|ui| {
            charts::stat_cell(
                ui,
                cell,
                &timeutil::fmt_dur(s.worked),
                "Worked",
                Some(p.accent),
                |ui| {
                    small(
                        ui,
                        format!("{} productive", timeutil::fmt_dur(s.productive)),
                    )
                },
            );
            charts::stat_cell(ui, cell, &timeutil::fmt_dur(s.away), "Away", None, |ui| {
                small(ui, "idle, locked, breaks".into())
            });
            let span = match (s.first, s.last) {
                (Some(a), Some(b)) => {
                    format!(
                        "{} – {}",
                        timeutil::fmt_hm(a, cx.tz),
                        timeutil::fmt_hm(b, cx.tz)
                    )
                }
                _ => "–".into(),
            };
            charts::stat_cell(ui, cell, &span, "First to last activity", None, |ui| {
                if let (Some(a), Some(b)) = (s.first, s.last) {
                    small(ui, format!("{} span", timeutil::fmt_dur(b - a)));
                }
            });
            charts::stat_cell(
                ui,
                cell,
                &s.score.to_string(),
                "Productivity score",
                Some(score),
                |ui| {
                    if s.distracting > 0 {
                        small(
                            ui,
                            format!("{} distracting", timeutil::fmt_dur(s.distracting)),
                        );
                    } else {
                        small(ui, "out of 100".into());
                    }
                },
            );
            charts::stat_cell(
                ui,
                cell,
                &s.switches.to_string(),
                "Context switches",
                None,
                |ui| {
                    let hours = s.worked as f32 / 3600.0;
                    if hours > 0.25 {
                        small(ui, format!("{:.0} an hour", s.switches as f32 / hours));
                    }
                },
            );
            let focus_secs: i64 = data
                .focus
                .iter()
                .filter(|f| !f.recorded)
                .map(|f| f.end - f.start)
                .sum();
            let found = data.focus.iter().filter(|f| !f.recorded).count();
            charts::stat_cell(ui, cell, &found.to_string(), "Focus sessions", None, |ui| {
                if found > 0 {
                    small(ui, format!("{} in total", timeutil::fmt_dur(focus_secs)));
                } else {
                    small(ui, "25 min or longer".into());
                }
            });
        });
    });
}

fn timeline_card(ui: &mut Ui, cx: &mut Cx, data: &DayData) {
    charts::card(ui, |ui| {
        let p = pal(ui);
        let is_today = data.day == cx.today;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Day timeline").font(semibold(15.0)));
            if is_today {
                let (color, text) = match cx.status.state {
                    TrackState::Active if !cx.status.app.is_empty() => {
                        (p.good, format!("Now tracking: {}", cx.status.app))
                    }
                    TrackState::Active => (p.good, "Tracking".to_string()),
                    TrackState::Idle => (p.warn, "Away from the keyboard".to_string()),
                    TrackState::Locked => (p.warn, "The PC is locked".to_string()),
                    TrackState::Asleep => (p.warn, "The PC is asleep".to_string()),
                    TrackState::Paused => (p.danger, "Tracking is paused".to_string()),
                    TrackState::Off => (p.faint, "The tracker is not running".to_string()),
                };
                ui.add_space(4.0);
                widgets::dot(ui, color);
                ui.add(egui::Label::new(RichText::new(text).size(12.5).color(p.muted)).truncate());
            }
        });
        let now = is_today.then_some(cx.now);
        let window = charts::timeline_window(&data.segments, data.bounds, now, cx.tz);
        if let Some(i) = charts::day_timeline(ui, &data.segments, window, now, cx.classifier, cx.tz)
        {
            cx.actions.push(Action::OpenTimeline {
                day: data.day,
                block: Some(data.segments[i].block),
            });
        }
        // Legend: the categories on the bar, most time first, then away.
        let mut items: Vec<(Color32, String)> = data
            .categories
            .iter()
            .filter_map(|r| Some((widgets::rgb(r.color?), r.name.clone())))
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
            for (color, name) in items.drain(..) {
                widgets::dot(ui, color);
                ui.label(RichText::new(name).size(12.5).color(p.muted));
                ui.add_space(8.0);
            }
            if data
                .segments
                .iter()
                .any(|s| matches!(s.kind, SegKind::Away(_)))
            {
                // Away is drawn as a thin line, not only in another colour.
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
                ui.painter().rect_filled(
                    Rect::from_center_size(r.center(), vec2(14.0, 5.0)),
                    CornerRadius::same(2),
                    p.faint.gamma_multiply(0.55),
                );
                ui.label(RichText::new("Away").size(12.5).color(p.muted));
                ui.add_space(8.0);
            }
            if data.timers > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} timer or focus {} in the timeline",
                        data.timers,
                        if data.timers == 1 { "entry" } else { "entries" }
                    ))
                    .size(12.5)
                    .color(p.faint),
                );
            }
        });
    });
}

fn top_list(ui: &mut Ui, cx: &mut Cx, data: &DayData, rows: &[Row], icons: bool, empty: &str) {
    if rows.is_empty() {
        charts::nothing(ui, empty);
        return;
    }
    let p = pal(ui);
    let clicked = charts::ranked_list(
        ui,
        rows,
        &RankOptions {
            icons: icons.then_some(cx.icons),
            accent: p.accent,
            clickable: true,
            max_rows: TOP_ROWS,
        },
    );
    if let Some(i) = clicked {
        cx.actions.push(Action::OpenTimeline {
            day: data.day,
            block: Some(rows[i].block),
        });
    }
}

fn categories_card(ui: &mut Ui, cx: &mut Cx, data: &DayData) {
    charts::card_title(ui, "Categories", "");
    if data.categories.is_empty() {
        charts::nothing(ui, "No worked time on this day.");
        return;
    }
    let p = pal(ui);
    let parts: Vec<(Color32, i64, String)> = data
        .categories
        .iter()
        .map(|r| {
            (
                r.color.map(widgets::rgb).unwrap_or(p.faint),
                r.secs,
                r.name.clone(),
            )
        })
        .collect();
    ui.add_space(2.0);
    let mut open = charts::share_bar(ui, &parts, 12.0).is_some();
    ui.add_space(4.0);
    open |= charts::ranked_list(
        ui,
        &data.categories,
        &RankOptions {
            icons: None,
            accent: p.accent,
            clickable: true,
            max_rows: TOP_ROWS - 1,
        },
    )
    .is_some();
    if open {
        cx.actions.push(Action::OpenReport {
            from: data.day,
            to: data.day,
        });
    }
}

fn projects_card(ui: &mut Ui, cx: &mut Cx, data: &DayData) {
    charts::card_title(ui, "Projects", "");
    let any = data.projects.iter().any(|r| !r.key.is_empty());
    if !any {
        charts::nothing(
            ui,
            if cx.classifier.projects.is_empty() {
                "No projects yet. Add them on the Projects page; time is matched by window title."
            } else {
                "No time matched a project on this day."
            },
        );
        return;
    }
    let p = pal(ui);
    let clicked = charts::ranked_list(
        ui,
        &data.projects,
        &RankOptions {
            icons: None,
            accent: p.faint,
            clickable: true,
            max_rows: TOP_ROWS,
        },
    );
    if clicked.is_some() {
        cx.actions.push(Action::OpenReport {
            from: data.day,
            to: data.day,
        });
    }
}

fn focus_card(ui: &mut Ui, cx: &mut Cx, data: &DayData) {
    charts::card_title(ui, "Focus sessions", "");
    if data.focus.is_empty() {
        charts::nothing(
            ui,
            "No stretch of 25 minutes of productive work without a longer interruption on this day.",
        );
        return;
    }
    let p = pal(ui);
    ui.spacing_mut().item_spacing.y = 2.0;
    let longest = data
        .focus
        .iter()
        .map(|f| f.end - f.start)
        .max()
        .unwrap_or(1)
        .max(1);
    for session in data.focus.iter().take(TOP_ROWS) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect.expand2(vec2(6.0, 0.0)), CornerRadius::same(7), p.hover);
        }
        let color = if session.recorded { p.info } else { p.good };
        let top = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 28.0));
        let glyph = if session.recorded {
            Icon::FOCUS
        } else {
            Icon::CLOCK
        };
        ui.painter().text(
            pos2(rect.left() + 10.0, top.center().y),
            egui::Align2::CENTER_CENTER,
            glyph.glyph(),
            regular(14.0),
            color,
        );
        let secs = session.end - session.start;
        let name = format!(
            "{} – {}   {}",
            timeutil::fmt_hm(session.start, cx.tz),
            timeutil::fmt_hm(session.end, cx.tz),
            if session.recorded {
                "focus mode"
            } else {
                "deep work"
            }
        );
        widgets::text_in(
            ui,
            Rect::from_min_max(
                pos2(rect.left() + 26.0, top.top()),
                pos2(rect.right() - 70.0, top.bottom()),
            ),
            &name,
            regular(14.0),
            p.text,
        );
        widgets::text_in_right(
            ui,
            Rect::from_min_max(pos2(rect.right() - 68.0, top.top()), top.max),
            &timeutil::fmt_dur(secs),
            semibold(13.5),
            p.text,
        );
        let track = Rect::from_min_max(
            pos2(rect.left() + 26.0, rect.bottom() - 6.0),
            pos2(rect.right(), rect.bottom() - 3.0),
        );
        ui.painter()
            .rect_filled(track, CornerRadius::same(2), p.hover);
        ui.painter().rect_filled(
            Rect::from_min_size(
                track.min,
                vec2(
                    (track.width() * secs as f32 / longest as f32).max(3.0),
                    track.height(),
                ),
            ),
            CornerRadius::same(2),
            color,
        );
        let tip = if session.recorded {
            format!(
                "A focus-mode session you started. {} of productive work in it.",
                timeutil::fmt_dur(session.productive)
            )
        } else {
            "Found in the tracked time: productive work with no interruption longer than 2 minutes."
                .to_string()
        };
        if resp.on_hover_text(tip).clicked() {
            cx.actions.push(Action::OpenTimeline {
                day: data.day,
                block: Some(session.block),
            });
        }
    }
}
