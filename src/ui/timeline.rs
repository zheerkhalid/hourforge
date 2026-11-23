//! The Timeline page: every recorded block of a day, with the tools to fix
//! it by hand (add offline time, edit, split, merge, delete, label absences).

use super::app::{page_header, App};
use super::dialogs::{ConfirmAction, Dialog};
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::edit;
use crate::engine::Cmd;
use crate::model::{AwayLabel, Block, BlockId, Kind};
use crate::timeutil::{self, Day};
use eframe::egui::{self, pos2, vec2, CornerRadius, Rect, RichText, Sense, Ui};
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Tracked,
    Away,
    Manual,
    Overlays,
}

pub struct TimelineState {
    pub day: Option<Day>,
    pub filter: Filter,
    pub checked: HashSet<BlockId>,
    /// Scroll to and highlight this block once.
    pub focus: Option<BlockId>,
    pub highlight: Option<BlockId>,
    /// What is typed in the search box; narrows the list.
    pub search: String,
    /// Put the keyboard in the search box on the next frame (Ctrl+F).
    pub focus_search: bool,
    cache_key: (Day, u64),
    blocks: Vec<Block>,
    /// Category index per block, by the rules as they are now.
    classes: Vec<(usize, Option<usize>)>,
    worked: i64,
}

impl Default for TimelineState {
    fn default() -> Self {
        TimelineState {
            day: None,
            filter: Filter::All,
            checked: HashSet::new(),
            focus: None,
            highlight: None,
            search: String::new(),
            focus_search: false,
            cache_key: (i32::MIN, u64::MAX),
            blocks: Vec::new(),
            classes: Vec::new(),
            worked: 0,
        }
    }
}

const ROW: f32 = 46.0;

impl App {
    fn timeline_load(&mut self, day: Day) {
        let key = (day, self.rev());
        if self.timeline.cache_key == key {
            return;
        }
        let (from, to) = self.tz.day_bounds(day);
        let blocks = self.blocks(from, to);
        let spans = crate::spans::flatten(&blocks, from, to, &self.classifier);
        self.timeline.worked = crate::spans::worked(&spans);
        self.timeline.classes = blocks
            .iter()
            .map(|b| {
                let c = self.classifier.classify(b);
                (c.category, c.project)
            })
            .collect();
        let alive: HashSet<BlockId> = blocks.iter().map(|b| b.id).collect();
        self.timeline.checked.retain(|id| alive.contains(id));
        self.timeline.blocks = blocks;
        self.timeline.cache_key = key;
    }

    pub(super) fn timeline_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let today = self.today();
        let day = *self.timeline.day.get_or_insert(today);
        self.timeline_load(day);

        let mut go: Option<Day> = None;
        let mut add = false;
        let subtitle = format!(
            "{}  ·  {} worked  ·  {} blocks",
            timeutil::fmt_day(day),
            timeutil::fmt_dur(self.timeline.worked),
            self.timeline.blocks.len()
        );
        page_header(ui, "Timeline", &subtitle, |ui| {
            if widgets::primary_button(ui, &Icon::ADD.with("Add time"), true)
                .on_hover_text("Add time you worked away from the PC")
                .clicked()
            {
                add = true;
            }
            ui.add_space(6.0);
            if widgets::icon_button(ui, Icon::FORWARD, "Next day").clicked() && day < today {
                go = Some(day + 1);
            }
            if widgets::ghost_button_enabled(ui, "Today", day != today).clicked() {
                go = Some(today);
            }
            if widgets::icon_button(ui, Icon::BACK, "Previous day").clicked() {
                go = Some(day - 1);
            }
        });
        if let Some(d) = go {
            self.timeline.day = Some(d);
            self.timeline.checked.clear();
            return;
        }
        if add {
            self.dialog = Some(Dialog::manual(day, self.now(), &self.tz));
        }

        // Filters and selection actions.
        ui.horizontal(|ui| {
            let count = |f: Filter| {
                self.timeline
                    .blocks
                    .iter()
                    .filter(|b| filter_matches(f, b))
                    .count()
            };
            for (f, label) in [
                (Filter::All, "All"),
                (Filter::Tracked, "Tracked"),
                (Filter::Away, "Away"),
                (Filter::Manual, "Manual"),
                (Filter::Overlays, "Timers & focus"),
            ] {
                let n = count(f);
                if widgets::chip(ui, label, Some(n), self.timeline.filter == f).clicked() {
                    self.timeline.filter = f;
                }
            }
            // Search, on the right.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !self.timeline.search.is_empty()
                    && widgets::icon_button(ui, Icon::CLOSE, "Clear the search").clicked()
                {
                    self.timeline.search.clear();
                }
                let resp = widgets::text_edit(
                    ui,
                    &mut self.timeline.search,
                    "Search this day: title, program, note, #tag",
                    290.0,
                )
                .on_hover_text("Ctrl+F. Every word must match; #word looks at tags only.");
                if std::mem::take(&mut self.timeline.focus_search) {
                    resp.request_focus();
                }
            });
        });
        ui.add_space(2.0);

        let filter = self.timeline.filter;
        let query = self.timeline.search.trim().to_string();
        let rows: Vec<usize> = self
            .timeline
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| filter_matches(filter, b))
            .filter(|(i, b)| {
                if query.is_empty() {
                    return true;
                }
                let (cat, project) = self.timeline.classes[*i];
                let category = self.classifier.categories.get(cat);
                let project = project.and_then(|p| self.classifier.projects.get(p));
                edit::matches_search(
                    b,
                    &query,
                    category.map(|c| c.name.as_str()).unwrap_or(""),
                    project.map(|p| p.name.as_str()).unwrap_or(""),
                )
            })
            .map(|(i, _)| i)
            .collect();
        if rows.is_empty() {
            if query.is_empty() {
                widgets::empty_state(
                    ui,
                    Icon::CLOCK,
                    p.accent,
                    "Nothing recorded",
                    "No time was recorded on this day, or nothing matches the filter. Use Add time for work done away from the PC.",
                );
            } else {
                widgets::empty_state(
                    ui,
                    Icon::SEARCH,
                    p.accent,
                    "Nothing found",
                    "No block of this day matches the search. It looks at titles, programs, websites, notes, tags, categories and projects.",
                );
            }
            return;
        }
        if !query.is_empty() {
            let secs: i64 = rows
                .iter()
                .map(|&i| &self.timeline.blocks[i])
                .filter(|b| b.is_work())
                .map(|b| b.secs())
                .sum();
            ui.label(
                RichText::new(format!(
                    "{} block{} found, {} of worked time",
                    rows.len(),
                    if rows.len() == 1 { "" } else { "s" },
                    timeutil::fmt_dur(secs)
                ))
                .size(12.5)
                .color(p.muted),
            );
        }

        self.timeline_selection_bar(ui);

        widgets::card_plain(ui, |ui| {
            // Column header.
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
            let cols = columns(rect);
            for (r, label) in [
                (cols.time, "Time"),
                (cols.dur, "Length"),
                (cols.app, "What"),
                (cols.title, "Window, task or note"),
                (cols.project, "Project"),
            ] {
                widgets::text_in(ui, r, label, regular(12.0), p.faint);
            }
            widgets::divider(ui);

            let focus_row = self
                .timeline
                .focus
                .take()
                .and_then(|id| rows.iter().position(|&i| self.timeline.blocks[i].id == id));
            let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(row) = focus_row {
                scroll = scroll.vertical_scroll_offset((row as f32 * ROW - 2.0 * ROW).max(0.0));
            }
            let mut todo: Option<RowAction> = None;
            ui.spacing_mut().item_spacing.y = 0.0;
            scroll.show_rows(ui, ROW, rows.len(), |ui, range| {
                for row in range {
                    let index = rows[row];
                    if let Some(action) = self.timeline_row(ui, index) {
                        todo = Some(action);
                    }
                }
            });
            if let Some(action) = todo {
                self.timeline_do(action);
            }
        });
    }

    fn timeline_selection_bar(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let n = self.timeline.checked.len();
        if n == 0 {
            return;
        }
        let mut merge = false;
        let mut delete = false;
        let mut clear = false;
        let mut tag = false;
        let mut project: Option<Option<String>> = None;
        let mut category: Option<Option<String>> = None;
        egui::Frame::new()
            .fill(p.selected)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{n} selected")).font(semibold(14.0)));
                    ui.add_space(8.0);
                    if widgets::ghost_button_enabled(ui, &Icon::MERGE.with("Merge"), n >= 2)
                        .on_hover_text(
                            "Join the selected blocks into one; gaps between them are filled",
                        )
                        .clicked()
                    {
                        merge = true;
                    }
                    ui.menu_button(Icon::BRIEFCASE.with("Project"), |ui| {
                        ui.set_min_width(200.0);
                        if ui.button("By the rules").clicked() {
                            project = Some(None);
                            ui.close_menu();
                        }
                        if ui.button("No project").clicked() {
                            project = Some(Some(String::new()));
                            ui.close_menu();
                        }
                        ui.separator();
                        for pr in self.settings.projects.iter().filter(|x| !x.archived) {
                            if widgets::menu_item(ui, &pr.name).clicked() {
                                project = Some(Some(pr.id.clone()));
                                ui.close_menu();
                            }
                        }
                    });
                    ui.menu_button(Icon::TAG.with("Category"), |ui| {
                        ui.set_min_width(200.0);
                        if ui.button("By the rules").clicked() {
                            category = Some(None);
                            ui.close_menu();
                        }
                        ui.separator();
                        for c in &self.settings.categories {
                            if widgets::menu_item(ui, &c.name).clicked() {
                                category = Some(Some(c.id.clone()));
                                ui.close_menu();
                            }
                        }
                    });
                    if widgets::ghost_button(ui, "# Tags")
                        .on_hover_text("Add tags to the selected blocks, or replace theirs")
                        .clicked()
                    {
                        tag = true;
                    }
                    if widgets::ghost_button(ui, &Icon::DELETE.with("Delete")).clicked() {
                        delete = true;
                    }
                    if widgets::ghost_button(ui, "Clear selection").clicked() {
                        clear = true;
                    }
                });
            });
        ui.add_space(2.0);
        let selected: Vec<Block> = self
            .timeline
            .blocks
            .iter()
            .filter(|b| self.timeline.checked.contains(&b.id))
            .cloned()
            .collect();
        if clear {
            self.timeline.checked.clear();
        }
        if tag {
            self.dialog = Some(Dialog::tags(selected.iter().map(|b| b.id).collect()));
        }
        if merge {
            match edit::merge(&selected) {
                Ok((merged, remove)) => {
                    let done = self.store_write(|store| {
                        for id in &remove {
                            store.delete(*id)?;
                        }
                        store.put(&merged)
                    });
                    if done.is_some() {
                        self.timeline.checked.clear();
                        self.toast(format!("Merged {} blocks", remove.len() + 1), false);
                    }
                }
                Err(e) => self.toast(e.text(), true),
            }
        }
        if delete {
            self.dialog = Some(Dialog::confirm(
                &format!("Delete {n} block{}?", if n == 1 { "" } else { "s" }),
                "The selected time is removed from the timeline and from every report. This cannot be undone.",
                "Delete",
                ConfirmAction::DeleteBlocks(selected.iter().map(|b| b.id).collect()),
            ));
        }
        if let Some(value) = project {
            let done = self.store_write(|store| {
                for b in &selected {
                    store.put(&Block {
                        project: value.clone(),
                        ..b.clone()
                    })?;
                }
                Ok(())
            });
            if done.is_some() {
                self.toast("Project updated", false);
            }
        }
        if let Some(value) = category {
            let done = self.store_write(|store| {
                for b in &selected {
                    store.put(&Block {
                        category: value.clone(),
                        ..b.clone()
                    })?;
                }
                Ok(())
            });
            if done.is_some() {
                self.toast("Category updated", false);
            }
        }
    }

    fn timeline_row(&mut self, ui: &mut Ui, index: usize) -> Option<RowAction> {
        let p = pal(ui);
        let b = &self.timeline.blocks[index];
        let (cat, project) = self.timeline.classes[index];
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        if !ui.is_rect_visible(rect) {
            return None;
        }
        let cols = columns(rect);
        let checked = self.timeline.checked.contains(&b.id);
        let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
        if self.timeline.highlight == Some(b.id) {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), p.selected);
        } else if checked {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), p.selected.gamma_multiply(0.6));
        } else if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), p.hover);
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0_f32, p.border.gamma_multiply(0.5)),
        );
        let mut action = None;

        // Checkbox.
        let box_resp = ui.interact(cols.check, ui.id().with(("check", b.id)), Sense::click());
        widgets::paint_checkbox(ui, cols.check, Some(checked), box_resp.hovered());
        if box_resp.clicked() {
            action = Some(RowAction::Toggle(b.id));
        }

        let overlay = b.is_overlay();
        let color = match b.kind {
            Kind::Away if !b.is_work() => p.faint,
            Kind::Timer => p.accent,
            Kind::Focus => p.info,
            _ => self
                .classifier
                .categories
                .get(cat)
                .map(|c| widgets::rgb(c.color))
                .unwrap_or(p.faint),
        };
        // Time and length.
        let time = format!(
            "{} – {}",
            timeutil::fmt_hm(b.start, &self.tz),
            timeutil::fmt_hm(b.end, &self.tz)
        );
        widgets::text_in(ui, cols.time, &time, regular(13.5), p.text);
        widgets::text_in(
            ui,
            cols.dur,
            &timeutil::fmt_dur(b.secs()),
            semibold(13.5),
            if b.is_work() { p.text } else { p.muted },
        );

        // Colour bar, icon, name.
        let bar = Rect::from_min_size(
            pos2(cols.app.left(), rect.top() + 9.0),
            vec2(4.0, ROW - 18.0),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), color);
        let icon_rect = Rect::from_center_size(
            pos2(cols.app.left() + 26.0, rect.center().y),
            vec2(24.0, 24.0),
        );
        match b.kind {
            Kind::Active => {
                widgets::program_icon(ui, icon_rect, b.app_label(), self.icons.get(&b.exe))
            }
            _ => {
                let glyph = match (b.kind, b.away) {
                    (Kind::Away, AwayLabel::Meeting) => Icon::PEOPLE,
                    (Kind::Away, AwayLabel::Call) => Icon::PHONE,
                    (Kind::Away, AwayLabel::Break) => Icon::COFFEE,
                    (Kind::Away, _) => Icon::CLOCK,
                    (Kind::Manual, _) => Icon::EDIT,
                    (Kind::Timer, _) => Icon::STOPWATCH,
                    _ => Icon::FOCUS,
                };
                ui.painter()
                    .circle_filled(icon_rect.center(), 12.0, color.gamma_multiply(0.18));
                ui.painter().text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph.glyph(),
                    regular(13.0),
                    color,
                );
            }
        }
        let name_rect = Rect::from_min_max(
            pos2(cols.app.left() + 46.0, rect.top()),
            pos2(cols.app.right() - 8.0, rect.bottom()),
        );
        let category = if overlay || (b.kind == Kind::Away && !b.is_work()) {
            String::new()
        } else {
            self.classifier
                .categories
                .get(cat)
                .map(|c| c.name.clone())
                .unwrap_or_default()
        };
        if category.is_empty() {
            widgets::text_in(ui, name_rect, b.app_label(), regular(14.0), p.text);
        } else {
            let top = Rect::from_min_max(
                name_rect.min,
                pos2(name_rect.right(), rect.center().y + 3.0),
            );
            let bottom =
                Rect::from_min_max(pos2(name_rect.left(), rect.center().y - 1.0), name_rect.max);
            widgets::text_in(
                ui,
                top.shrink2(vec2(0.0, 3.0)),
                b.app_label(),
                regular(14.0),
                p.text,
            );
            widgets::text_in(
                ui,
                bottom.shrink2(vec2(0.0, 4.0)),
                &category,
                regular(11.5),
                color,
            );
        }

        // Title / quick labels for unanswered absences.
        let unlabelled = b.kind == Kind::Away && b.away == AwayLabel::None;
        if unlabelled && hovered {
            let mut x = cols.title.left();
            for (label, value) in [
                ("Meeting", AwayLabel::Meeting),
                ("Call", AwayLabel::Call),
                ("Break", AwayLabel::Break),
            ] {
                let r = Rect::from_min_size(pos2(x, rect.center().y - 13.0), vec2(72.0, 26.0));
                let br = ui.interact(r, ui.id().with((label, b.id)), Sense::click());
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::same(13),
                    egui::Stroke::new(1.0_f32, if br.hovered() { p.accent } else { p.border }),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    regular(12.5),
                    if br.hovered() { p.accent } else { p.muted },
                );
                if br.clicked() {
                    action = Some(RowAction::Label(b.id, value));
                }
                x += 78.0;
            }
        } else {
            let mut text = b.title.clone();
            if text.is_empty() {
                text = match b.kind {
                    Kind::Away if unlabelled => "Away from the keyboard".into(),
                    Kind::Active => "No title recorded".into(),
                    _ => String::new(),
                };
            }
            let has_second = !b.domain.is_empty() || !b.note.is_empty() || !b.tags.is_empty();
            let title_color = if b.title.is_empty() { p.faint } else { p.text };
            if has_second {
                let mut second = b.domain.clone();
                if !b.note.is_empty() {
                    if !second.is_empty() {
                        second.push_str("  ·  ");
                    }
                    second.push_str(&b.note);
                }
                for tag in &b.tags {
                    if !second.is_empty() {
                        second.push_str("  ");
                    }
                    second.push('#');
                    second.push_str(tag);
                }
                let top = Rect::from_min_max(
                    pos2(cols.title.left(), rect.top() + 3.0),
                    pos2(cols.title.right(), rect.center().y + 2.0),
                );
                let bottom = Rect::from_min_max(
                    pos2(cols.title.left(), rect.center().y),
                    pos2(cols.title.right(), rect.bottom() - 4.0),
                );
                widgets::text_in(ui, top, &text, regular(13.5), title_color);
                widgets::text_in(ui, bottom, &second, regular(11.5), p.muted);
            } else {
                widgets::text_in(ui, cols.title, &text, regular(13.5), title_color);
            }
        }

        // Project.
        if let Some(pr) = project.and_then(|i| self.classifier.projects.get(i)) {
            let label = pr.name.clone();
            let c = widgets::rgb(pr.color);
            ui.painter()
                .circle_filled(pos2(cols.project.left() + 5.0, rect.center().y), 4.0, c);
            let r = Rect::from_min_max(
                pos2(cols.project.left() + 16.0, rect.top()),
                cols.project.max,
            );
            widgets::text_in(ui, r, &label, regular(12.5), p.muted);
        }

        // Row actions, shown on hover.
        if hovered {
            let mut x = cols.actions.right() - 30.0;
            for (icon, tip, act) in [
                (Icon::DELETE, "Delete", RowAction::Delete(b.id)),
                (Icon::CUT, "Split in two", RowAction::Split(b.id)),
                (Icon::EDIT, "Edit", RowAction::Edit(b.id)),
            ] {
                let r = Rect::from_min_size(pos2(x, rect.center().y - 14.0), vec2(28.0, 28.0));
                let br = ui
                    .interact(r, ui.id().with((tip, b.id)), Sense::click())
                    .on_hover_text(tip);
                if br.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(7), p.border);
                }
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    icon.glyph(),
                    regular(14.0),
                    if br.hovered() { p.text } else { p.muted },
                );
                if br.clicked() {
                    action = Some(act);
                }
                x -= 30.0;
            }
        }
        if action.is_none() && resp.double_clicked() {
            action = Some(RowAction::Edit(b.id));
        }
        action
    }

    fn timeline_do(&mut self, action: RowAction) {
        let find = |id: BlockId| self.timeline.blocks.iter().find(|b| b.id == id).cloned();
        match action {
            RowAction::Toggle(id) => {
                if !self.timeline.checked.remove(&id) {
                    self.timeline.checked.insert(id);
                }
            }
            RowAction::Edit(id) => {
                if let Some(b) = find(id) {
                    self.dialog = Some(Dialog::edit(&b, &self.tz));
                }
            }
            RowAction::Split(id) => {
                if let Some(b) = find(id) {
                    if b.secs() < 120 {
                        self.toast("This block is too short to split.", true);
                    } else {
                        self.dialog = Some(Dialog::split(&b, &self.tz));
                    }
                }
            }
            RowAction::Delete(id) => {
                self.dialog = Some(Dialog::confirm(
                    "Delete this block?",
                    "The time is removed from the timeline and from every report. This cannot be undone.",
                    "Delete",
                    ConfirmAction::DeleteBlocks(vec![id]),
                ));
            }
            RowAction::Label(id, label) => {
                let block = find(id);
                if self.control_available() {
                    self.send(Cmd::LabelAway {
                        id,
                        label: Some(label),
                    });
                }
                if let Some(mut b) = block {
                    b.away = label;
                    self.store_write(|store| store.put(&b));
                }
            }
        }
    }
}

enum RowAction {
    Toggle(BlockId),
    Edit(BlockId),
    Split(BlockId),
    Delete(BlockId),
    Label(BlockId, AwayLabel),
}

fn filter_matches(filter: Filter, b: &Block) -> bool {
    match filter {
        Filter::All => true,
        Filter::Tracked => b.kind == Kind::Active,
        Filter::Away => b.kind == Kind::Away,
        Filter::Manual => b.kind == Kind::Manual,
        Filter::Overlays => b.is_overlay(),
    }
}

struct Columns {
    check: Rect,
    time: Rect,
    dur: Rect,
    app: Rect,
    title: Rect,
    project: Rect,
    actions: Rect,
}

fn columns(rect: Rect) -> Columns {
    let x = rect.left();
    let w = rect.width();
    let col = |from: f32, to: f32| {
        Rect::from_min_max(
            pos2(x + from, rect.top()),
            pos2(x + to - 10.0, rect.bottom()),
        )
    };
    // Fixed columns left and right; the title takes what is left.
    let project_w = 170.0;
    let actions_w = 96.0;
    let title_from = 430.0;
    let title_to = (w - project_w - actions_w).max(title_from + 120.0);
    Columns {
        check: Rect::from_min_size(pos2(x + 6.0, rect.top()), vec2(24.0, rect.height())),
        time: col(40.0, 150.0),
        dur: col(150.0, 222.0),
        app: col(222.0, title_from),
        title: col(title_from, title_to),
        project: col(title_to, title_to + project_w),
        actions: Rect::from_min_max(pos2(x + w - actions_w, rect.top()), rect.max),
    }
}
