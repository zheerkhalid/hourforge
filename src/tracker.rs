//! The tracker: a background thread with a hidden window that watches the
//! foreground program, idle time, lock and sleep, and writes blocks to the
//! store. It also owns the notification-area icon, the global hotkey, the
//! reminders and the small "now tracking" indicator.
//!
//! Cost: one cheap sample every few seconds (a handful of system calls) and
//! one bookkeeping pass a minute. No polling loops, no network.

use crate::engine::{Cmd, Control, Returned, RunningFocus, RunningTimer, Shared, Status};
use crate::model::{new_id, AwayLabel, Block, DomainSource, GoalTarget, Kind, TrackState};
use crate::rules::{Classifier, PrivacyFilter, Sanitized};
use crate::sessions::{Config, Event, Out, Sample, SessionBuilder};
use crate::settings::{ReturnPrompt, Settings};
use crate::sys::wide;
use crate::timeutil::{self, Tz};
use crate::{digest, domain, spans, uia, winfo};
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    Ellipse, EndPaint, FillRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, FindWindowW, GetClientRect, GetCursorPos, GetMessageW, KillTimer, LoadIconW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW,
    TrackPopupMenu, TranslateMessage, EVENT_SYSTEM_FOREGROUND, HMENU, HTCAPTION, HWND_TOPMOST,
    IDI_APPLICATION, LWA_ALPHA, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, SPI_GETWORKAREA,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WINEVENT_OUTOFCONTEXT, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_HOTKEY,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NCHITTEST, WM_PAINT, WM_POWERBROADCAST, WM_QUERYENDSESSION,
    WM_RBUTTONUP, WM_TIMECHANGE, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

/// What the tracker asks the main thread to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiRequest {
    /// Open (or bring to front) the window, optionally on a page.
    Open(Option<String>),
    /// The user chose Exit.
    Quit,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Show the notification-area icon (and with it, notifications).
    pub tray: bool,
    /// Register the global timer hotkey from the settings.
    pub hotkey: bool,
    /// Milliseconds between samples.
    pub sample_ms: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            tray: true,
            hotkey: true,
            sample_ms: 2000,
        }
    }
}

const WINDOW_CLASS: PCWSTR = w!("HourForgeTracker");
const INDICATOR_CLASS: PCWSTR = w!("HourForgeIndicator");
const WM_TRAY: u32 = WM_APP + 1;
const WM_COMMANDS: u32 = WM_APP + 2;
const WM_QUIT_THREAD: u32 = WM_APP + 3;
const TIMER_SAMPLE: usize = 1;
const TIMER_MINUTE: usize = 2;
const HOTKEY_ID: i32 = 1;
const NIN_BALLOONUSERCLICK: u32 = 0x0405;

const MENU_OPEN: usize = 1;
const MENU_TIMER: usize = 2;
const MENU_PAUSE_15: usize = 3;
const MENU_PAUSE_60: usize = 4;
const MENU_PAUSE_TOMORROW: usize = 5;
const MENU_RESUME: usize = 6;
const MENU_EXIT: usize = 7;

/// A parsed global hotkey.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Parses `Ctrl+Alt+T`, `Win+Shift+F9`. Needs Ctrl, Alt or Win (a plain or
/// Shift-only key would swallow ordinary typing) and exactly one key:
/// a letter, a digit or F1 to F24.
pub fn parse_hotkey(text: &str) -> Option<Hotkey> {
    let mut modifiers = 0u32;
    let mut vk = None;
    for part in text.split('+') {
        let part = part.trim().to_lowercase();
        match part.as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL.0,
            "alt" => modifiers |= MOD_ALT.0,
            "shift" => modifiers |= MOD_SHIFT.0,
            "win" | "windows" => modifiers |= MOD_WIN.0,
            key => {
                if vk.is_some() {
                    return None;
                }
                let mut chars = key.chars();
                let first = chars.next()?;
                let rest: String = chars.collect();
                vk = Some(if rest.is_empty() && first.is_ascii_alphanumeric() {
                    first.to_ascii_uppercase() as u32
                } else if first == 'f' {
                    let n: u32 = rest.parse().ok()?;
                    if !(1..=24).contains(&n) {
                        return None;
                    }
                    0x70 + n - 1
                } else {
                    return None;
                });
            }
        }
    }
    let vk = vk?;
    if modifiers & (MOD_CONTROL.0 | MOD_ALT.0 | MOD_WIN.0) == 0 {
        return None;
    }
    Some(Hotkey { modifiers, vk })
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Persisted {
    #[serde(default)]
    paused_until: Option<i64>,
    /// First day of the week in which the weekly summary was last shown.
    #[serde(default)]
    weekly_shown: Option<i32>,
}

struct State {
    window: isize,
    indicator: isize,
    shared: Arc<Shared>,
    settings: Settings,
    options: Options,
    privacy: PrivacyFilter,
    classifier: Classifier,
    builder: SessionBuilder,
    tz: Tz,
    commands: Receiver<Cmd>,
    requests: Sender<UiRequest>,
    uia: Option<uia::AddressBar>,
    uia_tried: bool,

    paused_until: Option<i64>,
    locked: bool,
    asleep: bool,
    timer: Option<RunningTimer>,
    focus: Option<RunningFocus>,
    returned: Vec<Returned>,
    /// The most recent absence, to join one that follows it directly.
    last_away: Option<Returned>,

    /// The last foreground window looked at: (hwnd, pid) -> program.
    last_fg: Option<winfo::Foreground>,
    /// (hwnd, title) the cached domain belongs to.
    last_domain: Option<(isize, String, String)>,
    app: String,
    exe: String,

    tray: bool,
    today_secs: i64,
    alerted: HashSet<(i32, usize)>,
    /// Since when the user has been active without a real break.
    active_since: Option<i64>,
    break_reminded: bool,
    retention_day: i32,
    /// First day of the week in which the weekly summary was last shown.
    weekly_shown: Option<i32>,
    /// The page a click on the newest notification opens.
    notify_page: Option<String>,
    apps_dirty: bool,
    indicator_text: String,
    indicator_color: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Runs `f` on the tracker state. Skipped (returns `None`) when the state is
/// already in use further up the stack: a menu or a COM call can re-enter
/// the window procedure.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn session_config(settings: &Settings) -> Config {
    Config {
        idle_secs: settings.idle_minutes * 60,
        media_counts_active: settings.media_counts_active,
        ..Config::default()
    }
}

impl State {
    fn store_apply(&mut self, outs: Vec<Out>, now: i64) {
        if outs.is_empty() {
            return;
        }
        let mut returned = Vec::new();
        if let Ok(mut store) = self.shared.store.lock() {
            for out in outs {
                match out {
                    Out::Put(block) => {
                        let _ = store.put(&block);
                    }
                    Out::Delete(id) => {
                        let _ = store.delete(id);
                    }
                    Out::Returned { id, start, end } => returned.push(Returned { id, start, end }),
                }
            }
        }
        for r in returned {
            self.on_returned(r, now);
        }
    }

    fn on_returned(&mut self, mut r: Returned, _now: i64) {
        // Waking up usually arrives as "resume" then "lock": two away blocks
        // back to back. Join them so the user is asked once.
        if let Some(last) = self.last_away {
            let adjacent = (r.start - last.end).abs() <= 5
                && self.tz.day_of(last.start) == self.tz.day_of(r.end - 1);
            if adjacent {
                if let Ok(mut store) = self.shared.store.lock() {
                    if let (Some(mut first), Some(second)) = (store.get(last.id), store.get(r.id)) {
                        if first.away == second.away {
                            first.end = second.end;
                            let _ = store.put(&first);
                            let _ = store.delete(second.id);
                            r = Returned {
                                id: first.id,
                                start: first.start,
                                end: first.end,
                            };
                            self.returned.retain(|x| x.id != first.id);
                        }
                    }
                }
            }
        }
        self.last_away = Some(r);
        // A real break resets the "you have been at it for a while" clock.
        if r.end - r.start >= 120 {
            self.active_since = None;
            self.break_reminded = false;
        }
        if self.settings.return_prompt == ReturnPrompt::Never
            || r.end - r.start < self.settings.ask_min_minutes as i64 * 60
        {
            return;
        }
        self.returned.push(r);
        if self.returned.len() > 20 {
            self.returned.remove(0);
        }
        match self.settings.return_prompt {
            ReturnPrompt::Ask => self.request_open(Some("away")),
            _ => self.notify(
                "Welcome back",
                &format!(
                    "You were away for {}. Click to say what you were doing.",
                    timeutil::fmt_dur(r.end - r.start)
                ),
            ),
        }
    }

    fn request_open(&mut self, page: Option<&str>) {
        if let Ok(mut show) = self.shared.show.lock() {
            *show = Some(page.unwrap_or("").to_string());
        }
        let _ = self
            .requests
            .send(UiRequest::Open(page.map(str::to_string)));
        self.shared.wake();
    }

    fn push(&mut self, event: Event, now: i64) {
        let outs = self.builder.push(event);
        self.store_apply(outs, now);
    }

    /// The program in front, after the privacy rules. `None` = nothing to record.
    fn read_foreground(&mut self) -> Option<Sample> {
        let fg = winfo::foreground()?;
        let fg = match &self.last_fg {
            Some(last) if last.hwnd == fg.hwnd && last.pid == fg.pid => last.clone(),
            _ => {
                self.last_fg = Some(fg.clone());
                fg
            }
        };
        let exe = if fg.exe.is_empty() {
            "unknown".to_string()
        } else {
            fg.exe.clone()
        };
        if self.privacy.skips(&exe) {
            self.app.clear();
            self.exe.clear();
            return None;
        }
        // Friendly name, looked up once per program.
        let app = {
            let known = self
                .shared
                .apps
                .lock()
                .ok()
                .and_then(|apps| apps.get(&exe).map(|a| (a.name.clone(), a.path.clone())));
            match known {
                Some((name, path)) if path == fg.path || fg.path.is_empty() => name,
                _ => {
                    let name = winfo::file_description(&fg.path)
                        .unwrap_or_else(|| winfo::name_from_exe(&exe));
                    if let Ok(mut apps) = self.shared.apps.lock() {
                        apps.insert(
                            exe.clone(),
                            crate::engine::AppInfo {
                                name: name.clone(),
                                path: fg.path.clone(),
                            },
                        );
                    }
                    self.apps_dirty = true;
                    name
                }
            }
        };
        self.app = app.clone();
        self.exe = exe.clone();

        let details = self.privacy.wants_details(&exe);
        let raw_title = if details {
            winfo::window_title(fg.hwnd)
        } else {
            String::new()
        };
        let browser = self
            .settings
            .browsers
            .iter()
            .find(|b| b.exe.eq_ignore_ascii_case(&exe))
            .map(|b| b.source);
        let mut site = String::new();
        if let Some(source) = browser {
            let allowed = details
                && source != DomainSource::Off
                && self.settings.privacy.record_domains
                && !(self.settings.privacy.hide_private_windows
                    && crate::rules::is_private_window(&raw_title));
            if allowed {
                site = self.domain_for(fg.hwnd, &raw_title, source);
            }
        }
        match self
            .privacy
            .apply(&exe, &raw_title, &site, browser.is_some())
        {
            Sanitized::Skip => None,
            Sanitized::Keep { title, domain } => {
                let title = if browser.is_some() {
                    domain::strip_browser_suffix(&title).to_string()
                } else {
                    title
                };
                Some(Sample {
                    exe,
                    app,
                    title,
                    domain,
                })
            }
        }
    }

    fn domain_for(&mut self, window: isize, title: &str, source: DomainSource) -> String {
        if let Some((w, t, d)) = &self.last_domain {
            if *w == window && t == title {
                return d.clone();
            }
        }
        let mut found = None;
        if source == DomainSource::AddressBar {
            if !self.uia_tried {
                self.uia_tried = true;
                self.uia = uia::AddressBar::new();
            }
            if let Some(bar) = self.uia.as_mut() {
                found = bar
                    .read(window)
                    .and_then(|text| domain::domain_from_url(&text));
            }
        }
        let from_bar = found.is_some();
        let site = found
            .or_else(|| domain::domain_from_title(domain::strip_browser_suffix(title)))
            .unwrap_or_default();
        // Only remember answers from the address bar, or misses: a guess from
        // the title is retried on the next sample (the bar may have been busy).
        if from_bar || site.is_empty() {
            self.last_domain = Some((window, title.to_string(), site.clone()));
        }
        site
    }

    fn sample(&mut self) {
        let now = timeutil::now();
        if let Some(until) = self.paused_until {
            if now < until {
                return;
            }
            self.resume(now);
        }
        if self.locked || self.asleep {
            return;
        }
        if let Some(focus) = &self.focus {
            if now >= focus.until {
                self.finish_focus(now, true);
            }
        }
        let idle = winfo::idle_seconds();
        let fg = self.read_foreground();
        // Only worth asking when the answer matters.
        let media = self.settings.media_counts_active
            && idle + 30 >= self.settings.idle_minutes * 60
            && (winfo::display_required() || winfo::fullscreen_in_front());
        self.push(
            Event::Tick {
                t: now,
                fg,
                idle_secs: idle,
                media,
            },
            now,
        );
        if self.builder.state() == TrackState::Active {
            if self.active_since.is_none() {
                self.active_since = Some(now);
                self.break_reminded = false;
            }
        } else {
            self.active_since = None;
        }
        self.publish();
    }

    fn state(&self) -> TrackState {
        if self.paused_until.is_some() {
            TrackState::Paused
        } else if self.asleep {
            TrackState::Asleep
        } else if self.locked {
            TrackState::Locked
        } else {
            self.builder.state()
        }
    }

    /// Writes the status snapshot and wakes the window if something changed.
    fn publish(&mut self) {
        let state = self.state();
        let showing_app = state == TrackState::Active;
        let mut changed = false;
        if let Ok(mut status) = self.shared.status.lock() {
            let next = Status {
                state,
                paused_until: self.paused_until,
                app: if showing_app {
                    self.app.clone()
                } else {
                    String::new()
                },
                exe: if showing_app {
                    self.exe.clone()
                } else {
                    String::new()
                },
                timer: self.timer.clone(),
                focus: self.focus.clone(),
                returned: self.returned.clone(),
            };
            changed = status.state != next.state
                || status.app != next.app
                || status.paused_until != next.paused_until
                || status.timer != next.timer
                || status.focus != next.focus
                || status.returned != next.returned;
            *status = next;
        }
        if changed {
            self.shared.wake();
            self.refresh_indicator();
        }
    }

    fn pause(&mut self, until: i64, now: i64) {
        if self.paused_until.is_none() {
            self.push(Event::Pause { t: now }, now);
        }
        self.paused_until = Some(until);
        self.active_since = None;
        self.persist();
        self.publish();
        self.update_tip();
    }

    fn resume(&mut self, now: i64) {
        if self.paused_until.take().is_some() {
            self.push(Event::Unpause { t: now }, now);
            self.persist();
            self.publish();
            self.update_tip();
        }
    }

    fn persist(&self) {
        let data = Persisted {
            paused_until: self.paused_until,
            weekly_shown: self.weekly_shown,
        };
        if let Ok(json) = serde_json::to_vec(&data) {
            let _ = std::fs::write(self.shared.dir.join("state.json"), json);
        }
    }

    fn start_timer(&mut self, task: String, project: Option<String>, now: i64) {
        self.stop_timer(now);
        let timer = RunningTimer {
            id: new_id(),
            start: now,
            task,
            project,
        };
        self.put_overlay(
            Kind::Timer,
            timer.id,
            timer.start,
            now,
            &timer.task,
            &timer.project,
        );
        self.timer = Some(timer);
        self.publish();
    }

    fn stop_timer(&mut self, now: i64) -> Option<i64> {
        let timer = self.timer.take()?;
        if now - timer.start < 5 {
            // Started by mistake: leave no trace.
            if let Ok(mut store) = self.shared.store.lock() {
                let _ = store.delete(timer.id);
            }
        } else {
            self.put_overlay(
                Kind::Timer,
                timer.id,
                timer.start,
                now,
                &timer.task,
                &timer.project,
            );
        }
        self.publish();
        Some(now - timer.start)
    }

    fn start_focus(&mut self, minutes: u32, now: i64) {
        self.finish_focus(now, false);
        let focus = RunningFocus {
            id: new_id(),
            start: now,
            until: now + minutes.clamp(1, 600) as i64 * 60,
        };
        self.put_overlay(Kind::Focus, focus.id, focus.start, now, "", &None);
        self.focus = Some(focus);
        self.publish();
    }

    fn finish_focus(&mut self, now: i64, completed: bool) {
        let Some(focus) = self.focus.take() else {
            return;
        };
        let end = now.min(focus.until).max(focus.start);
        if end - focus.start < 30 {
            if let Ok(mut store) = self.shared.store.lock() {
                let _ = store.delete(focus.id);
            }
        } else {
            self.put_overlay(Kind::Focus, focus.id, focus.start, end, "", &None);
        }
        if completed {
            self.notify(
                "Focus session complete",
                &format!(
                    "{} of focused time recorded. Take {} minutes off.",
                    timeutil::fmt_dur(end - focus.start),
                    self.settings.focus_break_minutes
                ),
            );
        }
        self.publish();
    }

    fn put_overlay(
        &self,
        kind: Kind,
        id: u64,
        start: i64,
        end: i64,
        title: &str,
        project: &Option<String>,
    ) {
        let block = Block {
            id,
            start,
            end,
            kind,
            title: title.to_string(),
            project: project.clone(),
            ..Default::default()
        };
        if let Ok(mut store) = self.shared.store.lock() {
            let _ = store.put(&block);
        }
    }

    fn label_away(&mut self, id: u64, label: Option<AwayLabel>) {
        self.returned.retain(|r| r.id != id);
        if let Ok(mut store) = self.shared.store.lock() {
            match label {
                None => {
                    let _ = store.delete(id);
                }
                Some(label) => {
                    if let Some(mut block) = store.get(id) {
                        block.away = label;
                        let _ = store.put(&block);
                    }
                }
            }
        }
        self.publish();
    }

    fn apply_settings(&mut self, settings: Settings) {
        let hotkey_changed = settings.hotkey != self.settings.hotkey;
        let indicator_changed = settings.indicator != self.settings.indicator;
        self.privacy = PrivacyFilter::new(&settings);
        self.classifier = Classifier::new(&settings);
        self.builder.set_config(session_config(&settings));
        self.settings = settings;
        self.last_domain = None;
        if hotkey_changed {
            self.register_hotkey();
        }
        if indicator_changed {
            self.show_indicator(self.settings.indicator);
        }
        self.minute();
    }

    fn handle_commands(&mut self) {
        let now = timeutil::now();
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                Cmd::Pause { until } => self.pause(until, now),
                Cmd::Resume => self.resume(now),
                Cmd::StartTimer { task, project } => self.start_timer(task, project, now),
                Cmd::StopTimer => {
                    self.stop_timer(now);
                }
                Cmd::StartFocus { minutes } => self.start_focus(minutes, now),
                Cmd::StopFocus => self.finish_focus(now, false),
                Cmd::LabelAway { id, label } => self.label_away(id, label),
                Cmd::Settings(settings) => self.apply_settings(*settings),
                Cmd::Flush => self.flush(),
            }
        }
    }

    fn flush(&mut self) {
        if let Ok(mut store) = self.shared.store.lock() {
            let _ = store.flush();
        }
        if self.apps_dirty {
            self.apps_dirty = false;
            crate::engine::save_apps(&self.shared);
        }
    }

    /// Once a minute: checkpoints, totals, goals, reminders, housekeeping.
    fn minute(&mut self) {
        let now = timeutil::now();
        if let Some(t) = &self.timer {
            self.put_overlay(Kind::Timer, t.id, t.start, now, &t.task, &t.project);
        }
        if let Some(f) = &self.focus {
            self.put_overlay(Kind::Focus, f.id, f.start, now.min(f.until), "", &None);
        }
        let day = self.tz.day_of(now);
        let (from, to) = self.tz.day_bounds(day);
        let blocks = match self.shared.store.lock() {
            Ok(mut store) => store.range(from, to),
            Err(_) => Vec::new(),
        };
        let flat = spans::flatten(&blocks, from, to, &self.classifier);
        self.today_secs = spans::worked(&flat);
        self.check_goals(day, &flat);
        self.check_break(now);
        self.check_weekly(day);
        if self.retention_day != day {
            self.retention_day = day;
            self.alerted.retain(|(d, _)| *d == day);
            if self.settings.retention_days > 0 {
                if let Ok(mut store) = self.shared.store.lock() {
                    let _ = store.apply_retention(self.settings.retention_days, now);
                }
            }
        }
        self.flush();
        self.update_tip();
        self.refresh_indicator();
    }

    fn check_goals(&mut self, day: i32, flat: &[spans::Span]) {
        for (index, goal) in self.settings.goals.clone().iter().enumerate() {
            if !goal.alert || goal.minutes == 0 || self.alerted.contains(&(day, index)) {
                continue;
            }
            let (secs, name) = match &goal.target {
                GoalTarget::Total => (spans::worked(flat), "Worked time".to_string()),
                GoalTarget::Category(id) => {
                    let Some(cat) = self.classifier.category_index(id) else {
                        continue;
                    };
                    (
                        flat.iter()
                            .filter(|s| s.work && s.category as usize == cat)
                            .map(|s| s.secs())
                            .sum(),
                        self.settings.category_name(id).to_string(),
                    )
                }
                GoalTarget::Project(id) => {
                    let Some(project) = self.classifier.project_index(id) else {
                        continue;
                    };
                    (
                        flat.iter()
                            .filter(|s| s.work && s.project == Some(project as u16))
                            .map(|s| s.secs())
                            .sum(),
                        self.settings.project_name(id).to_string(),
                    )
                }
            };
            if secs < goal.minutes as i64 * 60 {
                continue;
            }
            self.alerted.insert((day, index));
            let amount = timeutil::fmt_dur(goal.minutes as i64 * 60);
            if goal.limit {
                self.notify(
                    &format!("{name}: daily limit reached"),
                    &format!("That is {amount} today. Maybe time to switch to something else?"),
                );
            } else {
                self.notify(
                    &format!("{name}: daily goal reached"),
                    &format!("{amount} done today. Nicely done."),
                );
            }
        }
    }

    /// Once a week, while the user is at the keyboard: last week in a
    /// notification. A click on it opens that week in the reports.
    fn check_weekly(&mut self, day: i32) {
        if !self.settings.weekly_summary || !self.tray || self.state() != TrackState::Active {
            return;
        }
        let monday_first = self.settings.week_starts_monday;
        let Some(from) = digest::due(day, monday_first, self.weekly_shown) else {
            return;
        };
        let (start, end) = (self.tz.day_start(from - 7), self.tz.day_start(from + 7));
        let blocks = match self.shared.store.lock() {
            Ok(mut store) => store.range(start, end),
            Err(_) => return,
        };
        let week = digest::week(&blocks, &self.classifier, &self.tz, from);
        self.weekly_shown = Some(from + 7);
        self.persist();
        if let Some((title, text)) = digest::week_notification(&week) {
            self.notify(&title, &text);
            self.notify_page = Some("reports-last-week".to_string());
        }
    }

    fn check_break(&mut self, now: i64) {
        let minutes = self.settings.break_reminder_minutes as i64;
        if minutes == 0 || self.break_reminded {
            return;
        }
        if let Some(since) = self.active_since {
            if now - since >= minutes * 60 {
                self.break_reminded = true;
                self.notify(
                    "Time for a short break",
                    &format!(
                        "You have been working for {} without a pause.",
                        timeutil::fmt_dur(now - since)
                    ),
                );
            }
        }
    }

    // ------------------------------------------------------------------------------
    // Notification area
    // ------------------------------------------------------------------------------

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd(self.window),
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            ..Default::default()
        };
        // SAFETY: loading the icon embedded by the build script (resource 1).
        unsafe {
            let module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            data.hIcon = LoadIconW(HINSTANCE(module.0), PCWSTR(std::ptr::without_provenance(1)))
                .or_else(|_| LoadIconW(HINSTANCE::default(), IDI_APPLICATION))
                .unwrap_or_default();
        }
        copy_wide(&mut data.szTip, &self.tip_text());
        data
    }

    fn tip_text(&self) -> String {
        let state = match self.state() {
            TrackState::Paused => "paused",
            TrackState::Idle | TrackState::Locked | TrackState::Asleep => "away",
            _ => "tracking",
        };
        format!(
            "HourForge: {} today ({state})",
            timeutil::fmt_dur(self.today_secs)
        )
    }

    fn set_tray(&mut self, on: bool) {
        if on == self.tray {
            return;
        }
        let data = self.tray_data();
        // SAFETY: the structure is fully initialised and outlives the call.
        unsafe {
            let _ = Shell_NotifyIconW(if on { NIM_ADD } else { NIM_DELETE }, &data);
        }
        self.tray = on;
    }

    fn update_tip(&mut self) {
        if !self.tray {
            return;
        }
        let data = self.tray_data();
        // SAFETY: as above.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    /// A notification near the clock. Never contains window titles.
    fn notify(&mut self, title: &str, text: &str) {
        if !self.tray {
            return;
        }
        // A click on a notification opens the window; only the weekly summary
        // (which sets this after the call) asks for a particular page.
        self.notify_page = None;
        let mut data = self.tray_data();
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        // SAFETY: as above.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn register_hotkey(&mut self) {
        // SAFETY: registering a hotkey on our own window.
        unsafe {
            let _ = UnregisterHotKey(hwnd(self.window), HOTKEY_ID);
            if !self.options.hotkey {
                return;
            }
            if let Some(h) = parse_hotkey(&self.settings.hotkey) {
                let _ = RegisterHotKey(
                    hwnd(self.window),
                    HOTKEY_ID,
                    HOT_KEY_MODIFIERS(h.modifiers) | MOD_NOREPEAT,
                    h.vk,
                );
            }
        }
    }

    fn toggle_timer(&mut self) {
        let now = timeutil::now();
        if self.timer.is_some() {
            if let Some(secs) = self.stop_timer(now) {
                self.notify("Timer stopped", &timeutil::fmt_clock(secs));
            }
        } else {
            let task = self.settings.last_task.clone();
            let project = self.settings.last_task_project.clone();
            self.start_timer(task, project, now);
            self.notify("Timer started", "Press the hotkey again to stop it.");
        }
    }

    // ------------------------------------------------------------------------------
    // The "now tracking" indicator
    // ------------------------------------------------------------------------------

    fn show_indicator(&mut self, on: bool) {
        // SAFETY: creating / destroying our own small window.
        unsafe {
            if !on {
                if self.indicator != 0 {
                    let _ = DestroyWindow(hwnd(self.indicator));
                    self.indicator = 0;
                }
                return;
            }
            if self.indicator != 0 {
                return;
            }
            let module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let (w, h) = (176, 30);
            let mut area = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut area as *mut RECT as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let Ok(window) = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                INDICATOR_CLASS,
                w!("HourForge indicator"),
                WS_POPUP,
                area.right - w - 16,
                area.bottom - h - 16,
                w,
                h,
                HWND::default(),
                HMENU::default(),
                HINSTANCE(module.0),
                None,
            ) else {
                return;
            };
            let _ = SetLayeredWindowAttributes(window, COLORREF(0), 235, LWA_ALPHA);
            let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, h, h);
            SetWindowRgn(window, region, false);
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                window,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            self.indicator = window.0 as isize;
        }
        self.refresh_indicator();
    }

    fn refresh_indicator(&mut self) {
        let (label, color) = match self.state() {
            TrackState::Active => ("Tracking", 0x008e_cf3e_u32),
            TrackState::Paused => ("Paused", 0x0062_5fff),
            TrackState::Off => ("Off", 0x0086_7068),
            _ => ("Away", 0x0041_c5f0),
        };
        self.indicator_text = format!("{label}  ·  {}", timeutil::fmt_dur(self.today_secs));
        self.indicator_color = color;
        if self.indicator != 0 {
            // SAFETY: asking our own window to repaint.
            unsafe {
                let _ = InvalidateRect(hwnd(self.indicator), None, true);
            }
        }
    }
}

fn copy_wide(target: &mut [u16], text: &str) {
    let max = target.len() - 1;
    let mut n = 0;
    for unit in text.encode_utf16().take(max) {
        target[n] = unit;
        n += 1;
    }
    target[n] = 0;
}

unsafe fn tray_menu(window: HWND) {
    // Gather what the menu shows first: no state borrow may be held while
    // the menu runs its own message loop.
    let Some((total, timer, paused)) = with_state(|s| {
        s.minute();
        (s.today_secs, s.timer.is_some(), s.paused_until.is_some())
    }) else {
        return;
    };
    let Ok(menu) = CreatePopupMenu() else { return };
    let header = wide(&format!("Today: {}", timeutil::fmt_dur(total)));
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(header.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open HourForge"));
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_TIMER,
        if timer {
            w!("Stop timer")
        } else {
            w!("Start timer")
        },
    );
    if paused {
        let _ = AppendMenuW(menu, MF_STRING, MENU_RESUME, w!("Resume tracking"));
    } else if let Ok(sub) = CreatePopupMenu() {
        let _ = AppendMenuW(sub, MF_STRING, MENU_PAUSE_15, w!("15 minutes"));
        let _ = AppendMenuW(sub, MF_STRING, MENU_PAUSE_60, w!("1 hour"));
        let _ = AppendMenuW(sub, MF_STRING, MENU_PAUSE_TOMORROW, w!("Until tomorrow"));
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Pause tracking"));
    }
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Needed so the menu closes when the user clicks elsewhere.
    let _ = SetForegroundWindow(window);
    let choice = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        window,
        None,
    );
    let _ = DestroyMenu(menu);
    let now = timeutil::now();
    with_state(|s| match choice.0 as usize {
        MENU_OPEN => s.request_open(None),
        MENU_TIMER => s.toggle_timer(),
        MENU_PAUSE_15 => s.pause(now + 15 * 60, now),
        MENU_PAUSE_60 => s.pause(now + 3600, now),
        MENU_PAUSE_TOMORROW => {
            let tomorrow = s.tz.day_start(s.tz.day_of(now) + 1);
            s.pause(tomorrow, now)
        }
        MENU_RESUME => s.resume(now),
        MENU_EXIT => {
            s.shared.quit.store(true, Ordering::SeqCst);
            let _ = s.requests.send(UiRequest::Quit);
            s.shared.wake();
        }
        _ => {}
    });
}

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    with_state(|s| s.sample());
}

unsafe extern "system" fn wnd_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wparam.0 {
                TIMER_SAMPLE => {
                    with_state(|s| s.sample());
                }
                TIMER_MINUTE => {
                    with_state(|s| s.minute());
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMANDS => {
            with_state(|s| s.handle_commands());
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            let now = timeutil::now();
            with_state(|s| match wparam.0 {
                // Console / remote disconnect, lock.
                2 | 4 | 7 => {
                    if !s.locked {
                        s.locked = true;
                        s.push(Event::Lock { t: now }, now);
                        s.active_since = None;
                        s.publish();
                    }
                }
                // Console / remote connect, unlock.
                1 | 3 | 8 => {
                    if s.locked {
                        s.locked = false;
                        s.push(Event::Unlock { t: now }, now);
                        s.publish();
                    }
                }
                // Sign-out.
                6 => {
                    s.push(Event::Stop { t: now }, now);
                    s.flush();
                }
                _ => {}
            });
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            let now = timeutil::now();
            with_state(|s| match wparam.0 {
                // PBT_APMSUSPEND
                4 => {
                    if !s.asleep {
                        s.asleep = true;
                        s.push(Event::Suspend { t: now }, now);
                        s.active_since = None;
                        s.minute();
                    }
                }
                // PBT_APMRESUMESUSPEND, PBT_APMRESUMEAUTOMATIC
                7 | 0x12 if s.asleep => {
                    s.asleep = false;
                    s.push(Event::Resume { t: now }, now);
                    s.publish();
                }
                _ => {}
            });
            LRESULT(1)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                let now = timeutil::now();
                with_state(|s| {
                    s.stop_timer(now);
                    s.finish_focus(now, false);
                    s.push(Event::Stop { t: now }, now);
                    s.flush();
                });
            }
            LRESULT(0)
        }
        WM_TIMECHANGE => {
            timeutil::reset_system_tz();
            LRESULT(0)
        }
        WM_HOTKEY => {
            with_state(|s| s.toggle_timer());
            LRESULT(0)
        }
        WM_TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    with_state(|s| s.request_open(None));
                }
                NIN_BALLOONUSERCLICK => {
                    with_state(|s| {
                        let summary = s.notify_page.take();
                        let page = if !s.returned.is_empty() {
                            Some("away")
                        } else {
                            summary.as_deref()
                        };
                        s.request_open(page)
                    });
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(window),
                _ => {}
            }
            LRESULT(0)
        }
        WM_QUIT_THREAD => {
            let _ = DestroyWindow(window);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => {
            let open = OPEN_MESSAGE.with(|m| *m.borrow());
            let taskbar = TASKBAR_CREATED.with(|m| *m.borrow());
            if open != 0 && msg == open {
                with_state(|s| {
                    if wparam.0 == 1 {
                        // `HourForge.exe --quit`
                        s.shared.quit.store(true, Ordering::SeqCst);
                        let _ = s.requests.send(UiRequest::Quit);
                        s.shared.wake();
                    } else {
                        s.request_open(None)
                    }
                });
                return LRESULT(0);
            }
            if taskbar != 0 && msg == taskbar {
                // Explorer restarted: put the icon back.
                with_state(|s| {
                    if s.tray {
                        s.tray = false;
                        s.set_tray(true);
                    }
                });
                return LRESULT(0);
            }
            DefWindowProcW(window, msg, wparam, lparam)
        }
    }
}

thread_local! {
    static OPEN_MESSAGE: RefCell<u32> = const { RefCell::new(0) };
    static TASKBAR_CREATED: RefCell<u32> = const { RefCell::new(0) };
}

unsafe extern "system" fn indicator_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // The whole pill is a drag handle.
        WM_NCHITTEST => LRESULT(HTCAPTION as isize),
        // Only the Settings switch removes the pill (not Alt+F4 or a task manager "close").
        WM_CLOSE => LRESULT(0),
        WM_PAINT => {
            let (text, color) = with_state(|s| (s.indicator_text.clone(), s.indicator_color))
                .unwrap_or_else(|| ("HourForge".to_string(), 0x0086_7068));
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(window, &mut ps);
            let mut rect = RECT::default();
            let _ = GetClientRect(window, &mut rect);
            let background = CreateSolidBrush(COLORREF(0x0025_1a18));
            FillRect(dc, &rect, background);
            let _ = DeleteObject(background);
            let dot = CreateSolidBrush(COLORREF(color));
            let old_brush = SelectObject(dc, dot);
            let mid = (rect.bottom - rect.top) / 2;
            let _ = Ellipse(dc, 12, mid - 5, 22, mid + 5);
            SelectObject(dc, old_brush);
            let _ = DeleteObject(dot);
            let font = CreateFontW(-14, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, w!("Segoe UI"));
            let old_font = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00f5_eeec));
            let mut text_rect = RECT {
                left: 30,
                top: 0,
                right: rect.right - 10,
                bottom: rect.bottom,
            };
            let mut wide_text: Vec<u16> = text.encode_utf16().collect();
            DrawTextW(
                dc,
                &mut wide_text,
                &mut text_rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
            SelectObject(dc, old_font);
            let _ = DeleteObject(font);
            let _ = EndPaint(window, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(window, msg, wparam, lparam),
    }
}

/// Handle to the running tracker thread.
pub struct Tracker {
    window: isize,
    commands: Mutex<Sender<Cmd>>,
    requests: Mutex<Receiver<UiRequest>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Control for Tracker {
    fn send(&self, cmd: Cmd) {
        if let Ok(tx) = self.commands.lock() {
            let _ = tx.send(cmd);
        }
        // SAFETY: posting to our own window; a stale handle just fails.
        unsafe {
            let _ = PostMessageW(hwnd(self.window), WM_COMMANDS, WPARAM(0), LPARAM(0));
        }
    }
}

/// Asks an already running HourForge to open its window, or with `quit` to
/// exit. Returns whether one was found.
pub fn signal_running_instance(quit: bool) -> bool {
    // SAFETY: looking up a window by class and posting a registered message.
    unsafe {
        let Ok(window) = FindWindowW(WINDOW_CLASS, PCWSTR::null()) else {
            return false;
        };
        if window.0.is_null() {
            return false;
        }
        let message = RegisterWindowMessageW(w!("HourForge.Open"));
        PostMessageW(window, message, WPARAM(usize::from(quit)), LPARAM(0)).is_ok()
    }
}

impl Tracker {
    /// Starts tracking. `None` when the hidden window could not be created.
    pub fn start(shared: Arc<Shared>, settings: Settings, options: Options) -> Option<Tracker> {
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let (req_tx, req_rx) = channel::<UiRequest>();
        let (ready_tx, ready_rx) = channel::<isize>();
        let thread = std::thread::Builder::new()
            .name("hourforge-tracker".into())
            .spawn(move || thread_main(shared, settings, options, cmd_rx, req_tx, ready_tx))
            .ok()?;
        let window = ready_rx.recv().ok().filter(|w| *w != 0)?;
        Some(Tracker {
            window,
            commands: Mutex::new(cmd_tx),
            requests: Mutex::new(req_rx),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Blocks until the tracker wants the window opened or the program ended.
    pub fn wait_request(&self) -> UiRequest {
        match self.requests.lock() {
            Ok(rx) => rx.recv().unwrap_or(UiRequest::Quit),
            Err(_) => UiRequest::Quit,
        }
    }

    /// Forgets requests that piled up while the window was open (it handled
    /// them itself). Returns whether one of them was Quit.
    pub fn drain_requests(&self) -> bool {
        let mut quit = false;
        if let Ok(rx) = self.requests.lock() {
            while let Ok(request) = rx.try_recv() {
                quit |= request == UiRequest::Quit;
            }
        }
        quit
    }

    /// Stops tracking: closes the open block, writes everything, removes the
    /// icon and the hotkey, and waits for the thread to end.
    pub fn stop(&self) {
        // SAFETY: posting to our own window.
        unsafe {
            let _ = PostMessageW(hwnd(self.window), WM_QUIT_THREAD, WPARAM(0), LPARAM(0));
        }
        if let Ok(mut guard) = self.thread.lock() {
            if let Some(thread) = guard.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn thread_main(
    shared: Arc<Shared>,
    settings: Settings,
    options: Options,
    commands: Receiver<Cmd>,
    requests: Sender<UiRequest>,
    ready: Sender<isize>,
) {
    // SAFETY: a standard window class, window, hook and message loop, all
    // owned by this thread and torn down before it ends.
    unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        RegisterClassW(&class);
        let indicator_class = WNDCLASSW {
            lpfnWndProc: Some(indicator_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: INDICATOR_CLASS,
            ..Default::default()
        };
        RegisterClassW(&indicator_class);
        // A hidden top-level window (not message-only: power and session
        // broadcasts only reach top-level windows).
        let window = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("HourForge tracker"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            HINSTANCE(module.0),
            None,
        );
        let Ok(window) = window else {
            let _ = ready.send(0);
            return;
        };
        OPEN_MESSAGE.with(|m| *m.borrow_mut() = RegisterWindowMessageW(w!("HourForge.Open")));
        TASKBAR_CREATED.with(|m| *m.borrow_mut() = RegisterWindowMessageW(w!("TaskbarCreated")));

        let tz = Tz::System;
        let persisted: Persisted = std::fs::read(shared.dir.join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let now = timeutil::now();
        let mut state = State {
            window: window.0 as isize,
            indicator: 0,
            privacy: PrivacyFilter::new(&settings),
            classifier: Classifier::new(&settings),
            builder: SessionBuilder::new(session_config(&settings), tz.clone()),
            tz,
            shared,
            settings,
            options,
            commands,
            requests,
            uia: None,
            uia_tried: false,
            paused_until: None,
            locked: false,
            asleep: false,
            timer: None,
            focus: None,
            returned: Vec::new(),
            last_away: None,
            last_fg: None,
            last_domain: None,
            app: String::new(),
            exe: String::new(),
            tray: false,
            today_secs: 0,
            alerted: HashSet::new(),
            active_since: None,
            break_reminded: false,
            retention_day: i32::MIN,
            weekly_shown: persisted.weekly_shown,
            notify_page: None,
            apps_dirty: false,
            indicator_text: String::new(),
            indicator_color: 0,
        };
        if let Some(until) = persisted.paused_until.filter(|u| *u > now) {
            state.pause(until, now);
        }
        state.minute();
        state.set_tray(options.tray);
        state.register_hotkey();
        if state.settings.indicator {
            state.show_indicator(true);
        }
        STATE.with(|cell| *cell.borrow_mut() = Some(state));

        let _ = WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION);
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        SetTimer(window, TIMER_SAMPLE, options.sample_ms.max(500), None);
        SetTimer(window, TIMER_MINUTE, 60_000, None);
        with_state(|s| s.sample());
        let _ = ready.send(window.0 as isize);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Tear down. The window is already destroyed.
        let _ = UnhookWinEvent(hook);
        let _ = KillTimer(window, TIMER_SAMPLE);
        let _ = KillTimer(window, TIMER_MINUTE);
        let _ = WTSUnRegisterSessionNotification(window);
        if let Some(mut state) = STATE.with(|cell| cell.borrow_mut().take()) {
            let now = timeutil::now();
            let _ = UnregisterHotKey(hwnd(state.window), HOTKEY_ID);
            state.set_tray(false);
            state.show_indicator(false);
            state.stop_timer(now);
            state.finish_focus(now, false);
            state.push(Event::Stop { t: now }, now);
            state.flush();
            if let Ok(mut status) = state.shared.status.lock() {
                *status = Status::default();
            }
        }
        let _ = ShowWindow(window, SW_HIDE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_text() {
        let h = parse_hotkey("Ctrl+Alt+T").unwrap();
        assert_eq!(h.modifiers, MOD_CONTROL.0 | MOD_ALT.0);
        assert_eq!(h.vk, 0x54);
        assert_eq!(parse_hotkey(" ctrl + shift + f9 ").unwrap().vk, 0x78);
        assert_eq!(parse_hotkey("Win+Alt+7").unwrap().vk, 0x37);
        for bad in [
            "T",
            "Shift+T",
            "Ctrl+",
            "Ctrl+Alt",
            "Ctrl+A+B",
            "Ctrl+F25",
            "Ctrl+Space",
            "",
        ] {
            assert!(parse_hotkey(bad).is_none(), "{bad}");
        }
    }

    /// The state file of 1.0.0 has no `weekly_shown`; an empty file is fine too.
    #[test]
    fn state_file_of_1_0_0_is_read() {
        let old: Persisted = serde_json::from_slice(br#"{"paused_until":1794300000}"#).unwrap();
        assert_eq!(old.paused_until, Some(1_794_300_000));
        assert_eq!(old.weekly_shown, None);
        let empty: Persisted = serde_json::from_slice(b"{}").unwrap();
        assert_eq!((empty.paused_until, empty.weekly_shown), (None, None));
        let new = Persisted {
            paused_until: None,
            weekly_shown: Some(20_766),
        };
        let json = serde_json::to_vec(&new).unwrap();
        let back: Persisted = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.weekly_shown, Some(20_766));
    }

    #[test]
    fn wide_copy_truncates() {
        let mut buf = [1u16; 4];
        copy_wide(&mut buf, "abcdef");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, b'c' as u16, 0]);
    }
}
