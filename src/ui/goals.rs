//! The Goals & focus page: daily goals and limits with today's progress,
//! the focus-session timer and break reminders.

use super::app::{page_header, App};
use super::dialogs::Dialog;
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::engine::Cmd;
use crate::model::{Block, Goal, GoalTarget, Kind};
use crate::spans::Span;
use crate::timeutil;
use eframe::egui::{self, pos2, vec2, CornerRadius, Rect, RichText, Sense, Stroke, Ui};

#[derive(Default)]
pub struct GoalsState {
    key: Option<(u64, i32, usize)>,
    /// Per goal: seconds on each of the last 7 days, oldest first (today last).
    week: Vec<[i64; 7]>,
    /// Focus sessions of the last 7 days: (start, seconds).
    sessions: Vec<(i64, i64)>,
}

/// Seconds a goal's target got in a list of spans.
pub fn goal_secs(goal: &Goal, spans: &[Span], classifier: &crate::rules::Classifier) -> i64 {
    let work = spans.iter().filter(|s| s.work);
    match &goal.target {
        GoalTarget::Total => work.map(Span::secs).sum(),
        GoalTarget::Category(id) => match classifier.category_index(id) {
            Some(cat) => work
                .filter(|s| s.category as usize == cat)
                .map(Span::secs)
                .sum(),
            None => 0,
        },
        GoalTarget::Project(id) => match classifier.project_index(id) {
            Some(project) => work
                .filter(|s| s.project == Some(project as u16))
                .map(Span::secs)
                .sum(),
            None => 0,
        },
    }
}

impl App {
    fn load_goals(&mut self) {
        let today = self.today();
        let key = (self.rev(), today, self.settings.goals.len());
        if self.goals.key == Some(key) {
            return;
        }
        let from_day = today - 6;
        let (from, to) = (self.tz.day_start(from_day), self.tz.day_start(today + 1));
        let blocks: Vec<Block> = self.blocks(from, to);
        let mut week = vec![[0i64; 7]; self.settings.goals.len()];
        for offset in 0..7 {
            let (a, b) = self.tz.day_bounds(from_day + offset);
            let spans = crate::spans::flatten(&blocks, a, b, &self.classifier);
            for (g, goal) in self.settings.goals.iter().enumerate() {
                week[g][offset as usize] = goal_secs(goal, &spans, &self.classifier);
            }
        }
        self.goals.week = week;
        let mut sessions: Vec<(i64, i64)> = blocks
            .iter()
            .filter(|b| b.kind == Kind::Focus)
            .map(|b| (b.start, b.secs()))
            .collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.0));
        self.goals.sessions = sessions;
        self.goals.key = Some(key);
    }

    pub(super) fn goals_ui(&mut self, ui: &mut Ui) {
        let _ = pal(ui);
        self.load_goals();
        let mut new_goal = false;
        page_header(
            ui,
            "Goals & focus",
            "Daily targets, gentle limits on distractions, focus sessions and break reminders.",
            |ui| {
                if widgets::primary_button(ui, &Icon::ADD.with("New goal"), true).clicked() {
                    new_goal = true;
                }
            },
        );
        if new_goal {
            self.dialog = Some(Dialog::goal(None, &self.settings));
        }
        let now = self.now();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let w = (ui.available_width() - 12.0) / 2.0;
                    ui.allocate_ui_with_layout(
                        vec2(w, 190.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| self.focus_card(ui, now),
                    );
                    ui.allocate_ui_with_layout(
                        vec2(w, 190.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| self.breaks_card(ui),
                    );
                });
                ui.add_space(4.0);
                self.goals_card(ui);
            });
    }

    fn focus_card(&mut self, ui: &mut Ui, now: i64) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            ui.set_min_height(156.0);
            widgets::section(ui, "Focus session");
            if let Some(focus) = self.status.focus.clone() {
                let total = (focus.until - focus.start).max(1);
                let left = (focus.until - now).max(0);
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 64.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(timeutil::fmt_clock(left))
                                .font(semibold(34.0))
                                .color(p.accent),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "left of {} minutes\nuntil {}",
                                total / 60,
                                timeutil::fmt_hm(focus.until, &self.tz)
                            ))
                            .color(p.muted),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::danger_button(ui, &Icon::STOP.with("Stop"), true).clicked()
                            {
                                self.send(Cmd::StopFocus);
                            }
                        });
                    },
                );
                widgets::progress(ui, 1.0 - left as f32 / total as f32, p.accent, 8.0);
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(1));
            } else {
                ui.label(
                    RichText::new(
                        "Work on one thing for a set time. The session is recorded in the timeline, and a notice tells you when it is over.",
                    )
                    .color(p.muted),
                );
                ui.add_space(4.0);
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 38.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        if widgets::primary_button(
                            ui,
                            &Icon::FOCUS.with("Start focus session"),
                            true,
                        )
                        .clicked()
                        {
                            self.send(Cmd::StartFocus {
                                minutes: self.settings.focus_minutes,
                            });
                        }
                        ui.add_space(6.0);
                        ui.add(
                            egui::DragValue::new(&mut self.settings.focus_minutes)
                                .range(5..=180)
                                .suffix(" min"),
                        );
                        ui.label(RichText::new("then a break of").color(p.muted));
                        ui.add(
                            egui::DragValue::new(&mut self.settings.focus_break_minutes)
                                .range(1..=60)
                                .suffix(" min"),
                        );
                    },
                );
            }
            let done: i64 = self.goals.sessions.iter().map(|s| s.1).sum();
            ui.label(
                RichText::new(format!(
                    "{} session{} in the last 7 days, {} in total.",
                    self.goals.sessions.len(),
                    if self.goals.sessions.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    timeutil::fmt_dur(done)
                ))
                .size(12.5)
                .color(p.faint),
            );
        });
    }

    fn breaks_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            ui.set_min_height(156.0);
            widgets::section(ui, "Break reminders");
            ui.label(
                RichText::new(
                    "A notice near the clock after a long stretch at the keyboard without a pause of two minutes or more.",
                )
                .color(p.muted),
            );
            ui.add_space(4.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 38.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let on = self.settings.break_reminder_minutes > 0;
                    if widgets::toggle(ui, on, true).clicked() {
                        self.settings.break_reminder_minutes = if on { 0 } else { 50 };
                    }
                    if on {
                        ui.label("Remind me after");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.break_reminder_minutes)
                                .range(10..=240)
                                .suffix(" min"),
                        );
                    } else {
                        ui.label(RichText::new("Off").color(p.muted));
                    }
                },
            );
            ui.label(
                RichText::new("Notices need the icon in the notification area; they never show window titles.")
                    .size(12.5)
                    .color(p.faint),
            );
        });
    }

    fn goals_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let mut edit: Option<usize> = None;
        let mut delete: Option<usize> = None;
        let mut toggle: Option<usize> = None;
        widgets::card(ui, |ui| {
            widgets::section(ui, "Daily goals and limits");
            if self.settings.goals.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "No goals yet. Try a target such as four hours of Development a day, or a limit such as 45 minutes of Media.",
                    )
                    .color(p.muted),
                );
                return;
            }
            ui.add_space(2.0);
            for (index, goal) in self.settings.goals.iter().enumerate() {
                let week = self.goals.week.get(index).copied().unwrap_or([0; 7]);
                let secs = week[6];
                let target = goal.minutes as i64 * 60;
                let (name, color) = match &goal.target {
                    GoalTarget::Total => ("All worked time".to_string(), p.accent),
                    GoalTarget::Category(id) => (
                        self.settings.category_name(id).to_string(),
                        self.settings
                            .category(id)
                            .map(|c| widgets::rgb(c.color))
                            .unwrap_or(p.faint),
                    ),
                    GoalTarget::Project(id) => (
                        self.settings.project_name(id).to_string(),
                        self.settings
                            .project(id)
                            .map(|c| widgets::rgb(c.color))
                            .unwrap_or(p.faint),
                    ),
                };
                let reached = secs >= target;
                let (state, state_color) = match (goal.limit, reached) {
                    (false, true) => ("Reached".to_string(), p.good),
                    (false, false) => (
                        format!("{} to go", timeutil::fmt_dur(target - secs)),
                        p.muted,
                    ),
                    (true, true) => (
                        format!("Over by {}", timeutil::fmt_dur(secs - target)),
                        p.danger,
                    ),
                    (true, false) => (
                        format!("{} left", timeutil::fmt_dur(target - secs)),
                        p.muted,
                    ),
                };
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::hover());
                ui.add_space(-8.0);
                let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
                if index > 0 {
                    ui.painter().hline(
                        rect.x_range(),
                        rect.top(),
                        Stroke::new(1.0_f32, p.border.gamma_multiply(0.6)),
                    );
                }
                ui.painter()
                    .circle_filled(pos2(rect.left() + 8.0, rect.top() + 20.0), 5.0, color);
                widgets::text_in(
                    ui,
                    Rect::from_min_max(
                        pos2(rect.left() + 22.0, rect.top() + 8.0),
                        pos2(rect.left() + 250.0, rect.top() + 32.0),
                    ),
                    &name,
                    semibold(14.5),
                    p.text,
                );
                widgets::text_in(
                    ui,
                    Rect::from_min_max(
                        pos2(rect.left() + 22.0, rect.top() + 31.0),
                        pos2(rect.left() + 250.0, rect.bottom() - 6.0),
                    ),
                    &format!(
                        "{} {} a day{}",
                        if goal.limit { "At most" } else { "At least" },
                        timeutil::fmt_dur(target),
                        if goal.alert { "" } else { "  ·  no notice" }
                    ),
                    regular(12.5),
                    p.muted,
                );
                // Progress.
                let bar_left = rect.left() + 262.0;
                let bar_right = rect.right() - 318.0;
                let bar = Rect::from_min_max(
                    pos2(bar_left, rect.top() + 26.0),
                    pos2(bar_right.max(bar_left + 60.0), rect.top() + 34.0),
                );
                ui.painter()
                    .rect_filled(bar, CornerRadius::same(4), p.hover);
                let fraction = (secs as f32 / target.max(1) as f32).clamp(0.0, 1.0);
                if fraction > 0.0 {
                    let fill = Rect::from_min_size(
                        bar.min,
                        vec2((bar.width() * fraction).max(8.0), bar.height()),
                    );
                    let fill_color = match (goal.limit, reached) {
                        (true, true) => p.danger,
                        (false, true) => p.good,
                        _ => color,
                    };
                    ui.painter()
                        .rect_filled(fill, CornerRadius::same(4), fill_color);
                }
                widgets::text_in(
                    ui,
                    Rect::from_min_max(
                        pos2(bar.left(), rect.top() + 6.0),
                        pos2(bar.right(), rect.top() + 24.0),
                    ),
                    &format!("{} today", timeutil::fmt_dur(secs)),
                    regular(12.5),
                    p.text,
                );
                widgets::text_in_right(
                    ui,
                    Rect::from_min_max(
                        pos2(bar.left(), rect.top() + 36.0),
                        pos2(bar.right(), rect.bottom() - 4.0),
                    ),
                    &state,
                    regular(12.0),
                    state_color,
                );
                // The last seven days: a square per day, filled when met.
                let week_left = rect.right() - 296.0;
                let today = self.today();
                for (d, day_secs) in week.iter().enumerate() {
                    let met = if goal.limit {
                        *day_secs < target
                    } else {
                        *day_secs >= target
                    };
                    let cell = Rect::from_min_size(
                        pos2(week_left + d as f32 * 26.0, rect.top() + 12.0),
                        vec2(20.0, 20.0),
                    );
                    let had_data = *day_secs > 0 || goal.limit;
                    let fill = if met && had_data {
                        if goal.limit {
                            p.good.gamma_multiply(0.55)
                        } else {
                            p.good
                        }
                    } else if goal.limit {
                        p.danger.gamma_multiply(0.8)
                    } else {
                        p.hover
                    };
                    ui.painter().rect_filled(cell, CornerRadius::same(5), fill);
                    let wd = timeutil::weekday(today - 6 + d as i32) as usize;
                    ui.painter().text(
                        pos2(cell.center().x, rect.top() + 42.0),
                        egui::Align2::CENTER_CENTER,
                        &timeutil::WEEKDAYS[wd][..1],
                        regular(10.5),
                        if d == 6 { p.text } else { p.faint },
                    );
                    ui.interact(cell, ui.id().with(("goal-day", index, d)), Sense::hover())
                        .on_hover_text(format!(
                            "{}: {}",
                            timeutil::fmt_day(today - 6 + d as i32),
                            timeutil::fmt_dur(*day_secs)
                        ));
                }
                // Controls.
                let mut x = rect.right() - 32.0;
                for (icon, tip, what) in [(Icon::DELETE, "Delete", 0), (Icon::EDIT, "Edit", 1)] {
                    let r = Rect::from_min_size(pos2(x, rect.top() + 14.0), vec2(28.0, 28.0));
                    let br = ui
                        .interact(r, ui.id().with((tip, "goal", index)), Sense::click())
                        .on_hover_text(tip);
                    if br.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(7), p.border);
                    }
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        icon.glyph(),
                        regular(13.5),
                        if br.hovered() || hovered {
                            p.text
                        } else {
                            p.faint
                        },
                    );
                    if br.clicked() {
                        if what == 0 {
                            delete = Some(index);
                        } else {
                            edit = Some(index);
                        }
                    }
                    x -= 30.0;
                }
                let switch =
                    Rect::from_min_size(pos2(x - 12.0, rect.top() + 17.0), vec2(38.0, 22.0));
                let sr =
                    widgets::toggle_at(ui, switch, ui.id().with(("goal-alert", index)), goal.alert)
                        .on_hover_text("Notify when reached");
                if sr.clicked() {
                    toggle = Some(index);
                }
            }
        });
        if let Some(i) = toggle {
            self.settings.goals[i].alert = !self.settings.goals[i].alert;
        }
        if let Some(i) = edit {
            self.dialog = Some(Dialog::goal(Some(i), &self.settings));
        }
        if let Some(i) = delete {
            self.settings.goals.remove(i);
        }
    }
}
