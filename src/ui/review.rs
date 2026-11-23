//! The Review page: programs and websites that no rule of their own has
//! sorted yet, most time first. One click on a category makes the rule, so
//! the same program never has to be sorted twice.

use super::app::{page_header, App};
use super::dialogs::Dialog;
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::aggregate::Data;
use crate::digest::{self, ReviewRow};
use crate::model::{Field, Rule};
use crate::timeutil;
use eframe::egui::{self, pos2, vec2, Rect, RichText, Sense, Ui};

/// Less time than this in the whole period is not worth a question.
const MIN_SECS: i64 = 5 * 60;
const ROW: f32 = 56.0;

pub struct ReviewState {
    /// How many days back the page looks.
    pub days: i32,
    key: Option<(u64, i32, i32)>,
    pub rows: Vec<ReviewRow>,
    /// Worked time behind the rows.
    total: i64,
}

impl Default for ReviewState {
    fn default() -> Self {
        ReviewState {
            days: 14,
            key: None,
            rows: Vec::new(),
            total: 0,
        }
    }
}

impl App {
    /// Finds what waits to be sorted. Cheap when nothing changed; also used
    /// for the count next to "Review" in the navigation.
    pub(super) fn review_load(&mut self) {
        let today = self.today();
        let key = (self.rev(), self.review.days, today);
        if self.review.key == Some(key) {
            return;
        }
        let (from, to) = (
            self.tz.day_start(today - self.review.days + 1),
            self.tz.day_start(today + 1),
        );
        let blocks = self.blocks(from, to);
        let spans = crate::spans::flatten(&blocks, from, to, &self.classifier);
        let data = Data {
            blocks: &blocks,
            spans: &spans,
            classifier: &self.classifier,
            tz: &self.tz,
        };
        self.review.rows = digest::uncategorised(&data, MIN_SECS);
        self.review.total = self.review.rows.iter().map(|r| r.secs).sum();
        self.review.key = Some(key);
    }

    /// "This program / website counts as that category from now on."
    fn review_assign(&mut self, row: &ReviewRow, category: &str) {
        let field = if row.is_site {
            Field::Domain
        } else {
            Field::Exe
        };
        // One rule per program or site: a new choice replaces the old one.
        self.settings
            .rules
            .retain(|r| !(r.field == field && r.pattern.eq_ignore_ascii_case(&row.key)));
        self.settings.rules.insert(
            0,
            Rule {
                field,
                pattern: row.key.clone(),
                category: category.to_string(),
                enabled: true,
            },
        );
        let name = self.settings.category_name(category).to_string();
        self.toast(format!("{} now counts as {name}", row.label), false);
    }

    pub(super) fn review_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        self.review_load();
        let days = self.review.days;
        let subtitle = if self.review.rows.is_empty() {
            format!("Nothing from the last {days} days is waiting to be sorted.")
        } else {
            format!(
                "{} of the last {days} days has no rule of its own. Pick a category once and it is remembered.",
                timeutil::fmt_dur(self.review.total)
            )
        };
        page_header(ui, "Review", &subtitle, |ui| {
            let options = [7, 14, 30];
            let current = options.iter().position(|d| *d == days).unwrap_or(1);
            if let Some(i) = widgets::segmented(ui, &["7 days", "14 days", "30 days"], current) {
                self.review.days = options[i];
            }
        });

        if self.review.rows.is_empty() {
            widgets::empty_state(
                ui,
                Icon::DONE,
                p.good,
                "Everything is sorted",
                "Every program and website with more than a few minutes has a rule. New ones show up here as you use them.",
            );
            return;
        }

        let mut assign: Option<(usize, String)> = None;
        let mut custom: Option<usize> = None;
        widgets::card_plain(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW, self.review.rows.len(), |ui, range| {
                    for index in range {
                        let row = &self.review.rows[index];
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), ROW),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let full = ui.max_rect();
                                ui.painter().hline(
                                    full.x_range(),
                                    full.bottom(),
                                    egui::Stroke::new(1.0_f32, p.border.gamma_multiply(0.5)),
                                );
                                // Icon.
                                let (icon, _) =
                                    ui.allocate_exact_size(vec2(44.0, 32.0), Sense::hover());
                                let icon = Rect::from_center_size(icon.center(), vec2(28.0, 28.0));
                                if row.is_site {
                                    ui.painter().circle_filled(
                                        icon.center(),
                                        14.0,
                                        p.good.gamma_multiply(0.16),
                                    );
                                    ui.painter().text(
                                        icon.center(),
                                        egui::Align2::CENTER_CENTER,
                                        Icon::GLOBE.glyph(),
                                        regular(15.0),
                                        p.good,
                                    );
                                } else {
                                    widgets::program_icon(
                                        ui,
                                        icon,
                                        &row.label,
                                        self.icons.get(&row.exe),
                                    );
                                }
                                // Name and what it is.
                                let text_w = (ui.available_width() - 400.0).max(140.0);
                                let (text, _) =
                                    ui.allocate_exact_size(vec2(text_w, ROW), Sense::hover());
                                let top = Rect::from_min_max(
                                    pos2(text.left(), text.top() + 8.0),
                                    pos2(text.right(), text.center().y + 2.0),
                                );
                                let bottom = Rect::from_min_max(
                                    pos2(text.left(), text.center().y),
                                    pos2(text.right(), text.bottom() - 8.0),
                                );
                                widgets::text_in(ui, top, &row.label, semibold(14.5), p.text);
                                let now_in = self
                                    .classifier
                                    .categories
                                    .get(row.category)
                                    .map(|c| c.name.as_str())
                                    .unwrap_or("Other");
                                let what = if row.is_site {
                                    format!("Website  ·  counts as {now_in} for now")
                                } else {
                                    format!("Program  ·  {}", row.key)
                                };
                                widgets::text_in(ui, bottom, &what, regular(12.0), p.muted);

                                // Right side, added right to left.
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(6.0);
                                        if widgets::icon_button(
                                            ui,
                                            Icon::EDIT,
                                            "Write the rule yourself: a title instead of the program, a wildcard…",
                                        )
                                        .clicked()
                                        {
                                            custom = Some(index);
                                        }
                                        ui.menu_button(
                                            RichText::new(Icon::TAG.with("Put in…")).color(p.text),
                                            |ui| {
                                                ui.set_min_width(210.0);
                                                for c in &self.settings.categories {
                                                    if c.id == crate::model::OTHER {
                                                        continue;
                                                    }
                                                    if widgets::menu_item(ui, &c.name).clicked() {
                                                        assign = Some((index, c.id.clone()));
                                                        ui.close_menu();
                                                    }
                                                }
                                                ui.separator();
                                                if ui
                                                    .button("Leave it in Other")
                                                    .on_hover_text(
                                                        "A rule for Other: it stops being asked about",
                                                    )
                                                    .clicked()
                                                {
                                                    assign = Some((
                                                        index,
                                                        crate::model::OTHER.to_string(),
                                                    ));
                                                    ui.close_menu();
                                                }
                                            },
                                        );
                                        ui.add_space(10.0);
                                        let (time, _) = ui
                                            .allocate_exact_size(vec2(120.0, ROW), Sense::hover());
                                        let top = Rect::from_min_max(
                                            pos2(time.left(), time.top() + 8.0),
                                            pos2(time.right(), time.center().y + 2.0),
                                        );
                                        let bottom = Rect::from_min_max(
                                            pos2(time.left(), time.center().y),
                                            pos2(time.right(), time.bottom() - 8.0),
                                        );
                                        widgets::text_in_right(
                                            ui,
                                            top,
                                            &timeutil::fmt_dur(row.secs),
                                            semibold(14.5),
                                            p.text,
                                        );
                                        widgets::text_in_right(
                                            ui,
                                            bottom,
                                            &format!(
                                                "{} block{}",
                                                row.blocks,
                                                if row.blocks == 1 { "" } else { "s" }
                                            ),
                                            regular(12.0),
                                            p.faint,
                                        );
                                    },
                                );
                            },
                        );
                    }
                });
        });

        if let Some((index, category)) = assign {
            if let Some(row) = self.review.rows.get(index).cloned() {
                self.review_assign(&row, &category);
            }
        }
        if let Some(row) = custom.and_then(|i| self.review.rows.get(i)) {
            let field = if row.is_site {
                Field::Domain
            } else {
                Field::Exe
            };
            let category = self
                .settings
                .categories
                .first()
                .map(|c| c.id.clone())
                .unwrap_or_default();
            self.dialog = Some(Dialog::rule(field, &row.key, &category));
        }
    }
}
