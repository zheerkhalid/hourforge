//! Modal dialogs: the "what were you doing while away?" question,
//! confirmations, editing and splitting blocks, manual entries, rules,
//! categories, projects, goals and exports.
//!
//! Every kind of dialog has its own modal id (a modal remembers its size),
//! and every button row has a fixed height (see `widgets::button_row`).

use super::app::App;
use super::theme::{pal, pal_ctx, semibold, Icon};
use super::widgets;
use crate::edit;
use crate::engine::Cmd;
use crate::export::{self, Format};
use crate::model::{
    AwayLabel, Block, BlockId, Category, Field, Goal, GoalTarget, Kind, Project, Rule, Score,
};
use crate::settings::{Settings, PALETTE};
use crate::timeutil::{self, Day, Tz};
use eframe::egui::{self, vec2, RichText, Sense, Ui};
use std::path::PathBuf;

pub enum ConfirmAction {
    DeleteBlocks(Vec<BlockId>),
    /// Delete everything recorded in a UTC range.
    DeleteRange(i64, i64),
    DeleteCategory(String),
    DeleteProject(String),
    ImportBackup(PathBuf),
    ResetRules,
}

pub struct EditDlg {
    block: Block,
    is_new: bool,
    date: String,
    start: String,
    end: String,
    title: String,
    /// `None` = by the rules.
    category: Option<String>,
    /// `None` = by the rules; `Some("")` = no project.
    project: Option<String>,
    note: String,
    tags: String,
    away: AwayLabel,
    error: Option<String>,
}

pub struct SplitDlg {
    block: Block,
    at: String,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Report,
    Timesheet,
}

pub struct ExportDlg {
    pub from: String,
    pub to: String,
    pub format: Format,
    pub kind: ExportKind,
    pub project: Option<String>,
    error: Option<String>,
}

pub struct RuleDlg {
    field: Field,
    pattern: String,
    category: String,
    error: Option<String>,
}

pub struct CategoryDlg {
    /// Id of the category being edited; `None` for a new one.
    original: Option<String>,
    name: String,
    color: [u8; 3],
    score: Score,
    error: Option<String>,
}

pub struct ProjectDlg {
    original: Option<String>,
    name: String,
    client: String,
    color: [u8; 3],
    rate: String,
    keywords: String,
    error: Option<String>,
}

pub struct GoalDlg {
    index: Option<usize>,
    target: GoalTarget,
    hours: u32,
    minutes: u32,
    limit: bool,
    alert: bool,
}

pub enum Dialog {
    Confirm {
        title: String,
        text: String,
        button: String,
        action: ConfirmAction,
    },
    Away,
    Edit(Box<EditDlg>),
    Split(Box<SplitDlg>),
    Export(Box<ExportDlg>),
    Rule(Box<RuleDlg>),
    Category(Box<CategoryDlg>),
    Project(Box<ProjectDlg>),
    Goal(Box<GoalDlg>),
}

impl Dialog {
    pub fn confirm(title: &str, text: &str, button: &str, action: ConfirmAction) -> Dialog {
        Dialog::Confirm {
            title: title.into(),
            text: text.into(),
            button: button.into(),
            action,
        }
    }

    pub fn edit(block: &Block, tz: &Tz) -> Dialog {
        Dialog::Edit(Box::new(EditDlg {
            is_new: false,
            date: timeutil::iso_date(tz.day_of(block.start)),
            start: timeutil::fmt_hm(block.start, tz),
            end: timeutil::fmt_hm(block.end, tz),
            title: block.title.clone(),
            category: block.category.clone(),
            project: block.project.clone(),
            note: block.note.clone(),
            tags: edit::tags_text(&block.tags),
            away: block.away,
            block: block.clone(),
            error: None,
        }))
    }

    /// A new manual entry on `day`: the hour before now (today) or 09:00 to 10:00.
    pub fn manual(day: Day, now: i64, tz: &Tz) -> Dialog {
        let (start, end) = if tz.day_of(now) == day && tz.second_of_day(now) >= 3600 {
            (now - 3600, now)
        } else {
            (tz.at(day, 9, 0), tz.at(day, 10, 0))
        };
        Dialog::Edit(Box::new(EditDlg {
            is_new: true,
            date: timeutil::iso_date(day),
            start: timeutil::fmt_hm(start, tz),
            end: timeutil::fmt_hm(end, tz),
            title: String::new(),
            category: None,
            project: None,
            note: String::new(),
            tags: String::new(),
            away: AwayLabel::None,
            block: Block {
                kind: Kind::Manual,
                start,
                end,
                ..Default::default()
            },
            error: None,
        }))
    }

    pub fn split(block: &Block, tz: &Tz) -> Dialog {
        let middle = block.start + block.secs() / 2;
        Dialog::Split(Box::new(SplitDlg {
            block: block.clone(),
            at: timeutil::fmt_hm(middle - middle.rem_euclid(60), tz),
            error: None,
        }))
    }

    pub fn export(from: Day, to: Day, kind: ExportKind, project: Option<String>) -> Dialog {
        Dialog::Export(Box::new(ExportDlg {
            from: timeutil::iso_date(from),
            to: timeutil::iso_date(to),
            format: if kind == ExportKind::Timesheet {
                Format::Csv
            } else {
                Format::Html
            },
            kind,
            project,
            error: None,
        }))
    }

    pub fn rule(field: Field, pattern: &str, category: &str) -> Dialog {
        Dialog::Rule(Box::new(RuleDlg {
            field,
            pattern: pattern.into(),
            category: category.into(),
            error: None,
        }))
    }

    pub fn category(existing: Option<&Category>, settings: &Settings) -> Dialog {
        Dialog::Category(Box::new(match existing {
            Some(c) => CategoryDlg {
                original: Some(c.id.clone()),
                name: c.name.clone(),
                color: c.color,
                score: c.score,
                error: None,
            },
            None => CategoryDlg {
                original: None,
                name: String::new(),
                color: PALETTE[(settings.categories.len() + 1) % PALETTE.len()],
                score: Score::Neutral,
                error: None,
            },
        }))
    }

    pub fn project(existing: Option<&Project>, settings: &Settings) -> Dialog {
        Dialog::Project(Box::new(match existing {
            Some(p) => ProjectDlg {
                original: Some(p.id.clone()),
                name: p.name.clone(),
                client: p.client.clone(),
                color: p.color,
                rate: if p.rate > 0.0 {
                    format!("{}", p.rate)
                } else {
                    String::new()
                },
                keywords: p.keywords.join(", "),
                error: None,
            },
            None => ProjectDlg {
                original: None,
                name: String::new(),
                client: String::new(),
                color: PALETTE[(settings.projects.len() * 5 + 9) % PALETTE.len()],
                rate: String::new(),
                keywords: String::new(),
                error: None,
            },
        }))
    }

    pub fn goal(index: Option<usize>, settings: &Settings) -> Dialog {
        let goal = index.and_then(|i| settings.goals.get(i)).cloned();
        Dialog::Goal(Box::new(match goal {
            Some(g) => GoalDlg {
                index,
                target: g.target,
                hours: g.minutes / 60,
                minutes: g.minutes % 60,
                limit: g.limit,
                alert: g.alert,
            },
            None => GoalDlg {
                index: None,
                target: GoalTarget::Total,
                hours: 6,
                minutes: 0,
                limit: false,
                alert: true,
            },
        }))
    }
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(19.0)));
    ui.add_space(2.0);
}

fn para(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    ui.add(egui::Label::new(RichText::new(text).color(p.muted)).wrap());
}

fn error_line(ui: &mut Ui, error: &Option<String>) {
    if let Some(e) = error {
        let p = pal(ui);
        ui.add(egui::Label::new(RichText::new(Icon::WARNING.with(e)).color(p.danger)).wrap());
    }
}

/// A labelled form row with a fixed label column and a fixed height.
fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 32.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(112.0, 30.0), Sense::hover());
            widgets::text_in(ui, rect, label, super::theme::regular(13.5), p.muted);
            add(ui);
        },
    );
}

fn text_field(ui: &mut Ui, value: &mut String, hint: &str, width: f32) -> egui::Response {
    let p = pal(ui);
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(RichText::new(hint).color(p.faint))
            .desired_width(width)
            .margin(egui::Margin::symmetric(8, 6)),
    )
}

/// A row of colour swatches to pick from.
fn color_picker(ui: &mut Ui, color: &mut [u8; 3]) {
    let p = pal(ui);
    ui.spacing_mut().item_spacing.x = 6.0;
    for swatch in PALETTE {
        let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
        ui.painter()
            .circle_filled(rect.center(), 9.0, widgets::rgb(swatch));
        if *color == swatch {
            ui.painter()
                .circle_stroke(rect.center(), 11.0, egui::Stroke::new(2.0_f32, p.text));
        }
        if resp.clicked() {
            *color = swatch;
        }
    }
}

fn category_combo(ui: &mut Ui, id: &str, settings: &Settings, value: &mut Option<String>) {
    let shown = match value {
        None => "By the rules".to_string(),
        Some(id) => settings.category_name(id).to_string(),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(240.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "By the rules");
            for c in &settings.categories {
                ui.selectable_value(value, Some(c.id.clone()), &c.name);
            }
        });
}

fn project_combo(
    ui: &mut Ui,
    id: &str,
    settings: &Settings,
    value: &mut Option<String>,
    rules_label: &str,
) {
    let shown = match value.as_deref() {
        None => rules_label.to_string(),
        Some("") => "No project".to_string(),
        Some(id) => settings.project_name(id).to_string(),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(240.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, rules_label);
            ui.selectable_value(value, Some(String::new()), "No project");
            for p in settings.projects.iter().filter(|p| !p.archived) {
                ui.selectable_value(value, Some(p.id.clone()), &p.name);
            }
        });
}

impl App {
    pub(super) fn dialogs_ui(&mut self, ctx: &egui::Context) {
        // The question about an absence opens by itself.
        if self.dialog.is_none() && self.away_prompt && !self.status.returned.is_empty() {
            self.dialog = Some(Dialog::Away);
        }
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut close = false;
        let mut confirmed = false;
        let modal = egui::Modal::new(egui::Id::new("dialog").with(std::mem::discriminant(&dialog)))
            .frame(widgets::modal_frame(ctx))
            .show(ctx, |ui| match &mut dialog {
                Dialog::Confirm {
                    title: t,
                    text,
                    button,
                    ..
                } => {
                    ui.set_width(460.0);
                    title(ui, t);
                    para(ui, text);
                    ui.add_space(12.0);
                    widgets::button_row(ui, |ui| {
                        if widgets::danger_button(ui, button, true).clicked() {
                            confirmed = true;
                            close = true;
                        }
                        if widgets::ghost_button(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Away => close = self.away_dialog(ui),
                Dialog::Edit(d) => close = self.edit_dialog(ui, d),
                Dialog::Split(d) => close = self.split_dialog(ui, d),
                Dialog::Export(d) => close = self.export_dialog(ui, d),
                Dialog::Rule(d) => close = self.rule_dialog(ui, d),
                Dialog::Category(d) => close = self.category_dialog(ui, d),
                Dialog::Project(d) => close = self.project_dialog(ui, d),
                Dialog::Goal(d) => close = self.goal_dialog(ui, d),
            });
        if modal.should_close() {
            close = true;
            if matches!(dialog, Dialog::Away) {
                self.away_prompt = false;
            }
        }
        if confirmed {
            if let Dialog::Confirm { action, .. } = dialog {
                self.run_confirmed(action);
            }
            return;
        }
        if !close && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
        let _ = pal_ctx(ctx);
    }

    fn run_confirmed(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteBlocks(ids) => {
                let n = ids.len();
                let done = self.store_write(|store| {
                    for id in &ids {
                        store.delete(*id)?;
                    }
                    Ok(())
                });
                if done.is_some() {
                    for id in &ids {
                        self.timeline.checked.remove(id);
                    }
                    self.toast(
                        format!("Deleted {n} block{}", if n == 1 { "" } else { "s" }),
                        false,
                    );
                }
            }
            ConfirmAction::DeleteRange(from, to) => {
                if let Some(n) = self.store_write(|store| {
                    let n = store.delete_range(from, to)?;
                    store.compact()?;
                    Ok(n)
                }) {
                    self.toast(
                        format!("Deleted everything in that range ({n} blocks changed)"),
                        false,
                    );
                }
            }
            ConfirmAction::DeleteCategory(id) => {
                self.settings.categories.retain(|c| c.id != id || c.builtin);
                self.settings.rules.retain(|r| r.category != id);
                self.settings
                    .goals
                    .retain(|g| g.target != GoalTarget::Category(id.clone()));
                if self.cats.selected.as_deref() == Some(id.as_str()) {
                    self.cats.selected = None;
                }
            }
            ConfirmAction::DeleteProject(id) => {
                self.settings.projects.retain(|p| p.id != id);
                self.settings
                    .goals
                    .retain(|g| g.target != GoalTarget::Project(id.clone()));
            }
            ConfirmAction::ImportBackup(path) => {
                if let Some(report) = self.store_write(|store| store.import(&path)) {
                    self.toast(
                        format!(
                            "Imported: {} new, {} replaced, {} unreadable",
                            report.added, report.replaced, report.skipped
                        ),
                        false,
                    );
                }
            }
            ConfirmAction::ResetRules => {
                self.settings.rules = crate::settings::default_rules();
                self.toast("The starter rules were restored", false);
            }
        }
    }

    fn away_dialog(&mut self, ui: &mut Ui) -> bool {
        let p = pal(ui);
        let Some(r) = self.status.returned.first().copied() else {
            return true;
        };
        ui.set_width(470.0);
        title(ui, "Welcome back");
        let more = self.status.returned.len() - 1;
        para(
            ui,
            &format!(
                "You were away from {} to {} ({}). What were you doing?",
                timeutil::fmt_hm(r.start, &self.tz),
                timeutil::fmt_hm(r.end, &self.tz),
                timeutil::fmt_dur(r.end - r.start)
            ),
        );
        ui.add_space(10.0);
        let mut answer: Option<Option<AwayLabel>> = None;
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 74.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let w = (ui.available_width() - 30.0) / 4.0;
                for (icon, label, hint, value) in [
                    (
                        Icon::PEOPLE,
                        "Meeting",
                        "Counts as work",
                        Some(AwayLabel::Meeting),
                    ),
                    (Icon::PHONE, "Call", "Counts as work", Some(AwayLabel::Call)),
                    (Icon::COFFEE, "Break", "Not counted", Some(AwayLabel::Break)),
                    (Icon::DELETE, "Discard", "Forget it", None),
                ] {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 72.0), Sense::click());
                    let color = if value.is_none() { p.danger } else { p.accent };
                    ui.painter().rect(
                        rect,
                        egui::CornerRadius::same(12),
                        if resp.hovered() {
                            color.gamma_multiply(0.14)
                        } else {
                            p.bg
                        },
                        egui::Stroke::new(1.0_f32, if resp.hovered() { color } else { p.border }),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center_top() + vec2(0.0, 19.0),
                        egui::Align2::CENTER_CENTER,
                        icon.glyph(),
                        super::theme::regular(18.0),
                        color,
                    );
                    ui.painter().text(
                        rect.center_top() + vec2(0.0, 42.0),
                        egui::Align2::CENTER_CENTER,
                        label,
                        semibold(14.0),
                        p.text,
                    );
                    ui.painter().text(
                        rect.center_top() + vec2(0.0, 59.0),
                        egui::Align2::CENTER_CENTER,
                        hint,
                        super::theme::regular(11.5),
                        p.faint,
                    );
                    if resp.clicked() {
                        answer = Some(value);
                    }
                }
            },
        );
        ui.add_space(10.0);
        let mut later = false;
        widgets::button_row(ui, |ui| {
            if widgets::ghost_button(ui, "Ask me later").clicked() {
                later = true;
            }
            if widgets::ghost_button(ui, "Just away")
                .on_hover_text("Keep it as plain away time")
                .clicked()
            {
                answer = Some(Some(AwayLabel::None));
            }
            if more > 0 {
                ui.label(
                    RichText::new(format!(
                        "{more} more absence{} to go",
                        if more == 1 { "" } else { "s" }
                    ))
                    .color(p.faint),
                );
            }
        });
        if let Some(label) = answer {
            self.status.returned.remove(0);
            if self.control_available() {
                self.send(Cmd::LabelAway { id: r.id, label });
            } else if let Ok(mut status) = self.shared.status.lock() {
                status.returned.retain(|x| x.id != r.id);
            }
            return self.status.returned.is_empty();
        }
        if later {
            self.away_prompt = false;
            return true;
        }
        false
    }

    fn edit_dialog(&mut self, ui: &mut Ui, d: &mut EditDlg) -> bool {
        let p = pal(ui);
        ui.set_width(480.0);
        let manual_like = matches!(d.block.kind, Kind::Manual | Kind::Timer);
        title(
            ui,
            if d.is_new {
                "Add time"
            } else {
                match d.block.kind {
                    Kind::Active => "Edit tracked block",
                    Kind::Away => "Edit away time",
                    Kind::Manual => "Edit manual entry",
                    Kind::Timer => "Edit timer entry",
                    Kind::Focus => "Edit focus session",
                }
            },
        );
        if d.is_new {
            para(
                ui,
                "For work done away from the PC: a workshop, a site visit, reading on paper.",
            );
        } else if d.block.kind == Kind::Active {
            para(
                ui,
                &format!(
                    "{}  ·  {}",
                    d.block.app_label(),
                    if d.block.title.is_empty() {
                        "no title recorded"
                    } else {
                        &d.block.title
                    }
                ),
            );
        }
        ui.add_space(6.0);
        if d.is_new {
            row(ui, "Date", |ui| {
                text_field(ui, &mut d.date, "YYYY-MM-DD", 120.0);
            });
        }
        row(ui, "From / to", |ui| {
            text_field(ui, &mut d.start, "09:00", 64.0);
            ui.label(RichText::new("to").color(p.muted));
            text_field(ui, &mut d.end, "10:00", 64.0);
            ui.label(RichText::new("24-hour clock").size(12.0).color(p.faint));
        });
        if manual_like {
            row(
                ui,
                if d.block.kind == Kind::Timer {
                    "Task"
                } else {
                    "What"
                },
                |ui| {
                    text_field(ui, &mut d.title, "What did you work on?", 330.0);
                },
            );
        }
        if d.block.kind == Kind::Away {
            row(ui, "This was", |ui| {
                let options = [
                    AwayLabel::None,
                    AwayLabel::Meeting,
                    AwayLabel::Call,
                    AwayLabel::Break,
                ];
                let names: Vec<&str> = options.iter().map(|o| o.name()).collect();
                let current = options.iter().position(|o| *o == d.away).unwrap_or(0);
                if let Some(i) = widgets::segmented(ui, &names, current) {
                    d.away = options[i];
                }
            });
        }
        if !matches!(d.block.kind, Kind::Focus)
            && !(d.block.kind == Kind::Away && !d.away.counts_as_work())
        {
            if d.block.kind != Kind::Timer {
                row(ui, "Category", |ui| {
                    category_combo(ui, "edit-category", &self.settings, &mut d.category);
                });
            }
            row(ui, "Project", |ui| {
                let label = if d.block.kind == Kind::Timer {
                    "None"
                } else {
                    "By the rules"
                };
                project_combo(ui, "edit-project", &self.settings, &mut d.project, label);
            });
        }
        row(ui, "Note", |ui| {
            text_field(ui, &mut d.note, "Optional", 330.0);
        });
        row(ui, "Tags", |ui| {
            text_field(ui, &mut d.tags, "Separated by commas", 330.0);
        });
        error_line(ui, &d.error);
        ui.add_space(10.0);
        let mut save = false;
        let mut cancel = false;
        widgets::button_row(ui, |ui| {
            if widgets::primary_button(ui, if d.is_new { "Add" } else { "Save" }, true).clicked() {
                save = true;
            }
            if widgets::ghost_button(ui, "Cancel").clicked() {
                cancel = true;
            }
        });
        if cancel {
            return true;
        }
        if !save {
            return false;
        }
        // Validate and store.
        let day = if d.is_new {
            match timeutil::parse_iso_date(&d.date) {
                Some(day) => day,
                None => {
                    d.error = Some("The date must look like 2026-11-12.".into());
                    return false;
                }
            }
        } else {
            self.tz.day_of(d.block.start)
        };
        let (Some((sh, sm)), Some((eh, em))) =
            (timeutil::parse_hm(&d.start), timeutil::parse_hm(&d.end))
        else {
            d.error = Some("Times must look like 09:30.".into());
            return false;
        };
        let start = self.tz.at(day, sh, sm);
        let mut end = self.tz.at(day, eh, em);
        // Keep the seconds of an untouched end so that nothing is lost.
        if !d.is_new && timeutil::fmt_hm(d.block.end, &self.tz) == d.end.trim() {
            end = d.block.end;
        }
        let start = if !d.is_new && timeutil::fmt_hm(d.block.start, &self.tz) == d.start.trim() {
            d.block.start
        } else {
            start
        };
        if end <= start {
            d.error = Some("The end must be after the start (on the same day).".into());
            return false;
        }
        if end > self.now() + 60 {
            d.error = Some("That is in the future.".into());
            return false;
        }
        if end - start > edit::MAX_MANUAL_SECS {
            d.error = Some("A block can be at most 24 hours long.".into());
            return false;
        }
        let mut block = d.block.clone();
        if d.is_new {
            block.id = crate::model::new_id();
        }
        block.start = start;
        block.end = end;
        if manual_like {
            block.title = d.title.trim().to_string();
        }
        block.away = d.away;
        block.category = d.category.clone();
        block.project = d.project.clone();
        block.note = d.note.trim().to_string();
        block.tags = edit::parse_tags(&d.tags);
        if self.store_write(|store| store.put(&block)).is_some() {
            self.toast(if d.is_new { "Time added" } else { "Saved" }, false);
            if d.is_new {
                self.timeline.day = Some(day);
                self.timeline.highlight = Some(block.id);
                self.timeline.focus = Some(block.id);
            }
            return true;
        }
        false
    }

    fn split_dialog(&mut self, ui: &mut Ui, d: &mut SplitDlg) -> bool {
        let p = pal(ui);
        ui.set_width(420.0);
        title(ui, "Split block");
        para(
            ui,
            &format!(
                "This block runs from {} to {}. Where should it be cut? Both halves keep the details; you can then edit each.",
                timeutil::fmt_hm(d.block.start, &self.tz),
                timeutil::fmt_hm(d.block.end, &self.tz)
            ),
        );
        ui.add_space(6.0);
        row(ui, "Cut at", |ui| {
            text_field(ui, &mut d.at, "10:30", 64.0);
            ui.label(RichText::new("24-hour clock").size(12.0).color(p.faint));
        });
        error_line(ui, &d.error);
        ui.add_space(10.0);
        let mut go = false;
        let mut cancel = false;
        widgets::button_row(ui, |ui| {
            if widgets::primary_button(ui, &Icon::CUT.with("Split"), true).clicked() {
                go = true;
            }
            if widgets::ghost_button(ui, "Cancel").clicked() {
                cancel = true;
            }
        });
        if cancel {
            return true;
        }
        if !go {
            return false;
        }
        let Some((h, m)) = timeutil::parse_hm(&d.at) else {
            d.error = Some("The time must look like 10:30.".into());
            return false;
        };
        let at = self.tz.at(self.tz.day_of(d.block.start), h, m);
        match edit::split(&d.block, at) {
            Some((a, b)) => {
                if self
                    .store_write(|store| {
                        store.put(&a)?;
                        store.put(&b)
                    })
                    .is_some()
                {
                    self.toast("Block split in two", false);
                    return true;
                }
                false
            }
            None => {
                d.error = Some("The cut must lie inside the block.".into());
                false
            }
        }
    }

    fn export_dialog(&mut self, ui: &mut Ui, d: &mut ExportDlg) -> bool {
        let p = pal(ui);
        ui.set_width(480.0);
        title(ui, "Export");
        para(
            ui,
            "Writes a file where you choose. Nothing is uploaded anywhere.",
        );
        ui.add_space(6.0);
        row(ui, "What", |ui| {
            let current = usize::from(d.kind == ExportKind::Timesheet);
            if let Some(i) = widgets::segmented(ui, &["Activity report", "Timesheet"], current) {
                d.kind = if i == 1 {
                    ExportKind::Timesheet
                } else {
                    ExportKind::Report
                };
            }
        });
        row(ui, "From / to", |ui| {
            text_field(ui, &mut d.from, "YYYY-MM-DD", 110.0);
            ui.label(RichText::new("to").color(p.muted));
            text_field(ui, &mut d.to, "YYYY-MM-DD", 110.0);
        });
        row(ui, "Format", |ui| {
            let formats = [Format::Html, Format::Csv, Format::Json];
            let current = formats.iter().position(|f| *f == d.format).unwrap_or(0);
            if let Some(i) = widgets::segmented(ui, &["HTML", "CSV", "JSON"], current) {
                d.format = formats[i];
            }
        });
        if d.kind == ExportKind::Timesheet {
            row(ui, "Project", |ui| {
                let shown = match d.project.as_deref() {
                    None => "All projects".to_string(),
                    Some(id) => self.settings.project_name(id).to_string(),
                };
                egui::ComboBox::from_id_salt("export-project")
                    .selected_text(shown)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut d.project, None, "All projects");
                        for pr in &self.settings.projects {
                            ui.selectable_value(&mut d.project, Some(pr.id.clone()), &pr.name);
                        }
                    });
            });
            let t = &self.settings.timesheet;
            ui.label(
                RichText::new(format!(
                    "Rounding: {}{}. Change it on the Projects page.",
                    t.rounding.name().to_lowercase(),
                    if t.rounding == crate::model::Rounding::None {
                        String::new()
                    } else {
                        format!(", {} minutes", t.step_minutes)
                    }
                ))
                .size(12.0)
                .color(p.faint),
            );
        }
        error_line(ui, &d.error);
        ui.add_space(10.0);
        let mut go = false;
        let mut cancel = false;
        widgets::button_row(ui, |ui| {
            if widgets::primary_button(ui, &Icon::EXPORT.with("Save as…"), true).clicked() {
                go = true;
            }
            if widgets::ghost_button(ui, "Cancel").clicked() {
                cancel = true;
            }
        });
        if cancel {
            return true;
        }
        if !go {
            return false;
        }
        let (Some(from), Some(to)) = (
            timeutil::parse_iso_date(&d.from),
            timeutil::parse_iso_date(&d.to),
        ) else {
            d.error = Some("Dates must look like 2026-11-12.".into());
            return false;
        };
        if to < from {
            d.error = Some("The second date must not be before the first.".into());
            return false;
        }
        let label = if from == to {
            timeutil::fmt_day(from)
        } else {
            format!("{} to {}", timeutil::fmt_day(from), timeutil::fmt_day(to))
        };
        let (start, end) = (self.tz.day_start(from), self.tz.day_start(to + 1));
        let blocks = self.blocks(start, end);
        let input = export::Input {
            blocks: &blocks,
            settings: &self.settings,
            tz: &self.tz,
            from: start,
            to: end,
            label: &label,
        };
        let (text, kind) = match d.kind {
            ExportKind::Report => (export::report(&input, d.format), "report"),
            ExportKind::Timesheet => {
                let sheet = export::timesheet(&input, d.project.as_deref());
                (
                    export::timesheet_text(&sheet, &input, d.format),
                    "timesheet",
                )
            }
        };
        let name = export::default_file_name(
            kind,
            &format!("{}_{}", d.from.trim(), d.to.trim()),
            d.format,
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(name)
            .add_filter(d.format.extension().to_uppercase(), &[d.format.extension()])
            .save_file()
        else {
            return false;
        };
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.toast(format!("Saved {}", path.display()), false);
                true
            }
            Err(e) => {
                d.error = Some(format!("Could not write the file: {e}"));
                false
            }
        }
    }

    fn rule_dialog(&mut self, ui: &mut Ui, d: &mut RuleDlg) -> bool {
        ui.set_width(480.0);
        title(ui, "New rule");
        para(ui, "Time that matches goes into the category. Website rules win over title rules, which win over program rules.");
        ui.add_space(6.0);
        row(ui, "When the", |ui| {
            egui::ComboBox::from_id_salt("rule-field")
                .selected_text(d.field.name())
                .width(240.0)
                .show_ui(ui, |ui| {
                    for f in Field::ALL {
                        ui.selectable_value(&mut d.field, f, f.name());
                    }
                });
        });
        row(ui, "is", |ui| {
            let hint = match d.field {
                Field::Exe => "program.exe (a * matches anything)",
                Field::TitleContains => "text found in the window title",
                Field::TitleRegex => "regular expression",
                Field::Domain => "example.org",
            };
            text_field(ui, &mut d.pattern, hint, 330.0);
        });
        row(ui, "it counts as", |ui| {
            egui::ComboBox::from_id_salt("rule-category")
                .selected_text(self.settings.category_name(&d.category).to_string())
                .width(240.0)
                .show_ui(ui, |ui| {
                    for c in &self.settings.categories {
                        ui.selectable_value(&mut d.category, c.id.clone(), &c.name);
                    }
                });
        });
        error_line(ui, &d.error);
        ui.add_space(10.0);
        let mut go = false;
        let mut cancel = false;
        widgets::button_row(ui, |ui| {
            if widgets::primary_button(ui, "Add rule", true).clicked() {
                go = true;
            }
            if widgets::ghost_button(ui, "Cancel").clicked() {
                cancel = true;
            }
        });
        if cancel {
            return true;
        }
        if !go {
            return false;
        }
        let pattern = d.pattern.trim().to_string();
        if pattern.is_empty() {
            d.error = Some("Type what the rule should match.".into());
            return false;
        }
        if d.field == Field::TitleRegex {
            if let Err(e) = crate::rules::compile_regex(&pattern) {
                d.error = Some(format!("That is not a valid regular expression: {e}"));
                return false;
            }
        }
        self.settings.rules.insert(
            0,
            Rule {
                field: d.field,
                pattern,
                category: d.category.clone(),
                enabled: true,
            },
        );
        self.cats.selected = Some(d.category.clone());
        self.toast("Rule added", false);
        true
    }

    fn category_dialog(&mut self, ui: &mut Ui, d: &mut CategoryDlg) -> bool {
        ui.set_width(460.0);
        title(
            ui,
            if d.original.is_some() {
                "Edit category"
            } else {
                "New category"
            },
        );
        ui.add_space(4.0);
        row(ui, "Name", |ui| {
            text_field(ui, &mut d.name, "For example: Research", 300.0);
        });
        row(ui, "Colour", |ui| color_picker(ui, &mut d.color));
        row(ui, "Counts as", |ui| {
            let names: Vec<&str> = Score::ALL.iter().map(|s| s.name()).collect();
            let current = Score::ALL.iter().position(|s| *s == d.score).unwrap_or(1);
            if let Some(i) = widgets::segmented(ui, &names, current) {
                d.score = Score::ALL[i];
            }
        });
        error_line(ui, &d.error);
        ui.add_space(10.0);
        let mut go = false;
        let mut cancel = false;
        widgets::button_row(ui, |ui| {
            if widgets::primary_button(ui, "Save", true).clicked() {
                go = true;
            }
            if widgets::ghost_button(ui, "Cancel").clicked() {
                cancel = true;
            }
        });
        if cancel {
            return true;
        }
        if !go {
            return false;
        }
        let name = d.name.trim().to_string();
        if name.is_empty() {
            d.error = Some("The category needs a name.".into());
            return false;
        }
        let clash = self
            .settings
            .categories
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name) && Some(&c.id) != d.original.as_ref());
        if clash {
            d.error = Some("There is already a category with that name.".into());
            return false;
        }
        match &d.original {
            Some(id) => {
                if let Some(c) = self.settings.categories.iter_mut().find(|c| &c.id == id) {
                    c.name = name;
                    c.color = d.color;
                    c.score = d.score;
                }
            }
            None => {
                let taken: Vec<&str> = self
                    .settings
                    .categories
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect();
                let id = Settings::make_id(&name, &taken);
                // New categories go before "Other".
                let at = self.settings.categories.len().saturating_sub(1);
                self.settings.categories.insert(
                    at,
                    Category {
                        id: id.clone(),
                        name,
                        color: d.color,
                        score: d.score,
                        builtin: false,
                    },
                );
                self.cats.selected = Some(id);
            }
        }
        true
    }

    fn project_dialog(&mut self, ui: &mut Ui, d: &mut ProjectDlg) -> bool {
        let p = pal(ui);
        ui.set_width(500.0);
        title(
            ui,
            if d.original.is_some() {
                "Edit project"
            } else {
                "New project"
            },
        );
        ui.add_space(4.0);
        row(ui, "Name", |ui| {
            text_field(ui, &mut d.name, "Project name", 330.0);
        });
        row(ui, "Client", |ui| {
            text_field(ui, &mut d.client, "Optional", 330.0);
        });
        row(ui, "Colour", |ui| color_picker(ui, &mut d.color));
        row(ui, "Hourly rate", |ui| {
            text_field(ui, &mut d.rate, "0", 90.0);
            ui.label(
                RichText::new(format!(
                    "{} per hour, for timesheets",
                    self.settings.timesheet.currency
                ))
                .size(12.0)
                .color(p.faint),
            );
        });
        row(ui, "Keywords", |ui| {
            text_field(ui, &mut d.keywords, "Separated by commas", 330.0);
        });
        ui.label(
            RichText::new("Time whose window title or website contains a keyword belongs to this project: a folder name, a repository, a client's name, a ticket prefix.")
                .size(12.0)
                .color(p.faint),
        );
        error_line(ui, &d.error);
        ui.add_space(10.0);
        let mut go = false;
        let mut cancel = false;
        widgets::button_row(ui, |ui| {
            if widgets::primary_button(ui, "Save", true).clicked() {
                go = true;
            }
            if widgets::ghost_button(ui, "Cancel").clicked() {
                cancel = true;
            }
        });
        if cancel {
            return true;
        }
        if !go {
            return false;
        }
        let name = d.name.trim().to_string();
        if name.is_empty() {
            d.error = Some("The project needs a name.".into());
            return false;
        }
        let rate = if d.rate.trim().is_empty() {
            0.0
        } else {
            match d.rate.trim().replace(',', ".").parse::<f64>() {
                Ok(r) if (0.0..1_000_000.0).contains(&r) => r,
                _ => {
                    d.error = Some("The hourly rate must be a number.".into());
                    return false;
                }
            }
        };
        let keywords: Vec<String> = d
            .keywords
            .split(',')
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        match &d.original {
            Some(id) => {
                if let Some(pr) = self.settings.projects.iter_mut().find(|x| &x.id == id) {
                    pr.name = name;
                    pr.client = d.client.trim().to_string();
                    pr.color = d.color;
                    pr.rate = rate;
                    pr.keywords = keywords;
                }
            }
            None => {
                let taken: Vec<&str> = self
                    .settings
                    .projects
                    .iter()
                    .map(|x| x.id.as_str())
                    .collect();
                let id = Settings::make_id(&name, &taken);
                self.settings.projects.push(Project {
                    id,
                    name,
                    client: d.client.trim().to_string(),
                    color: d.color,
                    rate,
                    keywords,
                    archived: false,
                });
            }
        }
        true
    }

    fn goal_dialog(&mut self, ui: &mut Ui, d: &mut GoalDlg) -> bool {
        let p = pal(ui);
        ui.set_width(470.0);
        title(
            ui,
            if d.index.is_some() {
                "Edit goal"
            } else {
                "New goal or limit"
            },
        );
        ui.add_space(4.0);
        row(ui, "Kind", |ui| {
            if let Some(i) = widgets::segmented(
                ui,
                &["Goal: at least", "Limit: at most"],
                usize::from(d.limit),
            ) {
                d.limit = i == 1;
            }
        });
        row(ui, "For", |ui| {
            let shown = match &d.target {
                GoalTarget::Total => "All worked time".to_string(),
                GoalTarget::Category(id) => {
                    format!("Category: {}", self.settings.category_name(id))
                }
                GoalTarget::Project(id) => format!("Project: {}", self.settings.project_name(id)),
            };
            egui::ComboBox::from_id_salt("goal-target")
                .selected_text(shown)
                .width(260.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.target, GoalTarget::Total, "All worked time");
                    for c in &self.settings.categories {
                        ui.selectable_value(
                            &mut d.target,
                            GoalTarget::Category(c.id.clone()),
                            format!("Category: {}", c.name),
                        );
                    }
                    for pr in self.settings.projects.iter().filter(|x| !x.archived) {
                        ui.selectable_value(
                            &mut d.target,
                            GoalTarget::Project(pr.id.clone()),
                            format!("Project: {}", pr.name),
                        );
                    }
                });
        });
        row(ui, "Per day", |ui| {
            ui.add(
                egui::DragValue::new(&mut d.hours)
                    .range(0..=16)
                    .suffix(" h"),
            );
            ui.add(
                egui::DragValue::new(&mut d.minutes)
                    .range(0..=59)
                    .suffix(" min"),
            );
        });
        row(ui, "Notify", |ui| {
            if widgets::toggle(ui, d.alert, true).clicked() {
                d.alert = !d.alert;
            }
            ui.label(
                RichText::new(if d.limit {
                    "A gentle notice when the limit is reached"
                } else {
                    "A notice when the goal is reached"
                })
                .color(p.muted),
            );
        });
        ui.add_space(10.0);
        let mut go = false;
        let mut cancel = false;
        let total = d.hours * 60 + d.minutes;
        widgets::button_row(ui, |ui| {
            if widgets::primary_button(ui, "Save", total > 0).clicked() {
                go = true;
            }
            if widgets::ghost_button(ui, "Cancel").clicked() {
                cancel = true;
            }
        });
        if cancel {
            return true;
        }
        if !go {
            return false;
        }
        let goal = Goal {
            target: d.target.clone(),
            minutes: total,
            limit: d.limit,
            alert: d.alert,
        };
        match d.index {
            Some(i) if i < self.settings.goals.len() => self.settings.goals[i] = goal,
            _ => self.settings.goals.push(goal),
        }
        true
    }
}
