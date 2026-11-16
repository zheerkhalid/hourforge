#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use hourforge::engine::{self, Shared};
use hourforge::export::{self, Format};
use hourforge::settings::{self, Settings};
use hourforge::store::Store;
use hourforge::timeutil::{self, Tz};
use hourforge::tracker::{self, Tracker, UiRequest};
use hourforge::ui::{self, Options};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const USAGE: &str = "HourForge: a private, automatic time tracker

  HourForge.exe                    start tracking and open the window
  HourForge.exe --background       start tracking in the notification area only
                                   (what the start-with-Windows entry runs)
  HourForge.exe --quit             ask the running HourForge to exit
  HourForge.exe --export <range> [--format csv|json|html] [--timesheet]
        [--project <id>] [--out <file>]
                                   write a report (or a timesheet) and exit.
                                   <range>: today, yesterday, week, last-week, month,
                                   last-month, 2026-11-12 or 2026-11-01..2026-11-12
  HourForge.exe --demo             show invented data; nothing is tracked or saved
  HourForge.exe --screenshot <png> [--view <name>] [--light]
                                   render a page of the demo to a PNG and exit
  HourForge.exe --selftest         feed a synthetic day through the whole pipeline
                                   in a temporary folder and check the results
  HourForge.exe --probe            check what the tracker can read on this PC; prints
                                   counts only (no titles or names), stores nothing
  --data <folder>                  use this data folder instead of
                                   %LOCALAPPDATA%\\HourForge (also: HOURFORGE_DATA)

HourForge makes no network requests. Everything it records stays in its data folder.";

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    if has("--help") || has("-h") || has("/?") {
        hourforge::sys::attach_parent_console();
        println!("{USAGE}");
        return Ok(());
    }
    if has("--selftest") {
        hourforge::sys::attach_parent_console();
        match hourforge::selftest::run() {
            Ok(log) => {
                for line in log {
                    println!("{line}");
                }
                println!("SELFTEST PASSED");
                std::process::exit(0);
            }
            Err(e) => {
                println!("{e}");
                println!("SELFTEST FAILED");
                std::process::exit(1);
            }
        }
    }
    let data_dir = value("--data")
        .map(PathBuf::from)
        .unwrap_or_else(settings::default_data_dir);

    if let Some(range) = value("--export") {
        hourforge::sys::attach_parent_console();
        let code = match export_command(
            &data_dir,
            &range,
            value("--format").as_deref().unwrap_or("csv"),
            has("--timesheet"),
            value("--project"),
            value("--out").map(PathBuf::from),
        ) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("HourForge: {e}");
                2
            }
        };
        std::process::exit(code);
    }
    if has("--probe") {
        hourforge::sys::attach_parent_console();
        for line in hourforge::selftest::probe() {
            println!("{line}");
        }
        return Ok(());
    }
    if has("--quit") {
        tracker::signal_running_instance(true);
        return Ok(());
    }

    // The demo and screenshots: invented data, no tracker, no files.
    if has("--demo") || has("--screenshot") {
        let options = Options {
            demo: true,
            screenshot: value("--screenshot").map(PathBuf::from),
            view: value("--view").or_else(|| std::env::var("HOURFORGE_VIEW").ok()),
            light: has("--light"),
            ..Default::default()
        };
        return ui::run(options, window_icon());
    }

    // One tracker per user session: a second start just opens the window of the first.
    let Some(_instance) = hourforge::sys::single_instance("HourForge.Instance") else {
        tracker::signal_running_instance(false);
        return Ok(());
    };

    let settings = Settings::load(&data_dir);
    let store = match Store::open(&data_dir) {
        Ok(store) => store,
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("HourForge")
                .set_description(format!(
                    "The data folder could not be opened:\n{}\n\n{e}",
                    data_dir.display()
                ))
                .set_level(rfd::MessageLevel::Error)
                .show();
            std::process::exit(1);
        }
    };
    let shared = Arc::new(Shared::new(data_dir.clone(), store));
    if let Ok(mut apps) = shared.apps.lock() {
        *apps = engine::load_apps(&data_dir);
    }
    let Some(tracker) = Tracker::start(shared.clone(), settings, tracker::Options::default())
    else {
        rfd::MessageDialog::new()
            .set_title("HourForge")
            .set_description("The tracker could not be started.")
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    };
    let tracker = Arc::new(tracker);

    let mut open = !has("--background");
    let mut page: Option<String> = value("--view");
    loop {
        if open {
            // Requests that came in before the window existed are covered by opening it.
            tracker.drain_requests();
            let options = Options {
                view: page.take(),
                shared: Some(shared.clone()),
                control: Some(tracker.clone()),
                ..Default::default()
            };
            let result = ui::run(options, window_icon());
            if result.is_err() || shared.quit.load(Ordering::SeqCst) || tracker.drain_requests() {
                break;
            }
            // The window was closed. Keep tracking only if the user wants that.
            if !Settings::load(&data_dir).close_to_tray {
                break;
            }
        }
        match tracker.wait_request() {
            UiRequest::Open(view) => {
                open = true;
                page = view;
            }
            UiRequest::Quit => break,
        }
    }
    tracker.stop();
    Ok(())
}

/// `--export`: reads the data folder and writes a report. Works while the
/// tracker is running (it only reads).
fn export_command(
    dir: &std::path::Path,
    range: &str,
    format: &str,
    timesheet: bool,
    project: Option<String>,
    out: Option<PathBuf>,
) -> Result<(), String> {
    let format = Format::parse(format)
        .ok_or_else(|| format!("unknown format \"{format}\" (csv, json, html)"))?;
    if !dir.is_dir() {
        return Err(format!("no data folder at {}", dir.display()));
    }
    let settings = Settings::load(dir);
    let tz = Tz::System;
    let (from, to, label) =
        export::parse_range(range, timeutil::now(), &tz, settings.week_starts_monday)
            .ok_or_else(|| format!("unknown range \"{range}\" (see --help)"))?;
    let mut store = Store::open_read_only(dir).map_err(|e| e.to_string())?;
    let blocks = store.range(from, to);
    let input = export::Input {
        blocks: &blocks,
        settings: &settings,
        tz: &tz,
        from,
        to,
        label: &label,
    };
    let text = if timesheet {
        if let Some(id) = &project {
            if settings.project(id).is_none() {
                return Err(format!("no project with the id \"{id}\""));
            }
        }
        let sheet = export::timesheet(&input, project.as_deref());
        export::timesheet_text(&sheet, &input, format)
    } else {
        export::report(&input, format)
    };
    match out {
        Some(path) => std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display())),
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())
        }
    }
}

fn window_icon() -> egui::IconData {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icon-256.png"));
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
}
