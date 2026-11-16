//! The Privacy page: what is recorded, what never is, pausing, redaction
//! and deleting recorded data.

use super::app::{page_header, App};
use super::dialogs::{ConfirmAction, Dialog};
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::engine::Cmd;
use crate::model::{DomainSource, PrivateApp, PrivateMode, Redaction, TrackState};
use crate::rules::PrivacyFilter;
use crate::timeutil;
use eframe::egui::{self, vec2, RichText, Sense, Ui};

pub struct PrivacyState {
    new_app: String,
    new_pattern: String,
    new_replace: String,
    pattern_error: Option<String>,
    try_text: String,
    from: String,
    to: String,
    range_error: Option<String>,
}

impl Default for PrivacyState {
    fn default() -> Self {
        PrivacyState {
            new_app: String::new(),
            new_pattern: String::new(),
            new_replace: crate::model::default_mask(),
            pattern_error: None,
            try_text: "Invoice 4821 for sam.lee@example.org".into(),
            from: String::new(),
            to: String::new(),
            range_error: None,
        }
    }
}

fn field(ui: &mut Ui, value: &mut String, hint: &str, width: f32) -> egui::Response {
    let p = pal(ui);
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(RichText::new(hint).color(p.faint))
            .desired_width(width)
            .margin(egui::Margin::symmetric(8, 6)),
    )
}

impl App {
    pub(super) fn privacy_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        self.load_seen();
        page_header(
            ui,
            "Privacy",
            "You decide what is recorded. Nothing ever leaves this PC.",
            |_| {},
        );
        let now = self.now();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // The promise.
                widgets::card(ui, |ui| {
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), 70.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let (rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
                            ui.painter()
                                .circle_filled(rect.center(), 26.0, p.good.gamma_multiply(0.15));
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                Icon::LOCK.glyph(),
                                regular(22.0),
                                p.good,
                            );
                            ui.add_space(6.0);
                            ui.allocate_ui_with_layout(
                                vec2(ui.available_width() - 190.0, 66.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(
                                        RichText::new("Everything stays on this PC")
                                            .font(semibold(16.0)),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!(
                                                "HourForge makes no network requests at all: no account, no cloud, no statistics, no update checks. What it records is kept in {}",
                                                self.shared.dir.display()
                                            ))
                                            .color(p.muted),
                                        )
                                        .wrap(),
                                    );
                                },
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if widgets::ghost_button_enabled(
                                    ui,
                                    &Icon::FOLDER_OPEN.with("Open data folder"),
                                    !self.demo,
                                )
                                .clicked()
                                {
                                    crate::sys::open_path(&self.shared.dir);
                                }
                            });
                        },
                    );
                });
                ui.add_space(4.0);

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let w = (ui.available_width() - 12.0) / 2.0;
                    ui.allocate_ui_with_layout(vec2(w, 10.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.vertical(|ui| {
                            self.pause_card(ui, now);
                            ui.add_space(4.0);
                            self.switches_card(ui);
                            ui.add_space(4.0);
                            self.browsers_card(ui);
                        });
                    });
                    ui.allocate_ui_with_layout(vec2(w, 10.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.vertical(|ui| {
                            self.private_apps_card(ui);
                            ui.add_space(4.0);
                            self.redaction_card(ui);
                        });
                    });
                });
                ui.add_space(4.0);
                self.delete_card(ui, now);
                ui.add_space(8.0);
            });
    }

    fn pause_card(&mut self, ui: &mut Ui, now: i64) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Pause tracking");
            let paused = self.status.state == TrackState::Paused;
            ui.label(
                RichText::new(if paused {
                    match self.status.paused_until {
                        Some(t) if t != i64::MAX => format!(
                            "Paused until {}. Nothing is being recorded.",
                            timeutil::fmt_hm(t, &self.tz)
                        ),
                        _ => "Paused. Nothing is being recorded.".to_string(),
                    }
                } else {
                    "While paused nothing is recorded: the time leaves a gap.".to_string()
                })
                .color(if paused { p.danger } else { p.muted }),
            );
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 38.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    if paused {
                        if widgets::primary_button(ui, &Icon::PLAY.with("Resume tracking"), true)
                            .clicked()
                        {
                            self.send(Cmd::Resume);
                        }
                        return;
                    }
                    let tomorrow = self.tz.day_start(self.tz.day_of(now) + 1);
                    for (label, until) in [
                        ("15 minutes", now + 15 * 60),
                        ("1 hour", now + 3600),
                        ("Until tomorrow", tomorrow),
                    ] {
                        if widgets::ghost_button(ui, label).clicked() {
                            self.send(Cmd::Pause { until });
                        }
                    }
                },
            );
        });
    }

    fn switches_card(&mut self, ui: &mut Ui) {
        widgets::card(ui, |ui| {
            widgets::section(ui, "What is recorded");
            let privacy = &mut self.settings.privacy;
            widgets::setting_row(
                ui,
                "Record window titles",
                "Off: only the program and the time are kept, never a title.",
                |ui| {
                    if widgets::toggle(ui, privacy.record_titles, true).clicked() {
                        privacy.record_titles = !privacy.record_titles;
                    }
                },
            );
            widgets::setting_row(
                ui,
                "Record websites",
                "Only the site's domain is kept, never the full address.",
                |ui| {
                    if widgets::toggle(ui, privacy.record_domains, true).clicked() {
                        privacy.record_domains = !privacy.record_domains;
                    }
                },
            );
            widgets::setting_row(
                ui,
                "Hide private browser windows",
                "InPrivate, incognito and private windows keep the time but no title or site, where the browser says so in its title.",
                |ui| {
                    if widgets::toggle(ui, privacy.hide_private_windows, true).clicked() {
                        privacy.hide_private_windows = !privacy.hide_private_windows;
                    }
                },
            );
        });
    }

    fn browsers_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Websites, per browser");
            ui.label(
                RichText::new(
                    "Address bar reads the address the browser shows and keeps only the domain. Title guesses the site from the window title.",
                )
                .size(12.5)
                .color(p.muted),
            );
            let enabled = self.settings.privacy.record_domains;
            for browser in &mut self.settings.browsers {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(110.0, 30.0), Sense::hover());
                        widgets::text_in(ui, rect, &browser.name, regular(14.0), p.text);
                        ui.add_enabled_ui(enabled, |ui| {
                            let options = [
                                DomainSource::Off,
                                DomainSource::Title,
                                DomainSource::AddressBar,
                            ];
                            let current = options
                                .iter()
                                .position(|o| *o == browser.source)
                                .unwrap_or(0);
                            if let Some(i) =
                                widgets::segmented(ui, &["Off", "Title", "Address bar"], current)
                            {
                                browser.source = options[i];
                            }
                        });
                    },
                );
            }
        });
    }

    fn private_apps_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Private programs");
            ui.label(
                RichText::new(
                    "Never record: time in the program leaves a gap. Without titles: the time counts, but no window title or site is kept.",
                )
                .size(12.5)
                .color(p.muted),
            );
            let mut remove = None;
            for (index, app) in self.settings.privacy.private_apps.iter_mut().enumerate() {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(150.0, 30.0), Sense::hover());
                        widgets::text_in(ui, rect, &app.exe, regular(13.5), p.text);
                        let current = usize::from(app.mode == PrivateMode::NoTitle);
                        if let Some(i) =
                            widgets::segmented(ui, &["Never record", "Without titles"], current)
                        {
                            app.mode = if i == 1 {
                                PrivateMode::NoTitle
                            } else {
                                PrivateMode::Skip
                            };
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::DELETE, "Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                    },
                );
            }
            if let Some(i) = remove {
                self.settings.privacy.private_apps.remove(i);
            }
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 38.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    field(ui, &mut self.privacy.new_app, "program.exe", 150.0);
                    let name = crate::rules::normalize_exe(&self.privacy.new_app);
                    let ok = !name.is_empty()
                        && !self
                            .settings
                            .privacy
                            .private_apps
                            .iter()
                            .any(|a| crate::rules::normalize_exe(&a.exe) == name);
                    if widgets::ghost_button_enabled(ui, &Icon::ADD.with("Add"), ok).clicked() {
                        self.settings.privacy.private_apps.push(PrivateApp {
                            exe: name,
                            mode: PrivateMode::Skip,
                        });
                        self.privacy.new_app.clear();
                    }
                    // Pick from what was seen.
                    let seen: Vec<(String, String)> = self
                        .cats
                        .seen
                        .iter()
                        .filter(|s| !s.is_site)
                        .take(40)
                        .map(|s| (s.key.clone(), s.label.clone()))
                        .collect();
                    if !seen.is_empty() {
                        ui.menu_button(RichText::new("Choose…").color(p.muted), |ui| {
                            ui.set_min_width(220.0);
                            egui::ScrollArea::vertical()
                                .max_height(260.0)
                                .show(ui, |ui| {
                                    for (exe, label) in &seen {
                                        if ui.button(format!("{label}  ({exe})")).clicked() {
                                            self.privacy.new_app = exe.clone();
                                            ui.close_menu();
                                        }
                                    }
                                });
                        });
                    }
                },
            );
        });
    }

    fn redaction_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Title redaction");
            ui.label(
                RichText::new(
                    "Regular expressions whose matches are replaced in window titles before anything is stored. Use them for account numbers, names, ticket numbers.",
                )
                .size(12.5)
                .color(p.muted),
            );
            let mut remove = None;
            for (index, rule) in self.settings.privacy.redactions.iter_mut().enumerate() {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 34.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        if widgets::toggle(ui, rule.enabled, true).clicked() {
                            rule.enabled = !rule.enabled;
                        }
                        let w = ui.available_width() - 130.0;
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(w.max(60.0), 28.0), Sense::hover());
                        widgets::text_in(
                            ui,
                            rect,
                            &rule.pattern,
                            egui::FontId::monospace(12.5),
                            if rule.enabled { p.text } else { p.faint },
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::DELETE, "Remove").clicked() {
                                remove = Some(index);
                            }
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("→ {}", rule.replace)).color(p.muted),
                                )
                                .truncate(),
                            );
                        });
                    },
                );
            }
            if let Some(i) = remove {
                self.settings.privacy.redactions.remove(i);
            }
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 38.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let w = (ui.available_width() - 236.0).max(100.0);
                    if field(
                        ui,
                        &mut self.privacy.new_pattern,
                        "Pattern, e.g. \\d{6,}",
                        w,
                    )
                    .changed()
                    {
                        self.privacy.pattern_error = None;
                    }
                    field(ui, &mut self.privacy.new_replace, "Replacement", 70.0);
                    let ready = !self.privacy.new_pattern.trim().is_empty();
                    if widgets::ghost_button_enabled(ui, &Icon::ADD.with("Add"), ready).clicked() {
                        match crate::rules::compile_regex(self.privacy.new_pattern.trim()) {
                            Ok(_) => {
                                self.settings.privacy.redactions.push(Redaction {
                                    pattern: self.privacy.new_pattern.trim().to_string(),
                                    replace: self.privacy.new_replace.clone(),
                                    enabled: true,
                                });
                                self.privacy.new_pattern.clear();
                            }
                            Err(e) => self.privacy.pattern_error = Some(e),
                        }
                    }
                },
            );
            if let Some(e) = &self.privacy.pattern_error {
                ui.label(RichText::new(Icon::WARNING.with(e)).color(p.danger));
            }
            widgets::divider(ui);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 34.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(RichText::new("Try it").color(p.muted));
                    let w = ui.available_width();
                    field(
                        ui,
                        &mut self.privacy.try_text,
                        "Type a window title",
                        w - 16.0,
                    );
                },
            );
            let filter = PrivacyFilter::new(&self.settings);
            let result = filter.redact(&self.privacy.try_text);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Stored as").color(p.muted));
                ui.add(egui::Label::new(RichText::new(result).color(p.accent)).truncate());
            });
        });
    }

    fn delete_card(&mut self, ui: &mut Ui, now: i64) {
        let p = pal(ui);
        let mut ask: Option<(i64, i64, String)> = None;
        widgets::card(ui, |ui| {
            widgets::section(ui, "Delete recorded data");
            ui.label(
                RichText::new(
                    "Deleted time is removed from the data files for good; it is not kept anywhere else.",
                )
                .size(12.5)
                .color(p.muted),
            );
            let today = self.today();
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 38.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    for (label, from, what) in [
                        ("Last 15 minutes", now - 15 * 60, "the last 15 minutes"),
                        ("Last hour", now - 3600, "the last hour"),
                        (
                            "Today",
                            self.tz.day_start(today),
                            "everything recorded today",
                        ),
                    ] {
                        if widgets::ghost_button(ui, label).clicked() {
                            ask = Some((from, now + 1, what.to_string()));
                        }
                    }
                    ui.add_space(14.0);
                    ui.label(RichText::new("or from").color(p.muted));
                    field(ui, &mut self.privacy.from, "YYYY-MM-DD", 100.0);
                    ui.label(RichText::new("to").color(p.muted));
                    field(ui, &mut self.privacy.to, "YYYY-MM-DD", 100.0);
                    if widgets::danger_button(ui, "Delete range…", true).clicked() {
                        match (
                            timeutil::parse_iso_date(&self.privacy.from),
                            timeutil::parse_iso_date(&self.privacy.to),
                        ) {
                            (Some(a), Some(b)) if a <= b => {
                                self.privacy.range_error = None;
                                ask = Some((
                                    self.tz.day_start(a),
                                    self.tz.day_start(b + 1),
                                    format!(
                                        "everything from {} to {}",
                                        timeutil::fmt_day(a),
                                        timeutil::fmt_day(b)
                                    ),
                                ));
                            }
                            _ => {
                                self.privacy.range_error = Some(
                                    "Type both dates like 2026-11-12, the earlier one first."
                                        .into(),
                                )
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::ghost_button(ui, "Delete everything…").clicked() {
                            ask = Some((
                                i64::MIN / 4,
                                i64::MAX / 4,
                                "everything ever recorded".into(),
                            ));
                        }
                    });
                },
            );
            if let Some(e) = &self.privacy.range_error {
                ui.label(RichText::new(Icon::WARNING.with(e)).color(p.danger));
            }
        });
        if let Some((from, to, what)) = ask {
            self.dialog = Some(Dialog::confirm(
                "Delete recorded time?",
                &format!("This deletes {what}: tracked time, away time, manual entries and timers in that range. It cannot be undone."),
                "Delete",
                ConfirmAction::DeleteRange(from, to),
            ));
        }
    }
}
