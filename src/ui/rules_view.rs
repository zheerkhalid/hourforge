//! The Categories page (categories, their rules, and programs to drag onto
//! them) and the Projects page (projects, clients, keywords, timesheets).

use super::app::{page_header, App};
use super::dialogs::{ConfirmAction, Dialog, ExportKind};
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::model::{Field, Kind, Rounding, Rule, Score, OTHER};
use crate::timeutil;
use eframe::egui::{self, pos2, vec2, CornerRadius, Rect, RichText, Sense, Stroke, Ui};
use std::collections::HashMap;

/// A program or website seen recently, for the drag list.
#[derive(Clone, PartialEq)]
pub struct Seen {
    pub is_site: bool,
    /// Program file name or domain: what a rule would match.
    pub key: String,
    pub label: String,
    pub secs: i64,
    /// Category index by the rules as they are now.
    pub category: usize,
}

#[derive(Default)]
pub struct CatsState {
    pub selected: Option<String>,
    pub only_unsorted: bool,
    seen_key: Option<u64>,
    pub seen: Vec<Seen>,
    /// Worked seconds per category over the same period.
    totals: Vec<i64>,
}

#[derive(Default)]
pub struct ProjectsState {
    key: Option<u64>,
    /// Worked seconds this month per project index.
    month: Vec<i64>,
    unassigned: i64,
}

const SEEN_DAYS: i32 = 30;

impl App {
    /// Programs and sites of the last 30 days with their time.
    pub(super) fn load_seen(&mut self) {
        let rev = self.rev();
        if self.cats.seen_key == Some(rev) {
            return;
        }
        let today = self.today();
        let (from, to) = (
            self.tz.day_start(today - SEEN_DAYS + 1),
            self.tz.day_start(today + 1),
        );
        let blocks = self.blocks(from, to);
        let spans = crate::spans::flatten(&blocks, from, to, &self.classifier);
        let mut programs: HashMap<&str, (i64, &str)> = HashMap::new();
        let mut sites: HashMap<&str, i64> = HashMap::new();
        let mut totals = vec![0i64; self.classifier.categories.len()];
        for s in spans.iter().filter(|s| s.work) {
            if let Some(t) = totals.get_mut(s.category as usize) {
                *t += s.secs();
            }
            let b = &blocks[s.src as usize];
            if b.kind != Kind::Active || b.exe.is_empty() {
                continue;
            }
            let entry = programs
                .entry(b.exe.as_str())
                .or_insert((0, b.app.as_str()));
            entry.0 += s.secs();
            if !b.domain.is_empty() {
                *sites.entry(b.domain.as_str()).or_insert(0) += s.secs();
            }
        }
        let mut seen: Vec<Seen> = programs
            .into_iter()
            .map(|(exe, (secs, app))| Seen {
                is_site: false,
                key: exe.to_string(),
                label: if app.is_empty() {
                    exe.to_string()
                } else {
                    app.to_string()
                },
                secs,
                category: self.classifier.category_for(exe, "", ""),
            })
            .chain(sites.into_iter().map(|(domain, secs)| Seen {
                is_site: true,
                key: domain.to_string(),
                label: domain.to_string(),
                secs,
                // What the site would be without its browser's program rule.
                category: self.classifier.category_for("", "", domain),
            }))
            .filter(|s| s.secs >= 60)
            .collect();
        seen.sort_by(|a, b| b.secs.cmp(&a.secs).then_with(|| a.key.cmp(&b.key)));
        self.cats.seen = seen;
        self.cats.totals = totals;
        self.cats.seen_key = Some(rev);
    }

    pub(super) fn categories_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        self.load_seen();
        let mut new_rule = false;
        let mut new_category = false;
        let mut reset_rules = false;
        page_header(
            ui,
            "Categories",
            "Rules decide where time goes. Drag a program or site onto a category to make a rule.",
            |ui| {
                if widgets::primary_button(ui, &Icon::ADD.with("New rule"), true).clicked() {
                    new_rule = true;
                }
                if widgets::ghost_button(ui, "New category").clicked() {
                    new_category = true;
                }
                if widgets::icon_button(ui, Icon::UNDO, "Restore the starter rules").clicked() {
                    reset_rules = true;
                }
            },
        );
        let selected = self
            .cats
            .selected
            .clone()
            .filter(|id| self.settings.category(id).is_some())
            .unwrap_or_else(|| self.settings.categories[0].id.clone());
        self.cats.selected = Some(selected.clone());
        if new_rule {
            self.dialog = Some(Dialog::rule(Field::Exe, "", &selected));
        }
        if reset_rules {
            self.dialog = Some(Dialog::confirm(
                "Restore the starter rules?",
                "Every rule is replaced by the rules HourForge came with. Your categories stay, and recorded time is not touched.",
                "Restore rules",
                ConfirmAction::ResetRules,
            ));
        }
        if new_category {
            self.dialog = Some(Dialog::category(None, &self.settings));
        }

        let height = ui.available_height();
        let width = ui.available_width();
        let (left_w, right_w) = (270.0, 290.0);
        let mid_w = width - left_w - right_w - 24.0;
        let mut dropped: Option<(Seen, String)> = None;
        let mut select: Option<String> = None;
        let mut edit_category: Option<String> = None;
        let mut delete_category: Option<String> = None;

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            // Categories: drop targets.
            ui.allocate_ui_with_layout(
                vec2(left_w, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    widgets::card_plain(ui, |ui| {
                        ui.set_min_height(height - 26.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let total: i64 = self.cats.totals.iter().sum::<i64>().max(1);
                        for (index, c) in self.settings.categories.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 50.0),
                                Sense::click(),
                            );
                            let active = c.id == selected;
                            let hovering_drop = resp.dnd_hover_payload::<Seen>().is_some();
                            if hovering_drop {
                                ui.painter().rect(
                                    rect,
                                    CornerRadius::same(9),
                                    p.accent.gamma_multiply(0.16),
                                    Stroke::new(1.5_f32, p.accent),
                                    egui::StrokeKind::Inside,
                                );
                            } else if active {
                                ui.painter()
                                    .rect_filled(rect, CornerRadius::same(9), p.selected);
                            } else if resp.hovered() {
                                ui.painter()
                                    .rect_filled(rect, CornerRadius::same(9), p.hover);
                            }
                            let color = widgets::rgb(c.color);
                            ui.painter().circle_filled(
                                rect.left_center() + vec2(16.0, -7.0),
                                6.0,
                                color,
                            );
                            let name_rect = Rect::from_min_max(
                                pos2(rect.left() + 32.0, rect.top() + 6.0),
                                pos2(rect.right() - 62.0, rect.top() + 28.0),
                            );
                            widgets::text_in(
                                ui,
                                name_rect,
                                &c.name,
                                if active {
                                    semibold(14.5)
                                } else {
                                    regular(14.5)
                                },
                                p.text,
                            );
                            let secs = self.cats.totals.get(index).copied().unwrap_or(0);
                            widgets::text_in_right(
                                ui,
                                Rect::from_min_max(
                                    pos2(rect.right() - 70.0, rect.top() + 6.0),
                                    pos2(rect.right() - 10.0, rect.top() + 28.0),
                                ),
                                &timeutil::fmt_dur(secs),
                                regular(12.5),
                                p.muted,
                            );
                            let (score_color, score) = score_style(&p, c.score);
                            let rules = self
                                .settings
                                .rules
                                .iter()
                                .filter(|r| r.category == c.id)
                                .count();
                            widgets::text_in(
                                ui,
                                Rect::from_min_max(
                                    pos2(rect.left() + 32.0, rect.top() + 27.0),
                                    pos2(rect.right() - 10.0, rect.bottom() - 4.0),
                                ),
                                &format!(
                                    "{score}  ·  {rules} rule{}",
                                    if rules == 1 { "" } else { "s" }
                                ),
                                regular(11.5),
                                score_color,
                            );
                            // Share bar.
                            let bar = Rect::from_min_size(
                                pos2(rect.left() + 10.0, rect.bottom() - 3.0),
                                vec2((rect.width() - 20.0) * (secs as f32 / total as f32), 2.0),
                            );
                            ui.painter().rect_filled(bar, CornerRadius::same(1), color);
                            if let Some(seen) = resp.dnd_release_payload::<Seen>() {
                                dropped = Some(((*seen).clone(), c.id.clone()));
                            }
                            if resp.clicked() {
                                select = Some(c.id.clone());
                            }
                            if resp.double_clicked() {
                                edit_category = Some(c.id.clone());
                            }
                        }
                    });
                },
            );

            // Rules of the selected category.
            ui.allocate_ui_with_layout(
                vec2(mid_w, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    widgets::card_plain(ui, |ui| {
                        ui.set_min_height(height - 26.0);
                        let cat = self.settings.category(&selected).cloned();
                        let Some(cat) = cat else { return };
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), 34.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                widgets::dot(ui, widgets::rgb(cat.color));
                                ui.label(RichText::new(&cat.name).font(semibold(16.0)));
                                let (score_color, score) = score_style(&p, cat.score);
                                widgets::badge(ui, score, score_color);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if !cat.builtin
                                            && widgets::icon_button(
                                                ui,
                                                Icon::DELETE,
                                                "Delete category",
                                            )
                                            .clicked()
                                        {
                                            delete_category = Some(cat.id.clone());
                                        }
                                        if widgets::icon_button(
                                            ui,
                                            Icon::EDIT,
                                            "Rename, recolour, change how it counts",
                                        )
                                        .clicked()
                                        {
                                            edit_category = Some(cat.id.clone());
                                        }
                                    },
                                );
                            },
                        );
                        widgets::divider(ui);
                        self.rules_list(ui, &cat.id);
                    });
                },
            );

            // Programs and sites to drag.
            ui.allocate_ui_with_layout(
                vec2(right_w, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    widgets::card_plain(ui, |ui| {
                        ui.set_min_height(height - 26.0);
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), 34.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(RichText::new("Seen in 30 days").font(semibold(15.0)));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if widgets::chip(
                                            ui,
                                            "Unsorted",
                                            None,
                                            self.cats.only_unsorted,
                                        )
                                        .on_hover_text("Only show what has no rule yet")
                                        .clicked()
                                        {
                                            self.cats.only_unsorted = !self.cats.only_unsorted;
                                        }
                                    },
                                );
                            },
                        );
                        widgets::divider(ui);
                        let other = self.classifier.other();
                        let only = self.cats.only_unsorted;
                        let rows: Vec<&Seen> = self
                            .cats
                            .seen
                            .iter()
                            .filter(|s| !only || s.category == other)
                            .collect();
                        if rows.is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(if only {
                                        "Everything seen has a rule."
                                    } else {
                                        "Nothing recorded yet."
                                    })
                                    .color(p.muted),
                                );
                            });
                            return;
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("seen")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                for s in rows {
                                    let id = egui::Id::new(("seen", s.is_site, &s.key));
                                    let w = ui.available_width();
                                    ui.dnd_drag_source(id, s.clone(), |ui| {
                                        let (rect, _) =
                                            ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
                                        let icon = Rect::from_center_size(
                                            rect.left_center() + vec2(16.0, 0.0),
                                            vec2(22.0, 22.0),
                                        );
                                        if s.is_site {
                                            ui.painter().text(
                                                icon.center(),
                                                egui::Align2::CENTER_CENTER,
                                                Icon::GLOBE.glyph(),
                                                regular(15.0),
                                                p.muted,
                                            );
                                        } else {
                                            widgets::program_icon(
                                                ui,
                                                icon,
                                                &s.label,
                                                self.icons.get(&s.key),
                                            );
                                        }
                                        let color = self
                                            .classifier
                                            .categories
                                            .get(s.category)
                                            .map(|c| widgets::rgb(c.color))
                                            .unwrap_or(p.faint);
                                        ui.painter().circle_filled(
                                            pos2(rect.right() - 66.0, rect.center().y),
                                            4.0,
                                            if s.category == other { p.border } else { color },
                                        );
                                        widgets::text_in(
                                            ui,
                                            Rect::from_min_max(
                                                pos2(rect.left() + 36.0, rect.top()),
                                                pos2(rect.right() - 78.0, rect.bottom()),
                                            ),
                                            &s.label,
                                            regular(13.5),
                                            p.text,
                                        );
                                        widgets::text_in_right(
                                            ui,
                                            Rect::from_min_max(
                                                pos2(rect.right() - 58.0, rect.top()),
                                                pos2(rect.right() - 4.0, rect.bottom()),
                                            ),
                                            &timeutil::fmt_dur(s.secs),
                                            regular(12.5),
                                            p.muted,
                                        );
                                    });
                                }
                            });
                    });
                },
            );
        });

        if let Some(id) = select {
            self.cats.selected = Some(id);
        }
        if let Some((seen, category)) = dropped {
            let field = if seen.is_site {
                Field::Domain
            } else {
                Field::Exe
            };
            // One rule per program: a new drop replaces the old one.
            self.settings
                .rules
                .retain(|r| !(r.field == field && r.pattern.eq_ignore_ascii_case(&seen.key)));
            self.settings.rules.insert(
                0,
                Rule {
                    field,
                    pattern: seen.key.clone(),
                    category: category.clone(),
                    enabled: true,
                },
            );
            let name = self.settings.category_name(&category).to_string();
            self.cats.selected = Some(category);
            self.toast(format!("{} now counts as {name}", seen.label), false);
        }
        if let Some(id) = edit_category {
            self.dialog = Some(Dialog::category(
                self.settings.category(&id),
                &self.settings,
            ));
        }
        if let Some(id) = delete_category {
            let name = self.settings.category_name(&id).to_string();
            self.dialog = Some(Dialog::confirm(
                &format!("Delete the category {name}?"),
                "Its rules and goals are deleted too. Time that was in it falls back to the other rules. Recorded time itself is not touched.",
                "Delete category",
                ConfirmAction::DeleteCategory(id),
            ));
        }
    }

    fn rules_list(&mut self, ui: &mut Ui, category: &str) {
        let p = pal(ui);
        let indexes: Vec<usize> = self
            .settings
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.category == category)
            .map(|(i, _)| i)
            .collect();
        if indexes.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No rules yet").font(semibold(15.0)));
                ui.scope(|ui| {
                    ui.set_max_width(300.0);
                    ui.label(
                        RichText::new(if category == OTHER {
                            "Other is where time lands when no rule matches."
                        } else {
                            "Drag a program here from the list on the right, or add a rule by hand."
                        })
                        .color(p.muted),
                    );
                });
            });
            return;
        }
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let mut toggle: Option<usize> = None;
        egui::ScrollArea::vertical().id_salt("rules")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (pos, &i) in indexes.iter().enumerate() {
                    let rule = &self.settings.rules[i];
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
                    let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
                    if hovered {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), p.hover);
                    }
                    let (kind, color) = match rule.field {
                        Field::Exe => ("Program", p.info),
                        Field::TitleContains => ("Title has", p.accent),
                        Field::TitleRegex => ("Title regex", p.warn),
                        Field::Domain => ("Website", p.good),
                    };
                    let badge_rect = Rect::from_min_size(
                        pos2(rect.left() + 8.0, rect.center().y - 10.0),
                        vec2(84.0, 20.0),
                    );
                    widgets::paint_badge(ui, badge_rect, kind, color);
                    widgets::text_in(
                        ui,
                        Rect::from_min_max(
                            pos2(rect.left() + 104.0, rect.top()),
                            pos2(rect.right() - 150.0, rect.bottom()),
                        ),
                        &rule.pattern,
                        regular(13.5),
                        if rule.enabled { p.text } else { p.faint },
                    );
                    // Controls: switch always, the rest on hover.
                    let mut x = rect.right() - 46.0;
                    let switch = Rect::from_min_size(pos2(x, rect.center().y - 11.0), vec2(38.0, 22.0));
                    let sr = widgets::toggle_at(ui, switch, ui.id().with(("rule-on", i)), rule.enabled);
                    if sr.clicked() {
                        toggle = Some(i);
                    }
                    if hovered {
                        for (icon, tip, what) in [
                            (Icon::DELETE, "Delete rule", 0),
                            (Icon::DOWN, "Lower priority", 1),
                            (Icon::UP, "Higher priority", 2),
                        ] {
                            x -= 30.0;
                            let r = Rect::from_min_size(
                                pos2(x, rect.center().y - 14.0),
                                vec2(28.0, 28.0),
                            );
                            let br = ui
                                .interact(r, ui.id().with((tip, i)), Sense::click())
                                .on_hover_text(tip);
                            if br.hovered() {
                                ui.painter().rect_filled(r, CornerRadius::same(7), p.border);
                            }
                            ui.painter().text(
                                r.center(),
                                egui::Align2::CENTER_CENTER,
                                icon.glyph(),
                                regular(13.0),
                                if br.hovered() { p.text } else { p.muted },
                            );
                            if br.clicked() {
                                match what {
                                    0 => remove = Some(i),
                                    1 if pos + 1 < indexes.len() => {
                                        swap = Some((i, indexes[pos + 1]))
                                    }
                                    2 if pos > 0 => swap = Some((i, indexes[pos - 1])),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Website rules win over title rules, which win over program rules. Within a group the rule higher in its list wins.",
                    )
                    .size(12.0)
                    .color(p.faint),
                );
            });
        if let Some(i) = toggle {
            self.settings.rules[i].enabled = !self.settings.rules[i].enabled;
        }
        if let Some((a, b)) = swap {
            self.settings.rules.swap(a, b);
        }
        if let Some(i) = remove {
            self.settings.rules.remove(i);
        }
    }

    // ----------------------------------------------------------------------------------
    // Projects
    // ----------------------------------------------------------------------------------

    fn load_project_totals(&mut self) {
        let rev = self.rev();
        if self.projects.key == Some(rev) {
            return;
        }
        let today = self.today();
        let (from, to) = (
            self.tz.day_start(timeutil::month_start(today)),
            self.tz.day_start(today + 1),
        );
        let blocks = self.blocks(from, to);
        let spans = crate::spans::flatten(&blocks, from, to, &self.classifier);
        let mut month = vec![0i64; self.classifier.projects.len()];
        let mut unassigned = 0;
        for s in spans.iter().filter(|s| s.work) {
            match s.project.and_then(|i| month.get_mut(i as usize)) {
                Some(total) => *total += s.secs(),
                None => unassigned += s.secs(),
            }
        }
        self.projects.month = month;
        self.projects.unassigned = unassigned;
        self.projects.key = Some(rev);
    }

    pub(super) fn projects_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        self.load_project_totals();
        let today = self.today();
        let month_from = timeutil::month_start(today);
        let mut new_project = false;
        let mut timesheet: Option<Option<String>> = None;
        page_header(
            ui,
            "Projects",
            "Time goes to a project by keywords in window titles, by a running timer, or by hand in the timeline.",
            |ui| {
                if widgets::primary_button(ui, &Icon::ADD.with("New project"), true).clicked() {
                    new_project = true;
                }
                if widgets::ghost_button(ui, &Icon::EXPORT.with("Timesheet…")).clicked() {
                    timesheet = Some(None);
                }
            },
        );
        if new_project {
            self.dialog = Some(Dialog::project(None, &self.settings));
        }

        let mut edit: Option<String> = None;
        let mut delete: Option<String> = None;
        let mut archive: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Timesheet rules.
                widgets::card(ui, |ui| {
                    widgets::section(ui, "Timesheet rounding");
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), 36.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let names: Vec<&str> = Rounding::ALL.iter().map(|r| r.name()).collect();
                            let current = Rounding::ALL
                                .iter()
                                .position(|r| *r == self.settings.timesheet.rounding)
                                .unwrap_or(0);
                            if let Some(i) = widgets::segmented(ui, &names, current) {
                                self.settings.timesheet.rounding = Rounding::ALL[i];
                            }
                            ui.add_space(10.0);
                            let enabled = self.settings.timesheet.rounding != Rounding::None;
                            ui.label(RichText::new("to").color(p.muted));
                            ui.add_enabled_ui(enabled, |ui| {
                                egui::ComboBox::from_id_salt("round-step")
                                    .selected_text(format!(
                                        "{} minutes",
                                        self.settings.timesheet.step_minutes
                                    ))
                                    .width(110.0)
                                    .show_ui(ui, |ui| {
                                        for step in [5, 6, 10, 15, 30, 60] {
                                            ui.selectable_value(
                                                &mut self.settings.timesheet.step_minutes,
                                                step,
                                                format!("{step} minutes"),
                                            );
                                        }
                                    });
                            });
                            ui.add_space(10.0);
                            ui.label(RichText::new("Currency").color(p.muted));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.settings.timesheet.currency)
                                    .desired_width(56.0)
                                    .char_limit(6)
                                    .margin(egui::Margin::symmetric(8, 6)),
                            );
                        },
                    );
                    ui.label(
                        RichText::new(
                            "Each day's time per project is rounded on the timesheet; the timeline and the reports always show exact time.",
                        )
                        .size(12.0)
                        .color(p.faint),
                    );
                });
                ui.add_space(4.0);

                if self.settings.projects.is_empty() {
                    widgets::empty_state(
                        ui,
                        Icon::BRIEFCASE,
                        p.accent,
                        "No projects yet",
                        "Make a project and give it keywords, such as a folder or repository name that shows up in window titles. Time is then assigned by itself.",
                    );
                    return;
                }
                let currency = self.settings.timesheet.currency.clone();
                for (index, pr) in self.settings.projects.iter().enumerate() {
                    let secs = self.projects.month.get(index).copied().unwrap_or(0);
                    widgets::card(ui, |ui| {
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), 52.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                                let color = widgets::rgb(pr.color);
                                ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(10),
                                    color.gamma_multiply(if pr.archived { 0.08 } else { 0.18 }),
                                );
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    Icon::BRIEFCASE.glyph(),
                                    regular(17.0),
                                    if pr.archived { p.faint } else { color },
                                );
                                ui.add_space(4.0);
                                ui.allocate_ui_with_layout(
                                    vec2(330.0, 48.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&pr.name)
                                                        .font(semibold(15.5))
                                                        .color(if pr.archived {
                                                            p.muted
                                                        } else {
                                                            p.text
                                                        }),
                                                )
                                                .truncate(),
                                            );
                                            if pr.archived {
                                                widgets::badge(ui, "Archived", p.faint);
                                            }
                                        });
                                        let mut line = if pr.client.is_empty() {
                                            "No client".to_string()
                                        } else {
                                            pr.client.clone()
                                        };
                                        if pr.rate > 0.0 {
                                            line.push_str(&format!(
                                                "  ·  {currency} {:.2} / hour",
                                                pr.rate
                                            ));
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(line).size(12.5).color(p.muted),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                                // Keywords.
                                ui.allocate_ui_with_layout(
                                    vec2((ui.available_width() - 330.0).max(60.0), 48.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 5.0;
                                        if pr.keywords.is_empty() {
                                            ui.label(
                                                RichText::new("No keywords: timer or by hand only")
                                                    .size(12.0)
                                                    .color(p.faint),
                                            );
                                        }
                                        let mut used = 0.0;
                                        let max = ui.available_width();
                                        for (i, k) in pr.keywords.iter().enumerate() {
                                            let est = k.chars().count() as f32 * 7.0 + 22.0;
                                            if used + est > max - 40.0 {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "+{}",
                                                        pr.keywords.len() - i
                                                    ))
                                                    .size(12.0)
                                                    .color(p.faint),
                                                );
                                                break;
                                            }
                                            used += est;
                                            widgets::badge(ui, k, p.info);
                                        }
                                    },
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if widgets::icon_button(ui, Icon::DELETE, "Delete project")
                                            .clicked()
                                        {
                                            delete = Some(pr.id.clone());
                                        }
                                        if widgets::icon_button(
                                            ui,
                                            Icon::HISTORY,
                                            if pr.archived {
                                                "Bring back from the archive"
                                            } else {
                                                "Archive: keep the history, stop matching new time"
                                            },
                                        )
                                        .clicked()
                                        {
                                            archive = Some(pr.id.clone());
                                        }
                                        if widgets::icon_button(ui, Icon::EXPORT, "Timesheet for this project")
                                            .clicked()
                                        {
                                            timesheet = Some(Some(pr.id.clone()));
                                        }
                                        if widgets::icon_button(ui, Icon::EDIT, "Edit project").clicked()
                                        {
                                            edit = Some(pr.id.clone());
                                        }
                                        ui.add_space(8.0);
                                        ui.vertical(|ui| {
                                            ui.spacing_mut().item_spacing.y = 0.0;
                                            ui.with_layout(
                                                egui::Layout::top_down(egui::Align::Max),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(timeutil::fmt_dur(secs))
                                                            .font(semibold(16.0)),
                                                    );
                                                    let sub = if pr.rate > 0.0 {
                                                        format!(
                                                            "this month  ·  {currency} {:.2}",
                                                            secs as f64 / 3600.0 * pr.rate
                                                        )
                                                    } else {
                                                        "this month".to_string()
                                                    };
                                                    ui.label(
                                                        RichText::new(sub)
                                                            .size(12.0)
                                                            .color(p.muted),
                                                    );
                                                },
                                            );
                                        });
                                    },
                                );
                            },
                        );
                    });
                }
                ui.add_space(2.0);
                ui.label(
                    RichText::new(format!(
                        "{} of worked time this month has no project.",
                        timeutil::fmt_dur(self.projects.unassigned)
                    ))
                    .color(p.muted),
                );
            });

        if let Some(project) = timesheet {
            self.dialog = Some(Dialog::export(
                month_from,
                today,
                ExportKind::Timesheet,
                project,
            ));
        }
        if let Some(id) = edit {
            self.dialog = Some(Dialog::project(self.settings.project(&id), &self.settings));
        }
        if let Some(id) = archive {
            if let Some(pr) = self.settings.projects.iter_mut().find(|x| x.id == id) {
                pr.archived = !pr.archived;
            }
        }
        if let Some(id) = delete {
            let name = self.settings.project_name(&id).to_string();
            self.dialog = Some(Dialog::confirm(
                &format!("Delete the project {name}?"),
                "Its keywords and goals are deleted. Time assigned to it by hand shows as having no project. Recorded time itself is not touched.",
                "Delete project",
                ConfirmAction::DeleteProject(id),
            ));
        }
    }
}

pub fn score_style(p: &super::theme::Palette, score: Score) -> (egui::Color32, &'static str) {
    match score {
        Score::Productive => (p.good, "Productive"),
        Score::Neutral => (p.muted, "Neutral"),
        Score::Distracting => (p.danger, "Distracting"),
    }
}
