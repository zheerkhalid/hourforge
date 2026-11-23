//! Application state and the window frame: header (status, timer, pause),
//! navigation rail, toasts, and the plumbing between the pages, the store and
//! the tracker.

use super::dialogs::{Dialog, ExportKind};
use super::goals::GoalsState;
use super::privacy::PrivacyState;
use super::reports::ReportsView;
use super::review::ReviewState;
use super::rules_view::{CatsState, ProjectsState};
use super::settings_view::SettingsState;
use super::theme::{self, pal, pal_ctx, regular, semibold, Icon};
use super::timeline::TimelineState;
use super::today::TodayView;
use super::widgets;
use crate::engine::{Cmd, Control, Returned, RunningFocus, RunningTimer, Shared, Status};
use crate::icons::IconImage;
use crate::model::{Block, BlockId, Field, Kind, Rule, TrackState};
use crate::rules::Classifier;
use crate::settings::Settings;
use crate::store::Store;
use crate::timeutil::{self, Day, Tz};
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense,
    Stroke, TextureHandle, Ui,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// How the window was started.
#[derive(Clone, Default)]
pub struct Options {
    /// Show invented data; nothing is tracked and nothing is written.
    pub demo: bool,
    /// Render one frame of the chosen view to this PNG, then exit.
    pub screenshot: Option<PathBuf>,
    /// Which page / dialog to open first (see `App::apply_view`).
    pub view: Option<String>,
    /// Force the light theme (for screenshots).
    pub light: bool,
    /// The store, status and program table shared with the tracker.
    pub shared: Option<Arc<Shared>>,
    /// Where commands go.
    pub control: Option<Arc<dyn Control>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Today,
    Timeline,
    Reports,
    /// Programs and websites that no rule has sorted yet.
    Review,
    Categories,
    Projects,
    Goals,
    Privacy,
    Settings,
}

/// Something a view asks the application to do. Views never reach into each
/// other; they push one of these onto `Cx::actions`.
#[allow(dead_code)] // the full contract offered to views, whether or not one uses it yet
pub enum Action {
    Go(Tab),
    /// Open the timeline on a day, optionally with one block selected.
    OpenTimeline {
        day: Day,
        block: Option<BlockId>,
    },
    /// Open the reports on a range of days (both included).
    OpenReport {
        from: Day,
        to: Day,
    },
    /// Open the export dialog for a range of days (both included).
    Export {
        from: Day,
        to: Day,
    },
    /// Add a rule (after "drag a program onto a category" and similar).
    AddRule(Rule),
    /// Open the new-rule dialog with these values filled in.
    NewRule {
        field: Field,
        pattern: String,
    },
    /// Send a command to the tracker.
    Cmd(Cmd),
    /// Show a short message. `true` = it is an error.
    Toast(String, bool),
}

/// Program icons as textures. Missing icons are painted as coloured tiles by
/// `widgets::program_icon`.
pub struct Icons {
    textures: HashMap<String, Option<TextureHandle>>,
    requested: HashSet<String>,
    tx: Sender<(String, Option<IconImage>)>,
    rx: Receiver<(String, Option<IconImage>)>,
    last_scan: Option<Instant>,
}

impl Default for Icons {
    fn default() -> Self {
        let (tx, rx) = channel();
        Icons {
            textures: HashMap::new(),
            requested: HashSet::new(),
            tx,
            rx,
            last_scan: None,
        }
    }
}

impl Icons {
    /// The icon of a program by its lower-case file name, if it is loaded.
    pub fn get(&self, exe: &str) -> Option<&TextureHandle> {
        self.textures.get(exe).and_then(|t| t.as_ref())
    }

    /// Loads icons of programs the tracker has seen, in the background.
    fn poll(&mut self, ctx: &egui::Context, shared: &Shared) {
        while let Ok((exe, image)) = self.rx.try_recv() {
            let texture = image.map(|img| {
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width as usize, img.height as usize],
                    &img.rgba,
                );
                ctx.load_texture(format!("icon-{exe}"), color, egui::TextureOptions::LINEAR)
            });
            self.textures.insert(exe, texture);
        }
        if self
            .last_scan
            .is_some_and(|t| t.elapsed().as_secs_f32() < 5.0)
        {
            return;
        }
        self.last_scan = Some(Instant::now());
        let todo: Vec<(String, String)> = match shared.apps.lock() {
            Ok(apps) => apps
                .iter()
                .filter(|(exe, info)| !info.path.is_empty() && !self.requested.contains(*exe))
                .map(|(exe, info)| (exe.clone(), info.path.clone()))
                .collect(),
            Err(_) => Vec::new(),
        };
        if todo.is_empty() {
            return;
        }
        for (exe, _) in &todo {
            self.requested.insert(exe.clone());
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for (exe, path) in todo {
                let image = crate::icons::extract(&path);
                if tx.send((exe, image)).is_err() {
                    return;
                }
            }
            ctx.request_repaint();
        });
    }
}

/// What every view gets to work with.
#[allow(dead_code)]
pub struct Cx<'a> {
    pub shared: &'a Arc<Shared>,
    /// Views may change settings; set `settings_dirty` when they do.
    pub settings: &'a mut Settings,
    pub settings_dirty: &'a mut bool,
    /// The rules compiled from the settings as of this frame.
    pub classifier: &'a Classifier,
    pub tz: &'a Tz,
    /// Now, in UTC seconds (a fixed invented moment in demo mode).
    pub now: i64,
    /// The local day `now` falls on.
    pub today: Day,
    pub demo: bool,
    /// Changes whenever stored data or the rules change. Views cache what
    /// they compute and recompute when this differs from last time.
    pub rev: u64,
    pub status: &'a Status,
    pub actions: &'a mut Vec<Action>,
    pub icons: &'a Icons,
}

#[allow(dead_code)]
impl Cx<'_> {
    /// Every block overlapping `[from, to)` (UTC), sorted by start.
    pub fn blocks(&self, from: i64, to: i64) -> Vec<Block> {
        match self.shared.store.lock() {
            Ok(mut store) => store.range(from, to),
            Err(_) => Vec::new(),
        }
    }

    /// Every block overlapping the local days `from..=to`.
    pub fn blocks_for_days(&self, from: Day, to: Day) -> Vec<Block> {
        self.blocks(self.tz.day_start(from), self.tz.day_start(to + 1))
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.actions.push(Action::Toast(text.into(), false));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.actions.push(Action::Toast(text.into(), true));
    }
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub born: Instant,
}

struct Shot {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

pub struct App {
    pub(super) shared: Arc<Shared>,
    control: Option<Arc<dyn Control>>,
    pub(super) demo: bool,
    pub(super) settings: Settings,
    saved_settings: Settings,
    settings_rev: u64,
    pub(super) classifier: Classifier,
    pub(super) tz: Tz,
    pub(super) tab: Tab,
    pub(super) status: Status,

    today_view: TodayView,
    reports_view: ReportsView,
    pub(super) timeline: TimelineState,
    pub(super) review: ReviewState,
    pub(super) cats: CatsState,
    pub(super) projects: ProjectsState,
    pub(super) goals: GoalsState,
    pub(super) privacy: PrivacyState,
    pub(super) settings_ui: SettingsState,
    pub(super) icons: Icons,

    timer_task: String,
    pub(super) dialog: Option<Dialog>,
    /// Whether the "what were you doing?" question may open by itself.
    pub(super) away_prompt: bool,
    pub(super) about_open: bool,
    pub(super) toasts: Vec<Toast>,
    pub(super) logo: Option<TextureHandle>,
    shot: Option<Shot>,
}

/// Opens the window and returns when it is closed.
pub fn run(options: Options, icon: egui::IconData) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title(if options.demo {
            "HourForge (demo)"
        } else {
            "HourForge"
        })
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1040.0, 640.0])
        .with_icon(icon.clone());
    let native = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "HourForge",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, options, icon)))),
    )
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, options: Options, icon: egui::IconData) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);

        let demo = options.demo;
        let (shared, mut settings, tz) = if demo {
            let settings = crate::demo::settings();
            let mut store = Store::in_memory();
            crate::demo::populate(&mut store, &settings);
            let shared = Arc::new(Shared::new(
                PathBuf::from("C:\\Users\\Alex\\AppData\\Local\\HourForge"),
                store,
            ));
            (shared, settings, crate::demo::tz())
        } else {
            let shared = options.shared.clone().unwrap_or_else(|| {
                let dir = crate::settings::default_data_dir();
                let store = Store::open(&dir).unwrap_or_else(|_| Store::in_memory());
                Arc::new(Shared::new(dir, store))
            });
            let settings = Settings::load(&shared.dir);
            (shared, settings, Tz::System)
        };
        if options.light {
            settings.theme = crate::settings::ThemeChoice::Light;
        }
        theme::apply_choice(ctx, settings.theme);

        // The tracker wakes the window when something changed.
        let wake_ctx = ctx.clone();
        if let Ok(mut waker) = shared.waker.lock() {
            *waker = Some(Box::new(move || wake_ctx.request_repaint()));
        }

        let logo = (icon.width > 0).then(|| {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
        });
        let mut app = App {
            classifier: Classifier::new(&settings),
            saved_settings: settings.clone(),
            timer_task: settings.last_task.clone(),
            settings,
            settings_rev: 0,
            status: shared.status(),
            shared,
            control: options.control.clone(),
            demo,
            tz,
            tab: Tab::Today,
            today_view: TodayView::default(),
            reports_view: ReportsView::default(),
            timeline: TimelineState::default(),
            review: ReviewState::default(),
            cats: CatsState::default(),
            projects: ProjectsState::default(),
            goals: GoalsState::default(),
            privacy: PrivacyState::default(),
            settings_ui: SettingsState::default(),
            icons: Icons::default(),
            dialog: None,
            away_prompt: true,
            about_open: false,
            toasts: Vec::new(),
            logo,
            shot: options.screenshot.clone().map(|path| Shot {
                path,
                frames: 0,
                requested: false,
            }),
        };
        if demo {
            app.demo_status();
        }
        app.apply_view(options.view.as_deref().unwrap_or("today"));
        app
    }

    /// The invented tracker status of demo mode: active in whatever the last
    /// invented block was.
    fn demo_status(&mut self) {
        let now = self.now();
        let recent = self.blocks(now - 1800, now + 1);
        let last = recent.iter().rev().find(|b| b.kind == Kind::Active);
        if let Ok(mut status) = self.shared.status.lock() {
            status.state = TrackState::Active;
            if let Some(b) = last {
                status.app = b.app.clone();
                status.exe = b.exe.clone();
            }
        }
    }

    /// Opens the page / dialog named on the command line. Names with a
    /// `today` or `reports` prefix are passed on to those views.
    fn apply_view(&mut self, view: &str) {
        let today = self.today();
        match view {
            "timeline" => self.tab = Tab::Timeline,
            "timeline-edit" => {
                self.tab = Tab::Timeline;
                let (from, to) = self.tz.day_bounds(today);
                let blocks = self.blocks(from, to);
                if let Some(b) = blocks
                    .iter()
                    .filter(|b| b.kind == Kind::Active)
                    .max_by_key(|b| b.secs())
                {
                    self.timeline.highlight = Some(b.id);
                    self.dialog = Some(Dialog::edit(b, &self.tz));
                }
            }
            "timeline-rtl" | "timeline-search" => {
                // The demo with Kurdish and Arabic titles and a Kurdish project.
                self.tab = Tab::Timeline;
                if self.demo {
                    if let Ok(mut store) = self.shared.store.lock() {
                        crate::demo::showcase_rtl(&mut store, &mut self.settings);
                    }
                    if view == "timeline-search" {
                        self.timeline.search = "ماڵپەڕ".to_string();
                    }
                }
            }
            "review" => {
                self.tab = Tab::Review;
                if self.demo {
                    if let Ok(mut store) = self.shared.store.lock() {
                        crate::demo::showcase_review(&mut store);
                    }
                }
            }
            "shortcuts" => self.dialog = Some(Dialog::Shortcuts),
            "invoice" => {
                self.tab = Tab::Projects;
                if self.demo {
                    self.settings.timesheet.business = "Alex Rivers Studio".into();
                }
                self.dialog = Some(Dialog::export(
                    timeutil::month_start(today),
                    today,
                    ExportKind::Invoice,
                    None,
                ));
            }
            "reports-last-week" => {
                self.tab = Tab::Reports;
                self.reports_view.open_week(today - 7);
            }
            "categories" => self.tab = Tab::Categories,
            "projects" => self.tab = Tab::Projects,
            "goals" => self.tab = Tab::Goals,
            "privacy" => self.tab = Tab::Privacy,
            "settings" => self.tab = Tab::Settings,
            "about" => self.about_open = true,
            "export" => {
                self.tab = Tab::Projects;
                self.dialog = Some(Dialog::export(
                    timeutil::month_start(today),
                    today,
                    ExportKind::Timesheet,
                    None,
                ));
            }
            "away" => {
                self.away_prompt = true;
                if self.demo {
                    // An invented absence: the last away block of the day.
                    let (from, to) = self.tz.day_bounds(today);
                    let blocks = self.blocks(from, to);
                    let away = blocks
                        .iter()
                        .rev()
                        .find(|b| b.kind == Kind::Away && b.secs() >= 600);
                    if let (Some(b), Ok(mut status)) = (away, self.shared.status.lock()) {
                        status.returned = vec![Returned {
                            id: b.id,
                            start: b.start,
                            end: b.end,
                        }];
                    }
                }
            }
            v if v.starts_with("reports") => {
                self.tab = Tab::Reports;
                self.reports_view.apply_view(v);
            }
            v => {
                self.tab = Tab::Today;
                self.today_view.apply_view(v);
            }
        }
    }

    pub(super) fn now(&self) -> i64 {
        if self.demo {
            crate::demo::now()
        } else {
            timeutil::now()
        }
    }

    pub(super) fn today(&self) -> Day {
        self.tz.day_of(self.now())
    }

    /// Changes whenever stored data or the rules change.
    pub(super) fn rev(&self) -> u64 {
        let store = self.shared.store.lock().map(|s| s.rev()).unwrap_or(0);
        store
            .wrapping_mul(1_000_003)
            .wrapping_add(self.settings_rev)
    }

    pub(super) fn blocks(&self, from: i64, to: i64) -> Vec<Block> {
        match self.shared.store.lock() {
            Ok(mut store) => store.range(from, to),
            Err(_) => Vec::new(),
        }
    }

    /// Runs a change on the store; a failure becomes an error toast.
    pub(super) fn store_write<T>(
        &mut self,
        change: impl FnOnce(&mut Store) -> std::io::Result<T>,
    ) -> Option<T> {
        let result = match self.shared.store.lock() {
            Ok(mut store) => change(&mut store),
            Err(_) => Err(std::io::Error::other("the store is not available")),
        };
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                self.toast(format!("Could not save: {e}"), true);
                None
            }
        }
    }

    pub(super) fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast {
            text: text.into(),
            error,
            born: Instant::now(),
        });
    }

    /// Whether a real tracker is listening for commands.
    pub(super) fn control_available(&self) -> bool {
        self.control.is_some()
    }

    pub(super) fn send(&mut self, cmd: Cmd) {
        match &self.control {
            Some(control) => control.send(cmd),
            None => self.pretend(cmd),
        }
    }

    /// Without a tracker (demo mode) commands are acted out on the invented
    /// data so that every button can be tried.
    fn pretend(&mut self, cmd: Cmd) {
        let now = self.now();
        let Ok(mut status) = self.shared.status.lock() else {
            return;
        };
        match cmd {
            Cmd::Pause { until } => {
                status.state = TrackState::Paused;
                status.paused_until = Some(until);
            }
            Cmd::Resume => {
                status.state = TrackState::Active;
                status.paused_until = None;
            }
            Cmd::StartTimer { task, project } => {
                status.timer = Some(RunningTimer {
                    id: crate::model::new_id(),
                    // Pretend it has been running for a while: the demo clock stands still.
                    start: now - 754,
                    task,
                    project,
                });
            }
            Cmd::StopTimer => status.timer = None,
            Cmd::StartFocus { minutes } => {
                status.focus = Some(RunningFocus {
                    id: crate::model::new_id(),
                    start: now - 420,
                    until: now - 420 + minutes as i64 * 60,
                });
            }
            Cmd::StopFocus => status.focus = None,
            Cmd::LabelAway { id, label } => {
                status.returned.retain(|r| r.id != id);
                drop(status);
                if let Ok(mut store) = self.shared.store.lock() {
                    match label {
                        None => {
                            let _ = store.delete(id);
                        }
                        Some(label) => {
                            if let Some(mut b) = store.get(id) {
                                b.away = label;
                                let _ = store.put(&b);
                            }
                        }
                    }
                }
            }
            Cmd::Settings(_) | Cmd::Flush => {}
        }
    }

    /// Saves changed settings, recompiles the rules and tells the tracker.
    fn commit_settings(&mut self, ctx: &egui::Context) {
        if self.settings == self.saved_settings {
            return;
        }
        if self.settings.theme != self.saved_settings.theme {
            theme::apply_choice(ctx, self.settings.theme);
        }
        self.settings.repair();
        self.classifier = Classifier::new(&self.settings);
        self.settings_rev += 1;
        self.saved_settings = self.settings.clone();
        if self.demo {
            return;
        }
        if let Err(e) = self.settings.save(&self.shared.dir) {
            self.toast(format!("Could not save the settings: {e}"), true);
        }
        self.send(Cmd::Settings(Box::new(self.settings.clone())));
    }

    fn run_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Go(tab) => self.tab = tab,
                Action::OpenTimeline { day, block } => {
                    self.tab = Tab::Timeline;
                    self.timeline.day = Some(day);
                    self.timeline.focus = block;
                    self.timeline.highlight = block;
                    self.timeline.checked.clear();
                }
                Action::OpenReport { from, to } => {
                    self.tab = Tab::Reports;
                    self.reports_view.open_range(from, to);
                }
                Action::Export { from, to } => {
                    self.dialog = Some(Dialog::export(from, to, ExportKind::Report, None));
                }
                Action::AddRule(rule) => {
                    let name = self.settings.category_name(&rule.category).to_string();
                    self.toast(format!("Rule added: {} is {name}", rule.pattern), false);
                    self.settings.rules.insert(0, rule);
                }
                Action::NewRule { field, pattern } => {
                    self.tab = Tab::Categories;
                    let category = self
                        .cats
                        .selected
                        .clone()
                        .unwrap_or_else(|| crate::model::OTHER.to_string());
                    self.dialog = Some(Dialog::rule(field, &pattern, &category));
                }
                Action::Cmd(cmd) => self.send(cmd),
                Action::Toast(text, error) => self.toast(text, error),
            }
        }
    }

    /// Requests from the tracker: show a page, or exit.
    fn handle_requests(&mut self, ctx: &egui::Context) {
        if self.shared.quit.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let show = self.shared.show.lock().ok().and_then(|mut s| s.take());
        if let Some(page) = show {
            if page == "away" {
                self.away_prompt = true;
            } else if !page.is_empty() {
                self.apply_view(&page);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        ctx.request_repaint();
        shot.frames += 1;
        if shot.frames == 1 {
            ctx.set_pixels_per_point(1.0);
        }
        if shot.frames == 3 {
            // Now that one point is one pixel, this gives an exact 1280 x 800 image.
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(1280.0, 800.0)));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            if let Some(parent) = shot.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let result = image::save_buffer(
                &shot.path,
                image.as_raw(),
                w as u32,
                h as u32,
                image::ColorType::Rgba8,
            );
            if let Err(e) = result {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.shot = None;
            return;
        }
        if !shot.requested && shot.frames > 40 {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }

    // ----------------------------------------------------------------------------------
    // Frame: header and navigation
    // ----------------------------------------------------------------------------------

    fn status_pill(&self, ui: &mut Ui) {
        let p = pal(ui);
        let (color, text) = match self.status.state {
            TrackState::Active => {
                let app = if self.status.app.is_empty() {
                    "Tracking".to_string()
                } else {
                    format!("Tracking  ·  {}", self.status.app)
                };
                (p.good, app)
            }
            TrackState::Idle => (p.warn, "Away".to_string()),
            TrackState::Locked => (p.warn, "Locked".to_string()),
            TrackState::Asleep => (p.warn, "Asleep".to_string()),
            TrackState::Paused => {
                let until = match self.status.paused_until {
                    Some(t) if t != i64::MAX => {
                        format!("Paused until {}", timeutil::fmt_hm(t, &self.tz))
                    }
                    _ => "Paused".to_string(),
                };
                (p.danger, until)
            }
            TrackState::Off => (p.faint, "Not tracking".to_string()),
        };
        let width = widgets::text_width(ui, &text, semibold(12.5), 230.0);
        let size = vec2(width + 34.0, 26.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(13), color.gamma_multiply(0.14));
        ui.painter()
            .circle_filled(rect.left_center() + vec2(13.0, 0.0), 4.0, color);
        let text_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 23.0, rect.top()),
            egui::pos2(rect.left() + 23.0 + width + 1.0, rect.bottom()),
        );
        widgets::text_in(ui, text_rect, &text, semibold(12.5), color);
        resp.on_hover_text(
            "What the tracker is doing right now. Everything it records stays in a folder on this PC.",
        );
    }

    fn header(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        let now = self.now();
        egui::TopBottomPanel::top("header")
            .exact_height(64.0)
            .frame(
                Frame::new()
                    .fill(p.rail)
                    .inner_margin(Margin::symmetric(18, 0)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().hline(
                    full.x_range().expand(18.0),
                    full.bottom(),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.horizontal_centered(|ui| {
                    // Brand
                    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new("HourForge").font(semibold(19.0)));
                    if self.demo {
                        widgets::badge(ui, "DEMO", p.info)
                            .on_hover_text("Invented data. Nothing is tracked or saved.");
                    }
                    ui.add_space(190.0 - ui.min_rect().width().min(190.0));
                    self.status_pill(ui);

                    // Right side, added right to left: About, pause, timer.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::INFO, "About HourForge").clicked() {
                            self.about_open = true;
                        }
                        self.pause_menu(ui, now);
                        ui.add_space(6.0);
                        self.timer_controls(ui, now);
                    });
                });
            });
    }

    fn pause_menu(&mut self, ui: &mut Ui, now: i64) {
        let p = pal(ui);
        if self.status.state == TrackState::Paused {
            if widgets::ghost_button(ui, &Icon::PLAY.with("Resume tracking")).clicked() {
                self.send(Cmd::Resume);
            }
            return;
        }
        let tomorrow = self.tz.day_start(self.tz.day_of(now) + 1);
        let mut pick = None;
        ui.menu_button(
            RichText::new(Icon::PAUSE.with("Pause")).color(p.text),
            |ui| {
                ui.set_min_width(190.0);
                ui.label(
                    RichText::new("Stop recording for")
                        .size(12.0)
                        .color(p.faint),
                );
                for (label, until) in [
                    ("15 minutes", now + 15 * 60),
                    ("1 hour", now + 3600),
                    ("Until tomorrow", tomorrow),
                    ("Until I resume", i64::MAX),
                ] {
                    if ui.button(label).clicked() {
                        pick = Some(until);
                        ui.close_menu();
                    }
                }
            },
        );
        if let Some(until) = pick {
            self.send(Cmd::Pause { until });
        }
    }

    fn timer_controls(&mut self, ui: &mut Ui, now: i64) {
        let p = pal(ui);
        if let Some(timer) = self.status.timer.clone() {
            if widgets::danger_button(ui, &Icon::STOP.with("Stop"), true).clicked() {
                self.send(Cmd::StopTimer);
            }
            ui.label(
                RichText::new(timeutil::fmt_clock(now - timer.start))
                    .font(semibold(15.0))
                    .color(p.accent),
            );
            let mut name = if timer.task.is_empty() {
                "Timer running".to_string()
            } else {
                timer.task.clone()
            };
            if let Some(project) = timer.project.as_deref().filter(|x| !x.is_empty()) {
                name = format!("{name}  ·  {}", self.settings.project_name(project));
            }
            let (rect, _) = ui.allocate_exact_size(vec2(230.0, 30.0), Sense::hover());
            widgets::text_in_right(ui, rect, &name, regular(14.0), p.muted);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        } else {
            if widgets::primary_button(ui, &Icon::PLAY.with("Start timer"), true)
                .on_hover_text(format!(
                    "Time an explicit task alongside the automatic tracking{}",
                    if self.settings.hotkey.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", self.settings.hotkey)
                    }
                ))
                .clicked()
            {
                self.toggle_timer();
            }
            // Project of the task.
            let shown = match self.settings.last_task_project.as_deref() {
                Some(id) if self.settings.project(id).is_some() => {
                    self.settings.project_name(id).to_string()
                }
                _ => "No project".to_string(),
            };
            let mut pick: Option<Option<String>> = None;
            // A right-to-left project name is drawn over an empty button.
            let button_text = if super::bidi::needs_native(&shown) {
                " ".repeat(
                    ((widgets::text_width(ui, &shown, regular(14.0), 160.0) / 4.0) as usize).max(4),
                )
            } else {
                shown.clone()
            };
            let menu = ui.menu_button(RichText::new(button_text).color(p.muted), |ui| {
                ui.set_min_width(190.0);
                if ui.button("No project").clicked() {
                    pick = Some(None);
                    ui.close_menu();
                }
                for pr in self.settings.projects.iter().filter(|x| !x.archived) {
                    if widgets::menu_item(ui, &pr.name).clicked() {
                        pick = Some(Some(pr.id.clone()));
                        ui.close_menu();
                    }
                }
            });
            if super::bidi::needs_native(&shown) {
                let rect = menu.response.rect.shrink2(vec2(6.0, 0.0));
                widgets::text_in(ui, rect, &shown, regular(14.0), p.muted);
            }
            if let Some(value) = pick {
                self.settings.last_task_project = value;
            }
            super::bidi::show_edit(ui, &mut self.timer_task, 14.5, p.text, |edit| {
                edit.hint_text(RichText::new("What are you working on?").color(p.faint))
                    .desired_width(200.0)
                    .margin(Margin::symmetric(10, 7))
                    .font(regular(14.5))
            });
        }
    }

    /// Starts the timer with the task typed in the header, or stops it.
    fn toggle_timer(&mut self) {
        if self.status.timer.is_some() {
            self.send(Cmd::StopTimer);
            return;
        }
        let task = self.timer_task.trim().to_string();
        self.settings.last_task = task.clone();
        self.send(Cmd::StartTimer {
            task,
            project: self.settings.last_task_project.clone(),
        });
    }

    /// Keyboard shortcuts (listed in [`SHORTCUTS`], shown with F1).
    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        if self.shot.is_some() {
            return;
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if pressed(Modifiers::NONE, Key::F1) {
            if matches!(self.dialog, Some(Dialog::Shortcuts)) {
                self.dialog = None;
            } else if self.dialog.is_none() {
                self.dialog = Some(Dialog::Shortcuts);
            }
        }
        if self.dialog.is_some() || self.about_open {
            return;
        }
        const PAGES: [(Key, Tab); 9] = [
            (Key::Num1, Tab::Today),
            (Key::Num2, Tab::Timeline),
            (Key::Num3, Tab::Reports),
            (Key::Num4, Tab::Review),
            (Key::Num5, Tab::Categories),
            (Key::Num6, Tab::Projects),
            (Key::Num7, Tab::Goals),
            (Key::Num8, Tab::Privacy),
            (Key::Num9, Tab::Settings),
        ];
        for (key, tab) in PAGES {
            if pressed(Modifiers::COMMAND, key) {
                self.tab = tab;
            }
        }
        let today = self.today();
        // The day in view, for the shortcuts that act on one.
        let day = match self.tab {
            Tab::Today => self.today_view.day_shown(today),
            Tab::Timeline => self.timeline.day.unwrap_or(today),
            _ => today,
        };
        if pressed(Modifiers::COMMAND, Key::E) {
            self.dialog = Some(Dialog::export(day, day, ExportKind::Report, None));
        }
        if pressed(Modifiers::COMMAND, Key::N) {
            self.tab = Tab::Timeline;
            self.timeline.day = Some(day);
            self.dialog = Some(Dialog::manual(day, self.now(), &self.tz));
        }
        if pressed(Modifiers::COMMAND, Key::F) {
            self.tab = Tab::Timeline;
            self.timeline.focus_search = true;
        }
        if pressed(Modifiers::COMMAND, Key::T) {
            self.toggle_timer();
        }
        let step = if pressed(Modifiers::ALT, Key::ArrowLeft) {
            Some(day - 1)
        } else if pressed(Modifiers::ALT, Key::ArrowRight) {
            Some((day + 1).min(today))
        } else if pressed(Modifiers::ALT, Key::Home) {
            Some(today)
        } else {
            None
        };
        if let Some(target) = step {
            match self.tab {
                Tab::Today => self.today_view.show_day(target, today),
                Tab::Timeline if self.timeline.day != Some(target) => {
                    self.timeline.day = Some(target);
                    self.timeline.checked.clear();
                }
                _ => {}
            }
        }
    }

    fn rail(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        self.review_load();
        let waiting = self.review.rows.len();
        egui::SidePanel::left("rail")
            .exact_width(208.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(p.rail)
                    .inner_margin(Margin::symmetric(10, 14)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().vline(
                    full.right() + 10.0,
                    full.y_range().expand(14.0),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.spacing_mut().item_spacing.y = 4.0;
                let items: [(Tab, Icon, &str); 9] = [
                    (Tab::Today, Icon::HOME, "Today"),
                    (Tab::Timeline, Icon::LIST, "Timeline"),
                    (Tab::Reports, Icon::CHART, "Reports"),
                    (Tab::Review, Icon::CHECK, "Review"),
                    (Tab::Categories, Icon::TAG, "Categories"),
                    (Tab::Projects, Icon::BRIEFCASE, "Projects"),
                    (Tab::Goals, Icon::FLAG, "Goals & focus"),
                    (Tab::Privacy, Icon::LOCK, "Privacy"),
                    (Tab::Settings, Icon::SETTINGS, "Settings"),
                ];
                for (tab, icon, label) in items {
                    if matches!(tab, Tab::Categories | Tab::Privacy) {
                        ui.add_space(8.0);
                        widgets::divider(ui);
                        ui.add_space(8.0);
                    }
                    let active = self.tab == tab;
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                    if active {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(9), p.selected);
                        let bar = Rect::from_min_size(
                            rect.left_center() - vec2(0.0, 9.0),
                            vec2(3.0, 18.0),
                        );
                        ui.painter()
                            .rect_filled(bar, CornerRadius::same(2), p.accent);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(9), p.hover);
                    }
                    let fg = if active { p.text } else { p.muted };
                    ui.painter().text(
                        rect.left_center() + vec2(24.0, 0.0),
                        Align2::CENTER_CENTER,
                        icon.glyph(),
                        regular(16.0),
                        if active { p.accent } else { fg },
                    );
                    ui.painter().text(
                        rect.left_center() + vec2(46.0, -1.0),
                        Align2::LEFT_CENTER,
                        label,
                        if active {
                            semibold(14.5)
                        } else {
                            regular(14.5)
                        },
                        fg,
                    );
                    if tab == Tab::Review && waiting > 0 {
                        // How many programs and websites wait to be sorted.
                        let badge = Rect::from_min_max(
                            egui::pos2(rect.right() - 44.0, rect.top()),
                            egui::pos2(rect.right() - 8.0, rect.bottom()),
                        );
                        let text = if waiting > 99 {
                            "99+".to_string()
                        } else {
                            waiting.to_string()
                        };
                        let galley =
                            ui.painter()
                                .layout_no_wrap(text, semibold(11.5), p.accent);
                        let pill = Rect::from_center_size(
                            egui::pos2(badge.right() - galley.size().x / 2.0 - 7.0, badge.center().y),
                            vec2(galley.size().x + 14.0, 20.0),
                        );
                        ui.painter().rect_filled(
                            pill,
                            CornerRadius::same(10),
                            p.accent.gamma_multiply(0.16),
                        );
                        ui.painter().galley(
                            egui::pos2(
                                pill.center().x - galley.size().x / 2.0,
                                pill.center().y - galley.size().y / 2.0,
                            ),
                            galley,
                            p.accent,
                        );
                    }
                    if resp.clicked() {
                        self.tab = tab;
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                .size(12.0)
                                .color(p.faint),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(Icon::LOCK.with("Nothing leaves this PC"))
                                .size(12.0)
                                .color(p.good),
                        )
                        .on_hover_text(
                            "HourForge makes no network requests at all. Everything it records is in a folder on this PC that you can open, export or delete.",
                        );
                    });
                });
            });
    }

    // ----------------------------------------------------------------------------------
    // About and toasts
    // ----------------------------------------------------------------------------------

    fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let p = pal_ctx(ctx);
        let mut close = false;
        let mut shortcuts = false;
        let modal = egui::Modal::new(egui::Id::new("about"))
            .frame(
                Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(Margin::same(28)),
            )
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("HourForge").font(semibold(26.0)));
                    ui.label(
                        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                            .color(p.muted),
                    );
                    ui.add_space(10.0);
                    ui.label("A private, automatic time tracker.");
                    ui.label(
                        RichText::new(
                            "It records which programs and windows you worked in, on this PC only. It makes no network requests at all.",
                        )
                        .color(p.muted),
                    );
                    ui.add_space(10.0);
                    ui.label(RichText::new("Developed by Zheer Khalid").font(semibold(15.0)));
                    ui.add_space(2.0);
                    ui.hyperlink_to(
                        RichText::new("zheer.dev").color(p.accent),
                        "https://zheer.dev",
                    );
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(
                            "Runs with your normal rights. Titles of windows that run as administrator may not be available.",
                        )
                        .size(12.0)
                        .color(p.faint),
                    );
                    ui.label(
                        RichText::new("Free and open source under the MIT License.")
                            .size(12.0)
                            .color(p.faint),
                    );
                    ui.add_space(14.0);
                    ui.allocate_ui_with_layout(
                        vec2(250.0, 36.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            if widgets::ghost_button(ui, "Keyboard shortcuts")
                                .on_hover_text("Also with F1")
                                .clicked()
                            {
                                close = true;
                                shortcuts = true;
                            }
                            if widgets::primary_button(ui, "Close", true).clicked() {
                                close = true;
                            }
                        },
                    );
                });
            });
        if close || modal.should_close() {
            self.about_open = false;
        }
        if shortcuts {
            self.dialog = Some(Dialog::Shortcuts);
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|t| t.born.elapsed().as_secs_f32() < if t.error { 8.0 } else { 3.5 });
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let p = pal_ctx(ctx);
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
                    for t in self.toasts.iter().rev().take(4) {
                        let color = if t.error { p.danger } else { p.accent };
                        Frame::new()
                            .fill(p.card)
                            .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.7)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.horizontal(|ui| {
                                    let icon = if t.error { Icon::ERROR } else { Icon::DONE };
                                    ui.label(RichText::new(icon.glyph()).color(color));
                                    ui.add(egui::Label::new(t.text.as_str()).wrap());
                                });
                            });
                    }
                });
            });
    }
}

/// Every keyboard shortcut, as shown in the list that F1 opens.
pub const SHORTCUTS: [(&str, &str); 12] = [
    (
        "Ctrl+1 … Ctrl+9",
        "Open a page, in the order of the list on the left",
    ),
    (
        "Alt+←  /  Alt+→",
        "Previous / next day (Today and Timeline)",
    ),
    ("Alt+Home", "Back to today"),
    ("Ctrl+F", "Search the day in the timeline"),
    ("Ctrl+N", "Add time by hand"),
    ("Ctrl+E", "Export the day in view"),
    ("Ctrl+T", "Start or stop the timer"),
    ("Double-click a row", "Edit that block in the timeline"),
    ("Enter", "Confirm a form (never a deletion)"),
    ("Esc", "Close a dialog or a menu"),
    ("F1", "Show this list"),
    (
        "Timer hotkey",
        "Works everywhere in Windows; set it in Settings",
    ),
];

/// The standard page header: title, subtitle and right-aligned actions.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, actions: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).font(semibold(24.0)));
            ui.label(RichText::new(subtitle).color(p.muted));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), actions);
    });
    ui.add_space(6.0);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.status = self.shared.status();
        self.handle_requests(ctx);
        self.handle_screenshot(ctx);
        if !self.demo {
            self.icons.poll(ctx, &self.shared);
        }

        self.shortcuts(ctx);
        self.header(ctx);
        self.rail(ctx);

        let p = pal_ctx(ctx);
        let now = self.now();
        let mut actions = Vec::new();
        let mut dirty = false;
        let rev = self.rev();
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(24, 18)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Today | Tab::Reports => {
                    let mut cx = Cx {
                        shared: &self.shared,
                        settings: &mut self.settings,
                        settings_dirty: &mut dirty,
                        classifier: &self.classifier,
                        tz: &self.tz,
                        now,
                        today: self.tz.day_of(now),
                        demo: self.demo,
                        rev,
                        status: &self.status,
                        actions: &mut actions,
                        icons: &self.icons,
                    };
                    if self.tab == Tab::Today {
                        self.today_view.ui(ui, &mut cx);
                    } else {
                        self.reports_view.ui(ui, &mut cx);
                    }
                }
                Tab::Timeline => self.timeline_ui(ui),
                Tab::Review => self.review_ui(ui),
                Tab::Categories => self.categories_ui(ui),
                Tab::Projects => self.projects_ui(ui),
                Tab::Goals => self.goals_ui(ui),
                Tab::Privacy => self.privacy_ui(ui),
                Tab::Settings => self.settings_page_ui(ui),
            });
        self.run_actions(actions);
        self.commit_settings(ctx);

        self.dialogs_ui(ctx);
        self.about_dialog(ctx);
        self.toasts_ui(ctx);

        if !self.demo && self.shot.is_none() {
            // Keep clocks and "now" markers moving while the window is open.
            ctx.request_repaint_after(std::time::Duration::from_secs(20));
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Screenshots must not depend on where the mouse happens to be:
        // no hover highlights, no tooltips.
        if self.shot.is_some() {
            raw_input.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::PointerMoved(_)
                        | egui::Event::MouseMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                )
            });
            raw_input.events.push(egui::Event::PointerGone);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Ok(mut waker) = self.shared.waker.lock() {
            *waker = None;
        }
        self.send(Cmd::Flush);
        // The tracker keeps running: let go of history loaded for the pages.
        if let Ok(mut store) = self.shared.store.lock() {
            store.trim_cache();
        }
    }
}
