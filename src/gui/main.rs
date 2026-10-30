// OpenMemoryOptimizer - a simple memory optimizer for Windows
// Copyright (C) 2026 Lucian Caetano
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! OpenMemoryOptimizer: GUI memory optimizer for Windows.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod disk_page;
mod game_page;
#[cfg(debug_assertions)]
mod snapshot;
mod theme;
mod tray;
mod widgets;

use std::collections::VecDeque;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{
    pos2, vec2, Align, Align2, CornerRadius, Layout, Margin, Sense, Stroke, Ui, ViewportCommand,
};
use egui_plot::{Line, Plot, PlotPoints};

use open_memory_optimizer::auto::{AutoState, TICK};
use open_memory_optimizer::booster::Session;
use open_memory_optimizer::cpu::CpuSampler;
use open_memory_optimizer::i18n::{self, tr, Lang, Strings};
use open_memory_optimizer::instance::{self, Acquired, ShowRequests};
use open_memory_optimizer::memory::{self, MemStatus};
use open_memory_optimizer::optimizer::{self, Methods, Mode, Report};
use open_memory_optimizer::privilege;
use open_memory_optimizer::process::{ProcInfo, ProcStats};
use open_memory_optimizer::schedule::{self, Trigger};
use open_memory_optimizer::settings::{ScheduleForm, Settings};

use theme::{icons, *};
use widgets::*;

const HISTORY_SECS: f64 = 120.0;
const SAMPLE_EVERY: Duration = Duration::from_millis(500); // tmrkline interval
const SLOW_SAMPLE_EVERY: Duration = Duration::from_secs(2);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub enum Event {
    Step(Progress),
    Done(Option<ResultView>),
    AutoFired,
    Tray(tray::TrayMsg),
    Schedule(SchedMsg),
    /// Game Booster sample for a pick (`None`: the process exited).
    GameStats(u64, Option<ProcStats>),
    /// Game Booster finished applying for a PID (`None`: the process was gone).
    Boosted(u32, Option<Box<Session>>),
    /// The picked game exited and was started again under this process.
    GameReturned(u64, ProcInfo),
    /// Disk page worker messages.
    Disk(disk_page::DiskMsg),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
    Booster,
    Disk,
    Settings,
    Schedule,
    About,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AutoKind {
    LowMemory,
    Periodic,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TriggerKind {
    Daily,
    Weekly,
    Monthly,
    Idle,
    Logon,
}

impl TriggerKind {
    /// Order of `settings::ScheduleForm::kind`.
    const ALL: [TriggerKind; 5] = [
        TriggerKind::Daily,
        TriggerKind::Weekly,
        TriggerKind::Monthly,
        TriggerKind::Idle,
        TriggerKind::Logon,
    ];

    fn from_index(i: u8) -> Self {
        Self::ALL
            .get(i as usize)
            .copied()
            .unwrap_or(TriggerKind::Daily)
    }

    fn index(self) -> u8 {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0) as u8
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Err,
}

/// The line under the Stop button. Kept as a value so it is worded in the
/// language shown when it is drawn.
pub enum Progress {
    Preparing,
    Auto,
    Clipboard,
    Defrag,
    Trim,
    Standby,
    FileCache,
    /// A log line without friendlier wording.
    Other(String),
}

impl Progress {
    /// Maps an `optimizer::run` log line.
    fn from_log(m: &str) -> Self {
        [
            ("Clearing clipboard", Progress::Clipboard),
            ("Defragmenting", Progress::Defrag),
            ("Trimming working sets", Progress::Trim),
            ("Purging standby", Progress::Standby),
            ("Trimming system file cache", Progress::FileCache),
        ]
        .into_iter()
        .find(|(k, _)| m.starts_with(k))
        .map_or_else(|| Progress::Other(m.to_owned()), |(_, p)| p)
    }

    fn text<'a>(&'a self, t: &'static Strings) -> &'a str {
        match self {
            Progress::Preparing => t.progress_preparing,
            Progress::Auto => t.progress_auto,
            Progress::Clipboard => t.progress_clipboard,
            Progress::Defrag => t.progress_defrag,
            Progress::Trim => t.progress_trim,
            Progress::Standby => t.progress_standby,
            Progress::FileCache => t.progress_file_cache,
            Progress::Other(s) => s,
        }
    }
}

#[derive(Clone, Copy)]
enum StepName {
    Clipboard,
    DeepClean,
    OpenApps,
    CachedMemory,
    SystemCache,
}

impl StepName {
    fn text(self, t: &'static Strings) -> &'static str {
        match self {
            StepName::Clipboard => t.step_clipboard,
            StepName::DeepClean => t.step_deep_clean,
            StepName::OpenApps => t.step_open_apps,
            StepName::CachedMemory => t.step_cached_memory,
            StepName::SystemCache => t.step_system_cache,
        }
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Cleared,
    ClearFailed,
    Done,
    PartlyDone,
    AppsTrimmed(u32),
    Unreachable,
    NeedsAdmin,
    Unsupported,
    SkippedLimits,
}

impl Outcome {
    fn text(self, t: &'static Strings) -> String {
        match self {
            Outcome::Cleared => t.outcome_cleared.into(),
            Outcome::ClearFailed => t.outcome_clear_failed.into(),
            Outcome::Done => t.outcome_done.into(),
            Outcome::PartlyDone => t.outcome_partly_done.into(),
            Outcome::AppsTrimmed(n) => (t.outcome_apps_trimmed)(n),
            Outcome::Unreachable => t.outcome_unreachable.into(),
            Outcome::NeedsAdmin => t.outcome_needs_admin.into(),
            Outcome::Unsupported => t.outcome_unsupported.into(),
            Outcome::SkippedLimits => t.outcome_skipped_limits.into(),
        }
    }
}

/// Result of creating or removing the scheduled task.
#[derive(Clone, Copy)]
pub enum SchedMsg {
    Created,
    CreateFailed,
    Removed,
    NothingToRemove,
}

impl SchedMsg {
    fn ok(self) -> bool {
        matches!(self, SchedMsg::Created | SchedMsg::Removed)
    }

    fn text(self, t: &'static Strings) -> &'static str {
        match self {
            SchedMsg::Created => t.sched_created,
            SchedMsg::CreateFailed => t.sched_create_failed,
            SchedMsg::Removed => t.sched_removed,
            SchedMsg::NothingToRemove => t.sched_nothing_to_remove,
        }
    }
}

pub struct ResultView {
    mode: Mode,
    freed_kb: i64,
    before_kb: i64,
    after_kb: i64,
    elapsed: Duration,
    steps: Vec<(StepName, Outcome, Level)>,
    at: chrono_lite::Clock,
    /// Stopped with the Stop button before it finished.
    cancelled: bool,
}

/// Wall-clock HH:MM without pulling a date/time crate.
mod chrono_lite {
    #[derive(Clone, Copy)]
    pub struct Clock(u32, u32);
    impl Clock {
        pub fn now() -> Self {
            let mut st = unsafe { std::mem::zeroed() };
            unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st) };
            Clock(st.wHour as u32, st.wMinute as u32)
        }
    }
    impl std::fmt::Display for Clock {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:02}:{:02}", self.0, self.1)
        }
    }
}

impl ResultView {
    fn from_report(mode: Mode, r: &Report) -> Self {
        let mut steps = Vec::new();
        if let Some(ok) = r.clipboard_cleared {
            steps.push(if ok {
                (StepName::Clipboard, Outcome::Cleared, Level::Ok)
            } else {
                (StepName::Clipboard, Outcome::ClearFailed, Level::Warn)
            });
        }
        if let Some(d) = r.defrag {
            steps.push(if d.committed == d.requested {
                (StepName::DeepClean, Outcome::Done, Level::Ok)
            } else {
                (StepName::DeepClean, Outcome::PartlyDone, Level::Warn)
            });
        }
        match &r.trim {
            Some(Ok(t)) => steps.push((
                StepName::OpenApps,
                Outcome::AppsTrimmed(t.empty_ws_ok.max(t.set_ws_ok)),
                if t.opened * 2 >= t.processes {
                    Level::Ok
                } else {
                    Level::Warn
                },
            )),
            Some(Err(_)) => steps.push((StepName::OpenApps, Outcome::Unreachable, Level::Err)),
            None => {}
        }
        let cached = StepName::CachedMemory;
        match r.standby_status {
            Some(Some(0)) => steps.push((cached, Outcome::Cleared, Level::Ok)),
            Some(Some(_)) => steps.push((cached, Outcome::NeedsAdmin, Level::Err)),
            Some(None) => steps.push((cached, Outcome::Unsupported, Level::Err)),
            None => {}
        }
        match &r.file_cache {
            Some(Ok(fc)) => {
                let (outcome, lvl) = match fc.set_ok {
                    Some(true) => (Outcome::Cleared, Level::Ok),
                    Some(false) => (Outcome::NeedsAdmin, Level::Warn),
                    None => (Outcome::SkippedLimits, Level::Warn),
                };
                steps.push((StepName::SystemCache, outcome, lvl));
            }
            Some(Err(_)) => steps.push((StepName::SystemCache, Outcome::Unsupported, Level::Err)),
            None => {}
        }
        Self {
            mode,
            freed_kb: r.freed_kb(),
            before_kb: r.before_kb,
            after_kb: r.after_kb,
            elapsed: r.elapsed,
            steps,
            at: chrono_lite::Clock::now(),
            cancelled: r.cancelled,
        }
    }
}

/// State shared with the worker and auto-timer threads.
struct Shared {
    settings: Mutex<Settings>,
    tx: Mutex<Sender<Event>>,
    ctx: egui::Context,
}

impl Shared {
    fn send(&self, e: Event) {
        let _ = self.tx.lock().unwrap().send(e);
        self.ctx.request_repaint();
    }
}

/// Runs one pass on a background thread, like the original's worker thread.
fn spawn_optimization(shared: &Arc<Shared>, mode: Mode) {
    let methods = Methods::from_settings(&shared.settings.lock().unwrap());
    let shared = shared.clone();
    std::thread::spawn(move || {
        let mut log = |m: &str| shared.send(Event::Step(Progress::from_log(m)));
        let report = optimizer::run(mode, methods, &mut log);
        shared.send(Event::Done(
            report.map(|r| ResultView::from_report(mode, &r)),
        ));
    });
}

/// Port of tmrautoTimer on its own thread so it keeps running while hidden in the tray.
fn spawn_auto_timer(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let mut state = AutoState::default();
        let mut cpu = CpuSampler::default();
        loop {
            std::thread::sleep(TICK);
            let s = shared.settings.lock().unwrap().clone();
            let Ok(m) = memory::status() else { continue };
            let fire = state.tick(
                &s,
                memory::round_mb(m.avail_phys),
                memory::round_mb(m.total_phys),
                || cpu.sample(),
            );
            if fire {
                shared.send(Event::AutoFired);
                spawn_optimization(&shared, Mode::Auto);
            }
        }
    });
}

/// Brings the window back when the app is launched again while already running.
fn spawn_show_listener(shared: Arc<Shared>, requests: ShowRequests) {
    std::thread::spawn(move || {
        while requests.wait() {
            shared.send(Event::Tray(tray::TrayMsg::Show));
        }
    });
}

struct Toast {
    title: String,
    body: String,
    level: Level,
    born: Instant,
}

struct App {
    settings: Settings,
    config_path: PathBuf,
    shared: Arc<Shared>,
    events: Receiver<Event>,
    tray: Option<tray::Tray>,
    logo: egui::TextureHandle,
    page: Page,
    elevated: bool,
    visible: bool,
    quitting: bool,
    was_minimized: bool,

    mem: Option<MemStatus>,
    page_lists: Vec<(&'static str, Option<u64>)>,
    cpu: CpuSampler,
    cpu_pct: i64,
    history: VecDeque<(Instant, f64)>,
    last_sample: Option<Instant>,
    last_slow_sample: Option<Instant>,

    running: bool,
    /// Stop was clicked; waiting for the worker to reach a safe point.
    stopping: bool,
    step: Progress,
    last: Option<ResultView>,
    toast: Option<Toast>,

    trigger_kind: TriggerKind,
    sched_hour: u8,
    sched_minute: u8,
    sched_day: usize,
    sched_month_day: u8,
    sched_busy: bool,
    sched_msg: Option<SchedMsg>,

    game: game_page::GamePage,
    disk: disk_page::DiskPage,

    #[cfg(debug_assertions)]
    snap: Option<snapshot::Snap>,
}

#[cfg(debug_assertions)]
const PAGES: [(Page, &str); 12] = [
    (Page::Dashboard, "dashboard"),
    (Page::Booster, "booster"),
    (Page::Disk, "disk"),
    (Page::Settings, "settings"),
    (Page::Schedule, "schedule"),
    (Page::About, "about"),
    (Page::Dashboard, "dashboard_result"),
    (Page::Booster, "booster_active"),
    (Page::Disk, "disk_analyze"),
    (Page::Disk, "disk_clean"),
    (Page::Disk, "disk_optimize"),
    // Last: it leaves `running` set without a worker behind it.
    (Page::Dashboard, "dashboard_running"),
];

/// `MEMOPT_LANG=en|pt-BR` picks the language in snapshot mode (debug builds only).
fn language_override() -> Option<Lang> {
    #[cfg(debug_assertions)]
    if snapshot_mode() {
        return std::env::var("MEMOPT_LANG")
            .ok()
            .and_then(|v| Lang::from_code(&v));
    }
    None
}

fn snapshot_mode() -> bool {
    #[cfg(debug_assertions)]
    return snapshot::requested_dir().is_some();
    #[cfg(not(debug_assertions))]
    false
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, show_requests: Option<ShowRequests>) -> Self {
        theme::install(&cc.egui_ctx);
        // FormCreate: enable the four privileges before anything else.
        privilege::enable(&privilege::STARTUP_PRIVILEGES);

        let config_path = config_path();
        let settings = Settings::load(&config_path);
        i18n::set(language_override().unwrap_or_else(|| i18n::resolve(settings.language)));
        let form = settings.schedule;
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            settings: Mutex::new(settings.clone()),
            tx: Mutex::new(tx.clone()),
            ctx: cc.egui_ctx.clone(),
        });
        let tray = if snapshot_mode() {
            None
        } else {
            spawn_auto_timer(shared.clone());
            tray::create(tx, cc.egui_ctx.clone())
        };
        if let Some(requests) = show_requests {
            spawn_show_listener(shared.clone(), requests);
        }

        let mut app = Self {
            settings,
            config_path,
            shared,
            events: rx,
            tray,
            logo: cc.egui_ctx.load_texture(
                "app-logo",
                egui::ColorImage::from_rgba_unmultiplied([128, 128], &theme::app_icon_rgba(128)),
                egui::TextureOptions::LINEAR,
            ),
            page: Page::Dashboard,
            elevated: privilege::is_elevated(),
            visible: true,
            quitting: false,
            was_minimized: false,
            mem: None,
            page_lists: Vec::new(),
            cpu: CpuSampler::default(),
            cpu_pct: 0,
            history: VecDeque::new(),
            last_sample: None,
            last_slow_sample: None,
            running: false,
            stopping: false,
            step: Progress::Preparing,
            last: None,
            toast: None,
            trigger_kind: TriggerKind::from_index(form.kind),
            sched_hour: form.hour,
            sched_minute: form.minute,
            sched_day: form.weekday as usize,
            sched_month_day: form.month_day,
            sched_busy: false,
            sched_msg: None,
            game: Default::default(),
            disk: Default::default(),
            #[cfg(debug_assertions)]
            snap: snapshot::requested_dir()
                .map(|d| snapshot::Snap::new(d, PAGES.iter().map(|(_, n)| *n).collect())),
        };
        app.cpu.sample();
        app.sample(Instant::now());
        // `--minimized` (sign-in task): the window was created hidden; stay in the tray,
        // or show it after all if the tray icon could not be created.
        if start_minimized() {
            if app.tray.is_some() {
                app.visible = false;
            } else {
                cc.egui_ctx
                    .send_viewport_cmd(ViewportCommand::Visible(true));
            }
        }
        app
    }

    fn sample(&mut self, now: Instant) {
        if self.last_sample.is_none_or(|t| now - t >= SAMPLE_EVERY) {
            self.last_sample = Some(now);
            if let Ok(m) = memory::status() {
                let used = 100.0 * (1.0 - m.avail_phys as f64 / m.total_phys.max(1) as f64);
                self.history.push_back((now, used));
                while self
                    .history
                    .front()
                    .is_some_and(|(t, _)| (now - *t).as_secs_f64() > HISTORY_SECS + 1.0)
                {
                    self.history.pop_front();
                }
                self.mem = Some(m);
            }
        }
        if self
            .last_slow_sample
            .is_none_or(|t| now - t >= SLOW_SAMPLE_EVERY)
        {
            self.last_slow_sample = Some(now);
            self.page_lists = memory::page_lists();
            self.cpu_pct = self.cpu.sample().clamp(0, 100);
        }
    }

    fn settings_changed(&mut self) {
        *self.shared.settings.lock().unwrap() = self.settings.clone();
        if let Err(e) = self.settings.save(&self.config_path) {
            self.notify(tr().save_failed, &e.to_string(), Level::Err);
        }
    }

    fn notify(&mut self, title: &str, body: &str, level: Level) {
        self.toast = Some(Toast {
            title: title.to_owned(),
            body: body.to_owned(),
            level,
            born: Instant::now(),
        });
    }

    fn start(&mut self, mode: Mode) {
        if self.running {
            return;
        }
        if mode == Mode::Manual && !Methods::from_settings(&self.settings).any_selected() {
            let t = tr();
            self.notify(t.nothing_to_clean, t.nothing_to_clean_body, Level::Warn);
            return;
        }
        self.running = true;
        self.stopping = false;
        self.step = Progress::Preparing;
        spawn_optimization(&self.shared, mode);
    }

    fn stop(&mut self) {
        if self.running && !self.stopping {
            self.stopping = true;
            optimizer::cancel();
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.visible = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn tray_enabled(&self) -> bool {
        self.settings.min_to_tray && self.tray.is_some() && !self.quitting
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        self.visible = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    fn minimize(&mut self, ctx: &egui::Context) {
        if self.tray_enabled() {
            self.hide_to_tray(ctx);
        } else {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
    }

    fn hide_or_close(&mut self, ctx: &egui::Context) {
        if self.tray_enabled() {
            self.hide_to_tray(ctx);
        } else {
            self.quitting = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        let t = tr();
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::Step(s) => self.step = s,
                Event::AutoFired => {
                    self.running = true;
                    self.stopping = false;
                    self.step = Progress::Auto;
                }
                Event::Done(None) => {
                    self.running = false;
                    self.stopping = false;
                    self.notify(t.already_running, t.already_running_body, Level::Warn);
                }
                Event::Done(Some(r)) => {
                    self.running = false;
                    self.stopping = false;
                    if r.cancelled {
                        let body = if r.freed_kb > 0 {
                            (t.freed_before_stop)(&t.size(r.freed_kb))
                        } else {
                            t.nothing_freed_body.to_owned()
                        };
                        self.notify(t.cleanup_stopped, &body, Level::Warn);
                        continue;
                    }
                    let (title, level) = if r.freed_kb > 0 {
                        ((t.freed)(&t.size(r.freed_kb)), Level::Ok)
                    } else {
                        (t.nothing_freed.to_owned(), Level::Warn)
                    };
                    if let Some(t) = &self.tray {
                        let _ = t
                            .icon
                            .set_tooltip(Some(format!("OpenMemoryOptimizer — {title} ({})", r.at)));
                    }
                    if self.settings.show_notice {
                        let body = (t.available_change)(
                            &t.size(r.before_kb),
                            &t.size(r.after_kb),
                            r.elapsed.as_secs_f64(),
                        );
                        self.notify(&title, &body, level);
                    }
                    self.last = Some(r);
                }
                Event::Tray(tray::TrayMsg::Show) => self.show_window(ctx),
                Event::Tray(tray::TrayMsg::Optimize) => self.start(Mode::Manual),
                Event::Tray(tray::TrayMsg::Quit) => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                Event::Schedule(msg) => {
                    self.sched_busy = false;
                    self.sched_msg = Some(msg);
                }
                Event::GameStats(generation, s) => self.game_stats(generation, s),
                Event::Boosted(pid, s) => self.game_boosted(pid, s),
                Event::GameReturned(generation, p) => self.game_returned(generation, p),
                Event::Disk(m) => self.disk_event(m),
            }
        }
    }

    // ---------------------------------------------------------------- chrome

    fn title_bar(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let drag = ui.interact(
            rect,
            egui::Id::unique("titlebar_drag"),
            Sense::click_and_drag(),
        );
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        ui.horizontal_centered(|ui| {
            ui.add_space(14.0);
            let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
            ui.painter().image(
                self.logo.id(),
                r,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            ui.add_space(2.0);
            label(ui, "OpenMemoryOptimizer", bold(15.0), TEXT);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(6.0);
                if titlebar_button(ui, icons::CLOSE, true).clicked() {
                    self.hide_or_close(ui.ctx());
                }
                if titlebar_button(ui, icons::MINIMIZE, false).clicked() {
                    self.minimize(ui.ctx());
                }
                ui.add_space(12.0);
                if self.elevated {
                    chip(ui, Some(icons::SHIELD), tr().admin_mode, GREEN);
                } else {
                    chip(ui, Some(icons::WARNING), tr().limited_mode, AMBER)
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        let t = tr();
        for (page, icon, text) in [
            (Page::Dashboard, icons::DASHBOARD, t.nav_overview),
            (Page::Booster, icons::GAME, t.nav_booster),
            (Page::Disk, icons::DRIVE, t.nav_disk),
            (Page::Settings, icons::SETTINGS, t.nav_settings),
            (Page::Schedule, icons::CLOCK, t.nav_schedule),
            (Page::About, icons::INFO, t.nav_about),
        ] {
            if nav_item(ui, icon, text, self.page == page).clicked() {
                self.page = page;
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(6.0);
            let s = &self.settings;
            let (txt, color) = if !s.auto_opt {
                (t.auto_off.to_owned(), FAINT)
            } else if s.lowmem_opt {
                ((t.auto_below)(s.low_mem_percent), GREEN)
            } else {
                ((t.auto_every)(s.period_minutes), GREEN)
            };
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, color);
                label(ui, &txt, regular(12.0), MUTED);
            });
        });
    }

    fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
        label(ui, title, semibold(24.0), TEXT);
        ui.add_space(-8.0);
        label(ui, subtitle, regular(13.5), MUTED);
        ui.add_space(6.0);
    }

    // ------------------------------------------------------------- dashboard

    /// Spinner and the current step, centered under the Stop button. Up to two
    /// lines, cut with "…" so long steps stay inside the card.
    fn progress_line(&self, ui: &mut Ui) {
        let (spin, gap) = (14.0, 8.0);
        let w = ui.available_width();
        let text = fitted_text(
            ui.painter(),
            self.step.text(tr()),
            regular(12.5),
            MUTED,
            w - spin - gap,
            2,
        );
        let h = text.size().y.max(spin);
        let row_h = text.size().y / text.rows.len().max(1) as f32;
        let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let x = r.center().x - (spin + gap + text.size().x) / 2.0;
        let spin_rect = egui::Rect::from_min_size(
            pos2(x, r.top() + (row_h - spin).max(0.0) / 2.0),
            vec2(spin, spin),
        );
        ui.put(spin_rect, egui::Spinner::new().size(spin).color(CYAN));
        ui.painter()
            .galley(pos2(x + spin + gap, r.top()), text, MUTED);
    }

    fn dashboard(&mut self, ui: &mut Ui) {
        let t = tr();
        Self::page_header(ui, t.nav_overview, t.overview_sub);
        let Some(m) = self.mem else {
            label(ui, t.checking_memory, regular(14.0), MUTED);
            return;
        };
        let used = m.total_phys - m.avail_phys;
        let used_pct = (used as f64 / m.total_phys.max(1) as f64 * 100.0) as f32;
        let gap = 14.0;
        let full_w = ui.available_width();
        let left_w = 270.0;
        let right_w = full_w - left_w - gap;

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            // Gauge + optimize button
            card_sized(ui, vec2(left_w, 3.0 * 100.0 + 2.0 * gap), |ui| {
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(172.0, 172.0), Sense::hover());
                    let shown =
                        ui.ctx()
                            .animate_value_with_time(egui::Id::unique("gauge"), used_pct, 0.6);
                    ring(ui, r.center(), 72.0, 13.0, shown / 100.0, CYAN, VIOLET);
                    let p = ui.painter();
                    p.text(
                        r.center() - vec2(0.0, 8.0),
                        Align2::CENTER_CENTER,
                        format!("{shown:.0}%"),
                        bold(38.0),
                        TEXT,
                    );
                    p.text(
                        r.center() + vec2(0.0, 24.0),
                        Align2::CENTER_CENTER,
                        t.gauge_in_use,
                        regular(13.0),
                        MUTED,
                    );
                    ui.add_space(6.0);
                    let btn = vec2(ui.available_width(), 46.0);
                    if self.running {
                        let text = if self.stopping { t.stopping } else { t.stop };
                        if stop_button(ui, btn, text, Some(icons::STOP), !self.stopping).clicked() {
                            self.stop();
                        }
                        ui.add_space(8.0);
                        self.progress_line(ui);
                    } else if primary_button(ui, btn, t.free_up_memory, Some(icons::BOLT), true)
                        .clicked()
                    {
                        self.start(Mode::Manual);
                    }
                });
            });

            // Stat grid 2 x 3
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                let cw = (right_w - gap) / 2.0;
                let ch = 100.0;
                let kb = |b: u64| t.size((b / 1024) as i64);
                let list = |name: &str| {
                    self.page_lists
                        .iter()
                        .filter(|(l, _)| l.starts_with(name))
                        .filter_map(|(_, v)| *v)
                        .reduce(|a, b| a + b)
                };
                let cached = list("Standby");
                let lvl = level_color(used_pct);
                let (status, status_sub) = if used_pct < 60.0 {
                    (t.status_healthy, t.status_healthy_sub)
                } else if used_pct < 85.0 {
                    (t.status_busy, t.status_busy_sub)
                } else {
                    (t.status_low, t.status_low_sub)
                };
                ui.horizontal_top(|ui| {
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        t.total_memory,
                        &kb(m.total_phys),
                        t.total_memory_sub,
                        VIOLET,
                    );
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        t.in_use,
                        &kb(used),
                        &(t.in_use_sub)(m.load_percent),
                        lvl,
                    );
                });
                ui.horizontal_top(|ui| {
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        t.available,
                        &kb(m.avail_phys),
                        t.available_sub,
                        GREEN,
                    );
                    stat_card(ui, vec2(cw, ch), t.status, status, status_sub, lvl);
                });
                ui.horizontal_top(|ui| {
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        t.cached,
                        &cached.map_or("—".into(), kb),
                        if cached.is_some() {
                            t.cached_sub
                        } else {
                            t.cached_unavailable
                        },
                        AMBER,
                    );
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        t.processor,
                        &format!("{}%", self.cpu_pct),
                        if self.cpu_pct <= 5 {
                            t.cpu_resting
                        } else {
                            t.cpu_working
                        },
                        if self.cpu_pct <= 5 { GREEN } else { CYAN },
                    );
                });
            });
        });

        ui.add_space(gap - ui.spacing().item_spacing.y);
        let h = (ui.available_height() - 1.0).max(150.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            card_sized(ui, vec2(full_w - left_w - gap, h), |ui| {
                ui.horizontal(|ui| {
                    card_title(ui, t.memory_use, Some(t.last_2_minutes));
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        let pct = t.decimal(format!("{used_pct:.1}"));
                        label(ui, &format!("{pct}%"), semibold(15.0), CYAN);
                    });
                });
                self.chart(ui);
            });
            card_sized(ui, vec2(left_w, h), |ui| self.last_result(ui));
        });
    }

    fn chart(&self, ui: &mut Ui) {
        let now = Instant::now();
        let pts: Vec<[f64; 2]> = self
            .history
            .iter()
            .map(|(t, v)| [-(now - *t).as_secs_f64(), *v])
            .collect();
        // Grow the window with the data so the first seconds are not a sliver.
        let span = pts
            .first()
            .map_or(20.0, |p| -p[0])
            .clamp(20.0, HISTORY_SECS);
        Plot::new("mem_plot")
            .height(ui.available_height())
            .show_background(false)
            .show_axes([false, true])
            .show_grid(false)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .show_x(false)
            .show_y(false)
            .include_x(-span)
            .include_x(0.0)
            .include_y(0.0)
            .include_y(100.0)
            .y_axis_min_width(28.0)
            .y_axis_formatter(|m, _| format!("{:.0}%", m.value))
            .show(ui, |pui| {
                pui.line(
                    Line::new(tr().in_use, PlotPoints::from(pts))
                        .color(CYAN)
                        .width(2.0)
                        .fill(0.0)
                        .fill_alpha(0.12),
                );
            });
    }

    fn last_result(&self, ui: &mut Ui) {
        let t = tr();
        card_title(ui, t.last_cleanup, None);
        let Some(r) = &self.last else {
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(t.no_cleanup_yet)
                        .font(regular(13.0))
                        .color(MUTED),
                )
                .wrap(),
            );
            return;
        };
        let color = if r.freed_kb > 0 { GREEN } else { AMBER };
        let sign = if r.freed_kb > 0 { "+" } else { "" };
        label(
            ui,
            &format!("{sign}{}", t.size(r.freed_kb)),
            bold(26.0),
            color,
        );
        ui.add_space(-8.0);
        let mode = if r.mode == Mode::Auto {
            t.mode_auto
        } else {
            t.mode_manual
        };
        label(
            ui,
            &(t.result_meta)(&r.at.to_string(), mode, r.elapsed.as_secs_f64()),
            regular(12.0),
            FAINT,
        );
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                for &(name, outcome, lvl) in &r.steps {
                    let c = match lvl {
                        Level::Ok => GREEN,
                        Level::Warn => AMBER,
                        Level::Err => RED,
                    };
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        let (d, _) = ui.allocate_exact_size(vec2(7.0, 16.0), Sense::hover());
                        ui.painter().circle_filled(d.center(), 3.5, c);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            label(ui, name.text(t), semibold(12.5), TEXT);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(outcome.text(t))
                                        .font(regular(12.0))
                                        .color(MUTED),
                                )
                                .wrap(),
                            );
                        });
                    });
                }
            });
    }

    // -------------------------------------------------------------- settings

    fn settings_page(&mut self, ui: &mut Ui) {
        let t = tr();
        Self::page_header(ui, t.nav_settings, t.settings_sub);
        let mut changed = false;
        let mut language_changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 14.0;
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            label(ui, t.language, semibold(14.0), TEXT);
                            label(ui, t.language_desc, regular(12.5), MUTED);
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if segmented(
                                ui,
                                &mut self.settings.language,
                                &[
                                    (Some(Lang::PtBr), "Português"),
                                    (Some(Lang::En), "English"),
                                    (None, t.language_auto),
                                ],
                            ) {
                                changed = true;
                                language_changed = true;
                            }
                        });
                    });
                });
                card(ui, |ui| {
                    card_title(ui, t.what_to_clean, Some(t.what_to_clean_sub));
                    ui.add_space(4.0);
                    let s = &mut self.settings;
                    changed |=
                        setting_toggle(ui, t.trim_title, t.trim_desc, &mut s.trim_working_sets);
                    ui.separator();
                    changed |=
                        setting_toggle(ui, t.standby_title, t.standby_desc, &mut s.clear_standby);
                    ui.separator();
                    changed |= setting_toggle(ui, t.defrag_title, t.defrag_desc, &mut s.defrag_mem);
                    ui.separator();
                    changed |= setting_toggle(
                        ui,
                        t.clipboard_title,
                        t.clipboard_desc,
                        &mut s.clear_clipboard,
                    );
                });

                card(ui, |ui| {
                    card_title(ui, t.auto_title, Some(t.auto_sub));
                    ui.add_space(4.0);
                    let s = &mut self.settings;
                    changed |=
                        setting_toggle(ui, t.auto_toggle, t.auto_toggle_desc, &mut s.auto_opt);
                    ui.add_enabled_ui(s.auto_opt, |ui| {
                        ui.add_space(4.0);
                        let mut kind = if s.lowmem_opt {
                            AutoKind::LowMemory
                        } else {
                            AutoKind::Periodic
                        };
                        if segmented(
                            ui,
                            &mut kind,
                            &[
                                (AutoKind::LowMemory, t.auto_low_memory),
                                (AutoKind::Periodic, t.auto_timer),
                            ],
                        ) {
                            s.lowmem_opt = kind == AutoKind::LowMemory;
                            s.period_opt = kind == AutoKind::Periodic;
                            changed = true;
                        }
                        ui.add_space(4.0);
                        if kind == AutoKind::LowMemory {
                            label(ui, t.clean_below, regular(13.0), MUTED);
                            let mut pct = s.low_mem_percent as i32;
                            if ui
                                .add(egui::Slider::new(&mut pct, 5..=95).suffix(" %"))
                                .changed()
                            {
                                s.low_mem_percent = pct as i64;
                                changed = true;
                            }
                            changed |=
                                setting_toggle(ui, t.idle_title, t.idle_desc, &mut s.idle_only);
                        } else {
                            ui.horizontal(|ui| {
                                label(ui, t.clean_every, regular(13.0), MUTED);
                                let mut min = s.period_minutes as i32;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut min)
                                            .range(1..=1440)
                                            .suffix(" min"),
                                    )
                                    .changed()
                                {
                                    s.period_minutes = min as i64;
                                    changed = true;
                                }
                            });
                        }
                    });
                });

                card(ui, |ui| {
                    card_title(ui, t.general, None);
                    ui.add_space(4.0);
                    let s = &mut self.settings;
                    changed |=
                        setting_toggle(ui, t.notice_title, t.notice_desc, &mut s.show_notice);
                    ui.separator();
                    changed |= setting_toggle(ui, t.tray_title, t.tray_desc, &mut s.min_to_tray);
                });
                label(
                    ui,
                    &self.config_path.display().to_string(),
                    regular(11.5),
                    FAINT,
                );
            });
        if changed {
            self.settings_changed();
        }
        if language_changed {
            i18n::set(i18n::resolve(self.settings.language));
            if let Some(tray) = &self.tray {
                tray.relabel();
            }
        }
    }

    // -------------------------------------------------------------- schedule

    fn current_trigger(&self) -> Trigger {
        const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        match self.trigger_kind {
            TriggerKind::Daily => {
                Trigger::Daily(format!("{:02}:{:02}", self.sched_hour, self.sched_minute))
            }
            TriggerKind::Weekly => Trigger::Weekly(DAYS[self.sched_day].into()),
            TriggerKind::Monthly => Trigger::Monthly(self.sched_month_day),
            TriggerKind::Idle => Trigger::OnIdle,
            TriggerKind::Logon => Trigger::OnLogon,
        }
    }

    fn run_schtasks(
        &mut self,
        mut cmd: std::process::Command,
        ok_msg: SchedMsg,
        err_msg: SchedMsg,
    ) {
        self.sched_busy = true;
        self.sched_msg = None;
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let result = cmd.creation_flags(CREATE_NO_WINDOW).output();
            let msg = match result {
                Ok(o) if o.status.success() => ok_msg,
                _ => err_msg,
            };
            shared.send(Event::Schedule(msg));
        });
    }

    fn schedule_page(&mut self, ui: &mut Ui) {
        let t = tr();
        Self::page_header(ui, t.nav_schedule, t.schedule_sub);
        card(ui, |ui| {
            card_title(ui, t.when_to_clean, None);
            ui.add_space(4.0);
            segmented(
                ui,
                &mut self.trigger_kind,
                &[
                    (TriggerKind::Daily, t.sched_daily),
                    (TriggerKind::Weekly, t.sched_weekly),
                    (TriggerKind::Monthly, t.sched_monthly),
                    (TriggerKind::Idle, t.sched_idle),
                    (TriggerKind::Logon, t.sched_logon),
                ],
            );
            ui.add_space(6.0);
            match self.trigger_kind {
                TriggerKind::Daily => {
                    ui.horizontal(|ui| {
                        label(ui, t.sched_time, regular(13.0), MUTED);
                        ui.add(
                            egui::DragValue::new(&mut self.sched_hour)
                                .range(0..=23)
                                .custom_formatter(|v, _| format!("{v:02}")),
                        );
                        label(ui, ":", semibold(14.0), MUTED);
                        ui.add(
                            egui::DragValue::new(&mut self.sched_minute)
                                .range(0..=59)
                                .custom_formatter(|v, _| format!("{v:02}")),
                        );
                    });
                }
                TriggerKind::Weekly => {
                    let mut day = self.sched_day;
                    let days: [(usize, &str); 7] = std::array::from_fn(|i| (i, t.weekdays[i]));
                    segmented(ui, &mut day, &days);
                    self.sched_day = day;
                }
                TriggerKind::Monthly => {
                    ui.horizontal(|ui| {
                        label(ui, t.sched_month_day, regular(13.0), MUTED);
                        ui.add(egui::DragValue::new(&mut self.sched_month_day).range(1..=31));
                    });
                }
                TriggerKind::Idle => {
                    label(ui, t.sched_idle_desc, regular(13.0), MUTED);
                }
                TriggerKind::Logon => {
                    label(ui, t.sched_logon_desc, regular(13.0), MUTED);
                }
            }
            ui.add_space(8.0);
            let exe = std::env::current_exe().unwrap_or_default();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let enabled = !self.sched_busy;
                let width = pill_width(ui, t.create_schedule, Some(icons::ADD)).max(170.0);
                if primary_button(
                    ui,
                    vec2(width, 40.0),
                    t.create_schedule,
                    Some(icons::ADD),
                    enabled,
                )
                .clicked()
                {
                    let cmd = schedule::create_command(&exe, &self.current_trigger());
                    self.run_schtasks(cmd, SchedMsg::Created, SchedMsg::CreateFailed);
                }
                if ghost_button(ui, t.remove_schedule, Some(icons::DELETE)).clicked() && enabled {
                    self.run_schtasks(
                        schedule::delete_command(),
                        SchedMsg::Removed,
                        SchedMsg::NothingToRemove,
                    );
                }
                if self.sched_busy {
                    ui.add(egui::Spinner::new().color(CYAN));
                }
            });
            // Remember the choices so the page shows them again next time.
            let form = ScheduleForm {
                kind: self.trigger_kind.index(),
                hour: self.sched_hour,
                minute: self.sched_minute,
                weekday: self.sched_day as u8,
                month_day: self.sched_month_day,
            };
            if form != self.settings.schedule {
                self.settings.schedule = form;
                self.settings_changed();
            }
            if let Some(msg) = self.sched_msg {
                ui.add_space(4.0);
                let (icon, color) = if msg.ok() {
                    (icons::CHECK, GREEN)
                } else {
                    (icons::WARNING, RED)
                };
                ui.horizontal(|ui| {
                    label(ui, icon, theme::icon(13.0), color);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(msg.text(t))
                                .font(regular(13.0))
                                .color(color),
                        )
                        .wrap(),
                    );
                });
            }
        });
    }

    // ----------------------------------------------------------------- about

    fn about_page(&mut self, ui: &mut Ui) {
        let t = tr();
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
            ui.painter().image(
                self.logo.id(),
                r,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(6.0);
                Self::page_header(
                    ui,
                    "OpenMemoryOptimizer",
                    &(t.version_line)(env!("CARGO_PKG_VERSION")),
                );
            });
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                card(ui, |ui| {
                    card_title(ui, t.what_happens, None);
                    ui.add_space(2.0);
                    for (n, title, body) in [
                        ("1", t.step_clipboard, t.about_clipboard),
                        ("2", t.step_deep_clean, t.about_deep_clean),
                        ("3", t.step_open_apps, t.about_open_apps),
                        ("4", t.step_cached_memory, t.about_cached_memory),
                        ("5", t.step_system_cache, t.about_system_cache),
                        ("6", t.step_result, t.about_result),
                    ] {
                        ui.horizontal_top(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                            ui.painter().circle_filled(
                                r.center(),
                                12.0,
                                ACCENT.gamma_multiply(0.25),
                            );
                            ui.painter().text(
                                r.center(),
                                Align2::CENTER_CENTER,
                                n,
                                semibold(12.0),
                                ACCENT_HI,
                            );
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                label(ui, title, semibold(13.5), TEXT);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(body).font(regular(12.5)).color(MUTED),
                                    )
                                    .wrap(),
                                );
                            });
                        });
                    }
                });
                ui.add_space(4.0);
                card(ui, |ui| {
                    card_title(ui, t.good_to_know, None);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(t.good_to_know_body)
                                .font(regular(13.0))
                                .color(MUTED),
                        )
                        .wrap(),
                    );
                });
                ui.add_space(4.0);
                card(ui, |ui| {
                    card_title(ui, t.license, None);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(t.license_body)
                                .font(regular(13.0))
                                .color(MUTED),
                        )
                        .wrap(),
                    );
                    ui.hyperlink_to(
                        egui::RichText::new(t.read_gpl).font(regular(13.0)),
                        "https://www.gnu.org/licenses/gpl-3.0.html",
                    );
                });
            });
    }

    /// Visual-QA fixture for the snapshot mode only (debug builds, MEMOPT_SNAPSHOT set);
    /// avoids running a real optimization just to render the result card.
    #[cfg(debug_assertions)]
    fn load_snapshot_fixture(&mut self) {
        // Ends at the memory available right now, so the toast matches the cards.
        let freed = 1_288_490;
        let after = self
            .mem
            .map_or(16 * 1024 * 1024, |m| (m.avail_phys / 1024) as i64);
        let before = after - freed;
        self.last = Some(ResultView {
            mode: Mode::Manual,
            freed_kb: freed,
            before_kb: before,
            after_kb: after,
            elapsed: Duration::from_millis(3240),
            steps: vec![
                (StepName::Clipboard, Outcome::Cleared, Level::Ok),
                (StepName::OpenApps, Outcome::AppsTrimmed(189), Level::Ok),
                (StepName::CachedMemory, Outcome::Cleared, Level::Ok),
                (StepName::SystemCache, Outcome::Cleared, Level::Ok),
            ],
            at: chrono_lite::Clock::now(),
            cancelled: false,
        });
        let t = tr();
        let title = (t.freed)(&t.size(freed));
        let body = (t.available_change)(&t.size(before), &t.size(after), 3.24);
        self.notify(&title, &body, Level::Ok);
    }

    fn toast(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.toast else { return };
        let age = t.born.elapsed().as_secs_f32();
        const LIFE: f32 = 5.0;
        if age > LIFE {
            self.toast = None;
            return;
        }
        let alpha = (age / 0.2).min(1.0).min((LIFE - age) / 0.4).clamp(0.0, 1.0);
        let slide = (1.0 - (age / 0.25).min(1.0)) * 16.0;
        let color = match t.level {
            Level::Ok => GREEN,
            Level::Warn => AMBER,
            Level::Err => RED,
        };
        let icon = match t.level {
            Level::Ok => icons::CHECK,
            _ => icons::WARNING,
        };
        egui::Area::new(egui::Id::unique("toast"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-22.0, -22.0 + slide))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::new()
                    .fill(CARD_HI)
                    .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(16, 12))
                    .shadow(egui::Shadow {
                        offset: [0, 10],
                        blur: 30,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(140),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.horizontal_top(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                            ui.painter().circle_filled(
                                r.center(),
                                15.0,
                                color.gamma_multiply(0.18),
                            );
                            ui.painter().text(
                                r.center(),
                                Align2::CENTER_CENTER,
                                icon,
                                theme::icon(14.0),
                                color,
                            );
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                label(ui, &t.title, semibold(14.0), TEXT);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&t.body)
                                            .font(regular(12.5))
                                            .color(MUTED),
                                    )
                                    .wrap(),
                                );
                            });
                        });
                    });
            });
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events(ctx);
        // Minimized from Windows (taskbar click, Win+Down): send it to the tray instead.
        // Only the transition counts, so a stale "minimized" right after restoring is ignored.
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if minimized && !self.was_minimized && self.visible && self.tray_enabled() {
            self.hide_to_tray(ctx);
        }
        self.was_minimized = minimized;
        if self.visible {
            self.sample(Instant::now());
            ctx.request_repaint_after(SAMPLE_EVERY);
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.settings.min_to_tray && self.tray.is_some() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.hide_or_close(&ctx);
            } else {
                self.quitting = true;
            }
        }

        egui::Panel::top("titlebar")
            .exact_size(48.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .stroke(Stroke::new(1.0, STROKE)),
            )
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::left("nav")
            .exact_size(206.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(Margin::symmetric(12, 12)),
            )
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(26, 20)),
            )
            .show(ui, |ui| match self.page {
                Page::Dashboard => self.dashboard(ui),
                Page::Booster => self.game_page(ui),
                Page::Disk => self.disk_page(ui),
                Page::Settings => self.settings_page(ui),
                Page::Schedule => self.schedule_page(ui),
                Page::About => self.about_page(ui),
            });
        self.toast(&ctx);
        #[cfg(debug_assertions)]
        if let Some(snap) = &mut self.snap {
            match snap.drive(&ctx) {
                snapshot::Step::Show(i) => {
                    self.page = PAGES[i].0;
                    if PAGES[i].1 == "dashboard_result" && self.last.is_none() {
                        self.load_snapshot_fixture();
                    }
                    if PAGES[i].1 == "booster_active" && !self.game.has_pick() {
                        self.toast = None;
                        self.load_game_fixture();
                    }
                    if PAGES[i].1.starts_with("disk_") {
                        self.toast = None;
                        self.load_disk_fixture(PAGES[i].1);
                    }
                    if PAGES[i].1 == "dashboard_running" && !self.running {
                        // The longest step text, to check that it stays inside the card.
                        self.toast = None;
                        self.running = true;
                        self.step = Progress::Defrag;
                    }
                }
                snapshot::Step::Wait => {}
                snapshot::Step::Finished => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("MemoryConfig.ini")))
        .unwrap_or_else(|| PathBuf::from("MemoryConfig.ini"))
}

/// `OpenMemoryOptimizer.exe --optimize`, used by the scheduled task: one silent
/// cleanup with the saved settings, no window.
fn run_scheduled_cleanup() {
    privilege::enable(&privilege::STARTUP_PRIVILEGES);
    let methods = Methods::from_settings(&Settings::load(&config_path()));
    if methods.any_selected() {
        optimizer::run(Mode::Manual, methods, &mut |_| {});
    }
}

/// Started by the sign-in task created by the installer: open hidden in the tray.
fn start_minimized() -> bool {
    !snapshot_mode() && std::env::args().skip(1).any(|a| a == "--minimized")
}

fn main() -> eframe::Result {
    if std::env::args().skip(1).any(|a| a == "--optimize") {
        run_scheduled_cleanup();
        return Ok(());
    }
    // One window and one tray icon: a second launch brings the first one to the
    // front instead (the sign-in task's `--minimized` launch just exits).
    let instance = if snapshot_mode() {
        None
    } else {
        match instance::acquire(instance::NAME) {
            Acquired::AlreadyRunning => {
                if !start_minimized() {
                    instance::request_show(instance::NAME);
                }
                return Ok(());
            }
            Acquired::First(i) => Some(i),
        }
    };
    let show_requests = instance.as_ref().and_then(|i| i.show_requests());
    let size = 256;
    let icon = egui::IconData {
        rgba: theme::app_icon_rgba(size),
        width: size,
        height: size,
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("OpenMemoryOptimizer")
        .with_inner_size([1000.0, 750.0])
        .with_resizable(false)
        .with_decorations(false)
        .with_visible(!start_minimized())
        .with_icon(icon);
    if snapshot_mode() {
        // Off-screen, never activated, no taskbar button: does not disturb the desktop.
        viewport = viewport
            .with_position([-4000.0, -4000.0])
            .with_active(false)
            .with_taskbar(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: !snapshot_mode(),
        ..Default::default()
    };
    let result = eframe::run_native(
        "OpenMemoryOptimizer",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc, show_requests)))),
    );
    // Hold the instance mutex until the window is gone.
    drop(instance);
    result
}
