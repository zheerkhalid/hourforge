//! The Settings page: tracking behaviour, Windows integration, appearance
//! and storage (retention, compaction, backup, import).

use super::app::{page_header, App};
use super::dialogs::{ConfirmAction, Dialog, ExportKind};
use super::theme::{pal, Icon};
use super::widgets;
use crate::settings::{ReturnPrompt, ThemeChoice};
use crate::store::Stats;
use crate::timeutil;
use eframe::egui::{self, vec2, RichText, Ui};

#[derive(Default)]
pub struct SettingsState {
    hotkey: Option<String>,
    stats_key: Option<u64>,
    stats: Stats,
    startup: Option<bool>,
}

fn bytes_text(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

impl App {
    pub(super) fn settings_page_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let mut about = false;
        page_header(
            ui,
            "Settings",
            "How HourForge tracks, starts and stores.",
            |ui| {
                if widgets::ghost_button(ui, &Icon::INFO.with("About")).clicked() {
                    about = true;
                }
            },
        );
        if about {
            self.about_open = true;
        }
        let rev = self.rev();
        if self.settings_ui.stats_key != Some(rev) {
            self.settings_ui.stats = match self.shared.store.lock() {
                Ok(mut store) => store.stats(),
                Err(_) => Stats::default(),
            };
            self.settings_ui.stats_key = Some(rev);
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let w = (ui.available_width() - 12.0) / 2.0;
                    ui.allocate_ui_with_layout(
                        vec2(w, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.vertical(|ui| {
                                self.tracking_card(ui);
                                ui.add_space(4.0);
                                self.appearance_card(ui);
                            });
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(w, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.vertical(|ui| {
                                self.windows_card(ui);
                                ui.add_space(4.0);
                                self.storage_card(ui);
                            });
                        },
                    );
                });
                ui.add_space(8.0);
                let _ = p;
            });
    }

    fn tracking_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Tracking");
            let s = &mut self.settings;
            widgets::setting_row(
                ui,
                "Away after",
                "No keyboard or mouse input for this long counts as away, from the last input on.",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.idle_minutes)
                            .range(1..=120)
                            .suffix(" min"),
                    );
                },
            );
            widgets::setting_row(
                ui,
                "Media counts as active",
                "Fullscreen video, presentations and anything that keeps the screen on are not treated as away.",
                |ui| {
                    if widgets::toggle(ui, s.media_counts_active, true).clicked() {
                        s.media_counts_active = !s.media_counts_active;
                    }
                },
            );
            widgets::setting_row(
                ui,
                "When I come back",
                "Ask what you were doing while away: a meeting, a call, a break, or nothing worth keeping.",
                |ui| {
                    let options = [ReturnPrompt::Ask, ReturnPrompt::Notify, ReturnPrompt::Never];
                    let current = options
                        .iter()
                        .position(|o| *o == s.return_prompt)
                        .unwrap_or(0);
                    if let Some(i) = widgets::segmented(ui, &["Ask", "Notify", "Never"], current) {
                        s.return_prompt = options[i];
                    }
                },
            );
            widgets::setting_row(
                ui,
                "Only after absences of",
                "Shorter absences stay plain away time; they can still be labelled in the timeline.",
                |ui| {
                    ui.add_enabled(
                        s.return_prompt != ReturnPrompt::Never,
                        egui::DragValue::new(&mut s.ask_min_minutes)
                            .range(1..=240)
                            .suffix(" min"),
                    );
                },
            );
            ui.label(
                RichText::new(
                    "Locking the PC, sleep and signing out always end the current block.",
                )
                .size(12.0)
                .color(p.faint),
            );
        });
    }

    fn appearance_card(&mut self, ui: &mut Ui) {
        widgets::card(ui, |ui| {
            widgets::section(ui, "Appearance");
            let s = &mut self.settings;
            widgets::setting_row(ui, "Theme", "", |ui| {
                let options = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];
                let current = options.iter().position(|o| *o == s.theme).unwrap_or(0);
                if let Some(i) = widgets::segmented(ui, &["Dark", "Light", "System"], current) {
                    s.theme = options[i];
                }
            });
            widgets::setting_row(ui, "Week starts on", "", |ui| {
                if let Some(i) = widgets::segmented(
                    ui,
                    &["Monday", "Sunday"],
                    usize::from(!s.week_starts_monday),
                ) {
                    s.week_starts_monday = i == 0;
                }
            });
        });
    }

    fn windows_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let demo = self.demo;
        let mut startup_error = None;
        widgets::card(ui, |ui| {
            widgets::section(ui, "Windows");
            let startup = *self
                .settings_ui
                .startup
                .get_or_insert_with(|| !demo && crate::sys::startup_enabled(crate::sys::RUN_VALUE));
            widgets::setting_row(
                ui,
                "Start with Windows",
                "Starts tracking in the notification area when you sign in. For your account only.",
                |ui| {
                    if widgets::toggle(ui, startup, !demo).clicked() {
                        match crate::sys::set_startup(crate::sys::RUN_VALUE, !startup) {
                            Ok(()) => self.settings_ui.startup = Some(!startup),
                            Err(e) => startup_error = Some(e.to_string()),
                        }
                    }
                },
            );
            let s = &mut self.settings;
            widgets::setting_row(
                ui,
                "Keep tracking when the window is closed",
                "Closing the window leaves HourForge in the notification area. Exit from its menu there.",
                |ui| {
                    if widgets::toggle(ui, s.close_to_tray, true).clicked() {
                        s.close_to_tray = !s.close_to_tray;
                    }
                },
            );
            widgets::setting_row(
                ui,
                "Now-tracking indicator",
                "A small pill above the taskbar with the state and today's total. Drag it anywhere.",
                |ui| {
                    if widgets::toggle(ui, s.indicator, true).clicked() {
                        s.indicator = !s.indicator;
                    }
                },
            );
            let buffer = self
                .settings_ui
                .hotkey
                .get_or_insert_with(|| s.hotkey.clone());
            let valid = buffer.trim().is_empty() || crate::tracker::parse_hotkey(buffer).is_some();
            widgets::setting_row(
                ui,
                "Timer hotkey",
                "Starts and stops the timer from anywhere. Needs Ctrl, Alt or Win plus a letter, digit or F-key. Empty for none.",
                |ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(buffer)
                            .desired_width(110.0)
                            .margin(egui::Margin::symmetric(8, 6))
                            .text_color(if valid { p.text } else { p.danger }),
                    );
                    if resp.changed() && valid {
                        s.hotkey = buffer.trim().to_string();
                    }
                },
            );
            if !valid {
                ui.label(
                    RichText::new(
                        Icon::WARNING.with("Not a usable hotkey, for example Ctrl+Alt+T."),
                    )
                    .color(p.danger),
                );
            }
        });
        if let Some(e) = startup_error {
            self.toast(format!("Could not change the startup entry: {e}"), true);
        }
    }

    fn storage_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let stats = self.settings_ui.stats.clone();
        let mut compact = false;
        let mut backup = false;
        let mut import = false;
        let mut export = false;
        widgets::card(ui, |ui| {
            widgets::section(ui, "Storage");
            let since = match stats.first {
                Some(t) => format!(", since {}", timeutil::fmt_day(self.tz.day_of(t))),
                None => String::new(),
            };
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "{} blocks, {} in {} file{}{since}. Stored only in {}",
                        stats.blocks,
                        bytes_text(stats.bytes),
                        stats.files,
                        if stats.files == 1 { "" } else { "s" },
                        self.shared.dir.display()
                    ))
                    .color(p.muted),
                )
                .wrap(),
            );
            let s = &mut self.settings;
            widgets::setting_row(
                ui,
                "Keep history for",
                "Older data is deleted by itself, once a day.",
                |ui| {
                    let label = |days: u32| match days {
                        0 => "Forever".to_string(),
                        365 => "1 year".to_string(),
                        730 => "2 years".to_string(),
                        d => format!("{d} days"),
                    };
                    egui::ComboBox::from_id_salt("retention")
                        .selected_text(label(s.retention_days))
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for days in [0, 30, 90, 180, 365, 730] {
                                ui.selectable_value(&mut s.retention_days, days, label(days));
                            }
                        });
                },
            );
            ui.add_space(2.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 38.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    if widgets::ghost_button(ui, &Icon::SAVE.with("Back up…"))
                        .on_hover_text("Write everything recorded into one file")
                        .clicked()
                    {
                        backup = true;
                    }
                    if widgets::ghost_button(ui, "Import…")
                        .on_hover_text("Merge a backup file into the recorded data")
                        .clicked()
                    {
                        import = true;
                    }
                    if widgets::ghost_button(ui, &Icon::EXPORT.with("Export…")).clicked() {
                        export = true;
                    }
                    if widgets::ghost_button(ui, "Compact")
                        .on_hover_text("Rewrite the data files without superseded records")
                        .clicked()
                    {
                        compact = true;
                    }
                },
            );
        });
        if compact {
            if let Some(report) = self.store_write(|store| store.compact()) {
                self.toast(
                    format!(
                        "Compacted: {} to {} ({} old records dropped)",
                        bytes_text(report.bytes_before),
                        bytes_text(report.bytes_after),
                        report.records_dropped
                    ),
                    false,
                );
            }
        }
        if backup {
            let name = format!(
                "HourForge-backup-{}.hfbackup",
                timeutil::iso_date(self.today())
            );
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(name)
                .add_filter("HourForge backup", &["hfbackup"])
                .save_file()
            {
                if let Some(n) = self.store_write(|store| store.backup(&path)) {
                    self.toast(format!("Backed up {n} blocks to {}", path.display()), false);
                }
            }
        }
        if import {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("HourForge backup", &["hfbackup"])
                .pick_file()
            {
                self.dialog = Some(Dialog::confirm(
                    "Import this backup?",
                    "Blocks from the file are added to what is recorded here; blocks that exist in both are replaced by the file's version.",
                    "Import",
                    ConfirmAction::ImportBackup(path),
                ));
            }
        }
        if export {
            let today = self.today();
            self.dialog = Some(Dialog::export(
                timeutil::week_start(today, self.settings.week_starts_monday),
                today,
                ExportKind::Report,
                None,
            ));
        }
    }
}
