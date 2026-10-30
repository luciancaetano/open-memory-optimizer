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
use open_memory_optimizer::cpu::CpuSampler;
use open_memory_optimizer::instance::{self, Acquired, ShowRequests};
use open_memory_optimizer::memory::{self, MemStatus};
use open_memory_optimizer::optimizer::{self, Methods, Mode, Report};
use open_memory_optimizer::privilege;
use open_memory_optimizer::schedule::{self, Trigger};
use open_memory_optimizer::settings::{ScheduleForm, Settings};
use open_memory_optimizer::win::format_kb;

use theme::{icons, *};
use widgets::*;

const HISTORY_SECS: f64 = 120.0;
const SAMPLE_EVERY: Duration = Duration::from_millis(500); // tmrkline interval
const SLOW_SAMPLE_EVERY: Duration = Duration::from_secs(2);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub enum Event {
    Step(String),
    Done(Option<ResultView>),
    AutoFired,
    Tray(tray::TrayMsg),
    Schedule(bool, String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
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

pub struct ResultView {
    mode: Mode,
    freed_kb: i64,
    before_kb: i64,
    after_kb: i64,
    elapsed: Duration,
    steps: Vec<(&'static str, String, Level)>,
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
            steps.push((
                "Clipboard",
                if ok { "Cleared" } else { "Couldn't clear it" }.to_owned(),
                if ok { Level::Ok } else { Level::Warn },
            ));
        }
        if let Some(d) = r.defrag {
            steps.push((
                "Deep clean",
                if d.committed == d.requested {
                    "Done".to_owned()
                } else {
                    "Partly done".to_owned()
                },
                if d.committed == d.requested {
                    Level::Ok
                } else {
                    Level::Warn
                },
            ));
        }
        match &r.trim {
            Some(Ok(t)) => steps.push((
                "Open apps",
                format!("{} apps slimmed down", t.empty_ws_ok.max(t.set_ws_ok)),
                if t.opened * 2 >= t.processes {
                    Level::Ok
                } else {
                    Level::Warn
                },
            )),
            Some(Err(_)) => steps.push(("Open apps", "Couldn't reach them".into(), Level::Err)),
            None => {}
        }
        match r.standby_status {
            Some(Some(0)) => steps.push(("Cached memory", "Cleared".into(), Level::Ok)),
            Some(Some(_)) => steps.push((
                "Cached memory",
                "Needs administrator rights".into(),
                Level::Err,
            )),
            Some(None) => steps.push(("Cached memory", "Not supported here".into(), Level::Err)),
            None => {}
        }
        match &r.file_cache {
            Some(Ok(fc)) => {
                let (txt, lvl) = match fc.set_ok {
                    Some(true) => ("Cleared", Level::Ok),
                    Some(false) => ("Needs administrator rights", Level::Warn),
                    None => ("Skipped (custom limits set)", Level::Warn),
                };
                steps.push(("System cache", txt.into(), lvl));
            }
            Some(Err(_)) => steps.push(("System cache", "Not supported here".into(), Level::Err)),
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
        let steps = [
            ("Clearing clipboard", "Clearing the clipboard…"),
            ("Defragmenting", "Deep cleaning… this can take a while"),
            ("Trimming working sets", "Freeing memory from open apps…"),
            ("Purging standby", "Clearing cached memory…"),
            ("Trimming system file cache", "Tidying up the system cache…"),
        ];
        let mut log = |m: &str| {
            let text = steps
                .iter()
                .find(|(k, _)| m.starts_with(k))
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| m.to_owned());
            shared.send(Event::Step(text));
        };
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
    step: String,
    last: Option<ResultView>,
    toast: Option<Toast>,

    trigger_kind: TriggerKind,
    sched_hour: u8,
    sched_minute: u8,
    sched_day: usize,
    sched_month_day: u8,
    sched_busy: bool,
    sched_msg: Option<(bool, String)>,

    #[cfg(debug_assertions)]
    snap: Option<snapshot::Snap>,
}

#[cfg(debug_assertions)]
const PAGES: [(Page, &str); 6] = [
    (Page::Dashboard, "dashboard"),
    (Page::Settings, "settings"),
    (Page::Schedule, "schedule"),
    (Page::About, "about"),
    (Page::Dashboard, "dashboard_result"),
    // Last: it leaves `running` set without a worker behind it.
    (Page::Dashboard, "dashboard_running"),
];

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
            step: String::new(),
            last: None,
            toast: None,
            trigger_kind: TriggerKind::from_index(form.kind),
            sched_hour: form.hour,
            sched_minute: form.minute,
            sched_day: form.weekday as usize,
            sched_month_day: form.month_day,
            sched_busy: false,
            sched_msg: None,
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
            self.notify("Couldn't save your settings", &e.to_string(), Level::Err);
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
            self.notify(
                "Nothing to clean",
                "Turn on at least one cleanup option in Settings.",
                Level::Warn,
            );
            return;
        }
        self.running = true;
        self.stopping = false;
        self.step = "Getting ready…".into();
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
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::Step(s) => self.step = s,
                Event::AutoFired => {
                    self.running = true;
                    self.stopping = false;
                    self.step = "Automatic cleanup…".into();
                }
                Event::Done(None) => {
                    self.running = false;
                    self.stopping = false;
                    self.notify(
                        "Already running",
                        "A cleanup is already in progress.",
                        Level::Warn,
                    );
                }
                Event::Done(Some(r)) => {
                    self.running = false;
                    self.stopping = false;
                    if r.cancelled {
                        let body = if r.freed_kb > 0 {
                            format!("Freed {} before stopping.", format_kb(r.freed_kb))
                        } else {
                            "No extra memory was freed.".to_owned()
                        };
                        self.notify("Cleanup stopped", &body, Level::Warn);
                        continue;
                    }
                    let (title, level) = if r.freed_kb > 0 {
                        (
                            format!("Freed {} of memory", format_kb(r.freed_kb)),
                            Level::Ok,
                        )
                    } else {
                        ("No extra memory was freed".to_owned(), Level::Warn)
                    };
                    if let Some(t) = &self.tray {
                        let _ = t
                            .icon
                            .set_tooltip(Some(format!("OpenMemoryOptimizer — {title} ({})", r.at)));
                    }
                    if self.settings.show_notice {
                        let body = format!(
                            "Available memory: {} → {} (took {:.0} s)",
                            format_kb(r.before_kb),
                            format_kb(r.after_kb),
                            r.elapsed.as_secs_f64()
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
                Event::Schedule(ok, msg) => {
                    self.sched_busy = false;
                    self.sched_msg = Some((ok, msg));
                }
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
                    chip(ui, Some(icons::SHIELD), "Admin mode", GREEN);
                } else {
                    chip(ui, Some(icons::WARNING), "Limited mode", AMBER)
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        for (page, icon, text) in [
            (Page::Dashboard, icons::DASHBOARD, "Overview"),
            (Page::Settings, icons::SETTINGS, "Settings"),
            (Page::Schedule, icons::CLOCK, "Schedule"),
            (Page::About, icons::INFO, "About"),
        ] {
            if nav_item(ui, icon, text, self.page == page).clicked() {
                self.page = page;
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(6.0);
            let s = &self.settings;
            let (txt, color) = if !s.auto_opt {
                ("Auto clean is off".to_owned(), FAINT)
            } else if s.lowmem_opt {
                (
                    format!("Auto clean below {}% free", s.low_mem_percent),
                    GREEN,
                )
            } else {
                (format!("Auto clean every {} min", s.period_minutes), GREEN)
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
            &self.step,
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
        Self::page_header(ui, "Overview", "Your computer's memory, updated live");
        let Some(m) = self.mem else {
            label(ui, "Checking your memory…", regular(14.0), MUTED);
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
                        "in use",
                        regular(13.0),
                        MUTED,
                    );
                    ui.add_space(6.0);
                    let btn = vec2(ui.available_width(), 46.0);
                    if self.running {
                        let text = if self.stopping { "Stopping…" } else { "Stop" };
                        if stop_button(ui, btn, text, Some(icons::STOP), !self.stopping).clicked() {
                            self.stop();
                        }
                        ui.add_space(8.0);
                        self.progress_line(ui);
                    } else if primary_button(ui, btn, "Free up memory", Some(icons::BOLT), true)
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
                let kb = |b: u64| format_kb((b / 1024) as i64);
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
                    ("Healthy", "plenty of free memory")
                } else if used_pct < 85.0 {
                    ("Getting busy", "memory is filling up")
                } else {
                    ("Running low", "try freeing up memory")
                };
                ui.horizontal_top(|ui| {
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        "Total memory",
                        &kb(m.total_phys),
                        "installed in this PC",
                        VIOLET,
                    );
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        "In use",
                        &kb(used),
                        &format!("{}% of total", m.load_percent),
                        lvl,
                    );
                });
                ui.horizontal_top(|ui| {
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        "Available",
                        &kb(m.avail_phys),
                        "ready for your apps",
                        GREEN,
                    );
                    stat_card(ui, vec2(cw, ch), "Status", status, status_sub, lvl);
                });
                ui.horizontal_top(|ui| {
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        "Cached",
                        &cached.map_or("—".into(), kb),
                        if cached.is_some() {
                            "kept ready by Windows"
                        } else {
                            "not available on this PC"
                        },
                        AMBER,
                    );
                    stat_card(
                        ui,
                        vec2(cw, ch),
                        "Processor",
                        &format!("{}%", self.cpu_pct),
                        if self.cpu_pct <= 5 {
                            "resting"
                        } else {
                            "working"
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
                    card_title(ui, "Memory use", Some("last 2 minutes"));
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        label(ui, &format!("{used_pct:.1}%"), semibold(15.0), CYAN);
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
                    Line::new("In use", PlotPoints::from(pts))
                        .color(CYAN)
                        .width(2.0)
                        .fill(0.0)
                        .fill_alpha(0.12),
                );
            });
    }

    fn last_result(&self, ui: &mut Ui) {
        card_title(ui, "Last cleanup", None);
        let Some(r) = &self.last else {
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new("No cleanup yet. Click “Free up memory” to start.")
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
            &format!("{sign}{}", format_kb(r.freed_kb)),
            bold(26.0),
            color,
        );
        ui.add_space(-8.0);
        let mode = if r.mode == Mode::Auto {
            "automatic"
        } else {
            "manual"
        };
        label(
            ui,
            &format!("{} · {mode} · took {:.0} s", r.at, r.elapsed.as_secs_f64()),
            regular(12.0),
            FAINT,
        );
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                for (name, value, lvl) in &r.steps {
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
                            label(ui, name, semibold(12.5), TEXT);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(value).font(regular(12.0)).color(MUTED),
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
        Self::page_header(ui, "Settings", "Changes are saved automatically");
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 14.0;
            card(ui, |ui| {
                card_title(ui, "What to clean", Some("Used when you click “Free up memory”"));
                ui.add_space(4.0);
                let s = &mut self.settings;
                changed |= setting_toggle(
                    ui,
                    "Free memory from open apps",
                    "Asks your running apps to give back memory they aren't using right now.",
                    &mut s.trim_working_sets,
                );
                ui.separator();
                changed |= setting_toggle(
                    ui,
                    "Clear cached memory",
                    "Removes files Windows keeps in memory for quick access. Needs administrator rights.",
                    &mut s.clear_standby,
                );
                ui.separator();
                changed |= setting_toggle(
                    ui,
                    "Deep clean",
                    "Briefly uses up to 1 GB of memory to push unused data out. Can take a few minutes.",
                    &mut s.defrag_mem,
                );
                ui.separator();
                changed |= setting_toggle(
                    ui,
                    "Clear clipboard",
                    "Empties whatever you last copied.",
                    &mut s.clear_clipboard,
                );
            });

            card(ui, |ui| {
                card_title(
                    ui,
                    "Automatic cleanup",
                    Some("A light cleanup that runs in the background"),
                );
                ui.add_space(4.0);
                let s = &mut self.settings;
                changed |= setting_toggle(
                    ui,
                    "Clean automatically",
                    "Keeps working while the app is in the system tray.",
                    &mut s.auto_opt,
                );
                ui.add_enabled_ui(s.auto_opt, |ui| {
                    ui.add_space(4.0);
                    let mut kind = if s.lowmem_opt { AutoKind::LowMemory } else { AutoKind::Periodic };
                    if segmented(
                        ui,
                        &mut kind,
                        &[(AutoKind::LowMemory, "When memory is low"), (AutoKind::Periodic, "On a timer")],
                    ) {
                        s.lowmem_opt = kind == AutoKind::LowMemory;
                        s.period_opt = kind == AutoKind::Periodic;
                        changed = true;
                    }
                    ui.add_space(4.0);
                    if kind == AutoKind::LowMemory {
                        label(ui, "Clean when free memory drops below", regular(13.0), MUTED);
                        let mut pct = s.low_mem_percent as i32;
                        if ui
                            .add(egui::Slider::new(&mut pct, 5..=95).suffix(" %"))
                            .changed()
                        {
                            s.low_mem_percent = pct as i64;
                            changed = true;
                        }
                        changed |= setting_toggle(
                            ui,
                            "Only when the computer is resting",
                            "Waits until your computer isn't busy. Checked every 5 minutes.",
                            &mut s.idle_only,
                        );
                    } else {
                        ui.horizontal(|ui| {
                            label(ui, "Clean every", regular(13.0), MUTED);
                            let mut min = s.period_minutes as i32;
                            if ui
                                .add(egui::DragValue::new(&mut min).range(1..=1440).suffix(" min"))
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
                card_title(ui, "General", None);
                ui.add_space(4.0);
                let s = &mut self.settings;
                changed |= setting_toggle(
                    ui,
                    "Show a notification",
                    "Tells you how much memory was freed after each cleanup.",
                    &mut s.show_notice,
                );
                ui.separator();
                changed |= setting_toggle(
                    ui,
                    "Keep in the system tray",
                    "Minimizing or closing the window hides it next to the clock. Click the tray icon to bring it back, or right-click it to exit.",
                    &mut s.min_to_tray,
                );
            });
            label(ui, &self.config_path.display().to_string(), regular(11.5), FAINT);
        });
        if changed {
            self.settings_changed();
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
        ok_msg: &'static str,
        err_msg: &'static str,
    ) {
        self.sched_busy = true;
        self.sched_msg = None;
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let result = cmd.creation_flags(CREATE_NO_WINDOW).output();
            let (ok, msg) = match result {
                Ok(o) if o.status.success() => (true, ok_msg.to_owned()),
                _ => (false, err_msg.to_owned()),
            };
            shared.send(Event::Schedule(ok, msg));
        });
    }

    fn schedule_page(&mut self, ui: &mut Ui) {
        Self::page_header(
            ui,
            "Schedule",
            "Clean up automatically at set times, even when the app is closed",
        );
        card(ui, |ui| {
            card_title(ui, "When to clean", None);
            ui.add_space(4.0);
            segmented(
                ui,
                &mut self.trigger_kind,
                &[
                    (TriggerKind::Daily, "Daily"),
                    (TriggerKind::Weekly, "Weekly"),
                    (TriggerKind::Monthly, "Monthly"),
                    (TriggerKind::Idle, "When idle"),
                    (TriggerKind::Logon, "At sign-in"),
                ],
            );
            ui.add_space(6.0);
            match self.trigger_kind {
                TriggerKind::Daily => {
                    ui.horizontal(|ui| {
                        label(ui, "Time", regular(13.0), MUTED);
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
                    segmented(
                        ui,
                        &mut day,
                        &[
                            (0, "Mon"),
                            (1, "Tue"),
                            (2, "Wed"),
                            (3, "Thu"),
                            (4, "Fri"),
                            (5, "Sat"),
                            (6, "Sun"),
                        ],
                    );
                    self.sched_day = day;
                }
                TriggerKind::Monthly => {
                    ui.horizontal(|ui| {
                        label(ui, "Day of the month", regular(13.0), MUTED);
                        ui.add(egui::DragValue::new(&mut self.sched_month_day).range(1..=31));
                    });
                }
                TriggerKind::Idle => {
                    label(
                        ui,
                        "Runs after your computer has been unused for 10 minutes.",
                        regular(13.0),
                        MUTED,
                    );
                }
                TriggerKind::Logon => {
                    label(
                        ui,
                        "Runs every time you sign in to Windows.",
                        regular(13.0),
                        MUTED,
                    );
                }
            }
            ui.add_space(8.0);
            let exe = std::env::current_exe().unwrap_or_default();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let enabled = !self.sched_busy;
                if primary_button(
                    ui,
                    vec2(170.0, 40.0),
                    "Create schedule",
                    Some(icons::ADD),
                    enabled,
                )
                .clicked()
                {
                    let cmd = schedule::create_command(&exe, &self.current_trigger());
                    self.run_schtasks(
                        cmd,
                        "Schedule created.",
                        "Couldn't create the schedule. Try running the app as administrator.",
                    );
                }
                if ghost_button(ui, "Remove schedule", Some(icons::DELETE)).clicked() && enabled {
                    self.run_schtasks(
                        schedule::delete_command(),
                        "Schedule removed.",
                        "There's no schedule to remove.",
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
            if let Some((ok, msg)) = &self.sched_msg {
                ui.add_space(4.0);
                let (icon, color) = if *ok {
                    (icons::CHECK, GREEN)
                } else {
                    (icons::WARNING, RED)
                };
                ui.horizontal(|ui| {
                    label(ui, icon, theme::icon(13.0), color);
                    ui.add(
                        egui::Label::new(egui::RichText::new(msg).font(regular(13.0)).color(color))
                            .wrap(),
                    );
                });
            }
        });
    }

    // ----------------------------------------------------------------- about

    fn about_page(&mut self, ui: &mut Ui) {
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
                    concat!(
                        "Version ",
                        env!("CARGO_PKG_VERSION"),
                        " · a simple memory optimizer for Windows"
                    ),
                );
            });
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                card_title(ui, "What happens during a cleanup", None);
                ui.add_space(2.0);
                for (n, title, body) in [
                    ("1", "Clipboard", "Empties whatever you last copied (if turned on)."),
                    ("2", "Deep clean", "Briefly uses spare memory to push unused data out (if turned on)."),
                    ("3", "Open apps", "Asks every running app to give back memory it isn't using."),
                    ("4", "Cached memory", "Removes files Windows kept in memory for quick access (if turned on)."),
                    ("5", "System cache", "Trims the memory Windows uses to speed up opening files."),
                    ("6", "Result", "Waits a few seconds, then shows how much memory became available."),
                ] {
                    ui.horizontal_top(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 12.0, ACCENT.gamma_multiply(0.25));
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, n, semibold(12.0), ACCENT_HI);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            label(ui, title, semibold(13.5), TEXT);
                            ui.add(egui::Label::new(egui::RichText::new(body).font(regular(12.5)).color(MUTED)).wrap());
                        });
                    });
                }
            });
            ui.add_space(4.0);
            card(ui, |ui| {
                card_title(ui, "Good to know", None);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "OpenMemoryOptimizer always shows the real amount of memory freed. Freed memory fills up again as you \
                             keep using your apps, and that's normal. Windows already manages memory well on its own, \
                             so a cleanup helps most when your computer is running low on memory.",
                        )
                        .font(regular(13.0))
                        .color(MUTED),
                    )
                    .wrap(),
                );
            });
            ui.add_space(4.0);
            card(ui, |ui| {
                card_title(ui, "License", None);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Copyright © 2026 Lucian Caetano. OpenMemoryOptimizer is free software: you can \
                             share and change it under the GNU General Public License, version 3 or later. \
                             It comes with no warranty.",
                        )
                        .font(regular(13.0))
                        .color(MUTED),
                    )
                    .wrap(),
                );
                ui.hyperlink_to(
                    egui::RichText::new("Read the GNU GPL v3").font(regular(13.0)),
                    "https://www.gnu.org/licenses/gpl-3.0.html",
                );
            });
        });
    }

    /// Visual-QA fixture for the snapshot mode only (debug builds, MEMOPT_SNAPSHOT set);
    /// avoids running a real optimization just to render the result card.
    #[cfg(debug_assertions)]
    fn load_snapshot_fixture(&mut self) {
        let gb = 1024 * 1024;
        self.last = Some(ResultView {
            mode: Mode::Manual,
            freed_kb: 1_288_490,
            before_kb: 15 * gb,
            after_kb: 15 * gb + 1_288_490,
            elapsed: Duration::from_millis(3240),
            steps: vec![
                ("Clipboard", "Cleared".into(), Level::Ok),
                ("Open apps", "189 apps slimmed down".into(), Level::Ok),
                ("Cached memory", "Cleared".into(), Level::Ok),
                ("System cache", "Cleared".into(), Level::Ok),
            ],
            at: chrono_lite::Clock::now(),
            cancelled: false,
        });
        self.notify(
            "Freed 1.23 GB of memory",
            "Available memory: 15.00 GB → 16.23 GB (took 3 s)",
            Level::Ok,
        );
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
                    if PAGES[i].1 == "dashboard_running" && !self.running {
                        // The longest step text, to check that it stays inside the card.
                        self.toast = None;
                        self.running = true;
                        self.step = "Deep cleaning… this can take a while".into();
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
