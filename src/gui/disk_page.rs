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

//! Disk page: pick a drive, then analyze it (read only), clean up junk or optimize it.
//! Every job runs on a worker thread and reports through `Event::Disk`. The drive
//! cannot be changed while a job runs.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui;
use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, Ui};

use open_memory_optimizer::disk::clean::{self, Category, Source, Status};
use open_memory_optimizer::disk::optimize::{self, Action, Outcome as OptOutcome};
use open_memory_optimizer::disk::scan::{self, BigFile, FileKind, Report, Tree};
use open_memory_optimizer::disk::volume::{self, Kind, Media, Volume};
use open_memory_optimizer::disk::{self, reveal};
use open_memory_optimizer::i18n::{tr, Strings};

use super::{App, Event, Level, Shared};
use crate::theme::{self, icons, *};
use crate::widgets::*;

/// Rows shown under one folder before "…and N smaller folders".
const MAX_CHILDREN: usize = 100;
const FOLDER_ROW: f32 = 30.0;
const FILE_ROW: f32 = 40.0;

pub enum DiskMsg {
    ScanProgress(scan::Progress),
    /// `None`: the scan failed unexpectedly.
    ScanDone(Option<Box<Report>>),
    Measured(Category, Status),
    MeasureDone,
    Cleaned(Category, Status),
    /// Free space on the drive afterwards.
    CleanDone(Option<u64>),
    OptProgress(f32),
    OptDone(Action, OptOutcome),
    /// Drive, TRIM on, size of the hibernate file.
    OptInfo(char, Option<bool>, Option<u64>),
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Analyze,
    Clean,
    Optimize,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Folders,
    Files,
    Types,
}

#[derive(Default)]
pub struct DiskPage {
    volumes: Vec<Volume>,
    listed: bool,
    letter: Option<char>,
    tab: Tab,

    scanning: bool,
    scan_cancel: Option<Arc<AtomicBool>>,
    progress: Option<scan::Progress>,
    report: Option<Arc<Report>>,
    expanded: HashSet<u32>,
    view: View,

    measuring: bool,
    cleaning: bool,
    /// Drive whose junk was measured; measuring starts again when it differs.
    measured_for: Option<char>,
    clean_cancel: Option<Arc<AtomicBool>>,
    found: [Option<Status>; clean::COUNT],
    removed: [Option<Status>; clean::COUNT],
    free_before: u64,

    opt_busy: Option<Action>,
    opt_pct: Option<f32>,
    opt_cancel: Option<Arc<AtomicBool>>,
    opt_results: Vec<(Action, OptOutcome)>,
    /// Drive whose TRIM and hibernate state were read.
    info_for: Option<char>,
    info_ready: bool,
    trim: Option<bool>,
    hiber: Option<u64>,
}

impl DiskPage {
    fn busy(&self) -> bool {
        self.scanning || self.measuring || self.cleaning || self.opt_busy.is_some()
    }

    fn volume(&self) -> Option<&Volume> {
        let l = self.letter?;
        self.volumes.iter().find(|v| v.letter == l)
    }
}

impl Drop for DiskPage {
    fn drop(&mut self) {
        for c in [&self.scan_cancel, &self.clean_cancel, &self.opt_cancel]
            .into_iter()
            .flatten()
        {
            c.store(true, Ordering::Relaxed);
        }
    }
}

fn bytes(t: &Strings, b: u64) -> String {
    t.size(disk::kb(b))
}

fn drive_name(v: &Volume, t: &Strings) -> String {
    let label = if v.label.is_empty() {
        t.disk_local
    } else {
        &v.label
    };
    format!("{label} ({}:)", v.letter)
}

fn media_text(v: &Volume, t: &'static Strings) -> &'static str {
    match (v.kind, v.media) {
        (Kind::Network, _) => t.media_network,
        (Kind::Removable, _) => t.media_removable,
        (_, Media::Ssd) => t.media_ssd,
        (_, Media::Hdd) => t.media_hdd,
        _ => "",
    }
}

fn wrapped(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).wrap());
}

/// Horizontal bar: `frac` of `rect` filled.
fn bar(p: &egui::Painter, rect: Rect, frac: f32, color: Color32) {
    let r = CornerRadius::same((rect.height() / 2.0) as u8);
    p.rect_filled(rect, r, TRACK);
    let frac = frac.clamp(0.0, 1.0);
    if frac > 0.0 {
        let w = (rect.width() * frac).max(rect.height());
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(w, rect.height())),
            r,
            color,
        );
    }
}

/// Small square icon button painted in `rect`; true when clicked.
fn row_button(ui: &mut Ui, rect: Rect, id: egui::Id, icon: &str, tip: &str) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered(), 0.1);
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius::same(7),
        lerp_color(Color32::TRANSPARENT, ACCENT.gamma_multiply(0.3), hover),
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        theme::icon(13.0),
        lerp_color(MUTED, Color32::WHITE, hover),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
        .clicked()
}

fn drive_tile(ui: &mut Ui, v: &Volume, selected: bool, enabled: bool) -> egui::Response {
    let t = tr();
    let size = vec2(214.0, 80.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), enabled && resp.hovered(), 0.1);
    let sel = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("s"), selected, 0.15);
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same(12),
        lerp_color(
            lerp_color(CARD, CARD_HI, hover),
            ACCENT.gamma_multiply(0.2),
            sel,
        ),
        Stroke::new(1.0, lerp_color(STROKE, ACCENT, sel)),
        egui::StrokeKind::Inside,
    );
    let icon = match v.kind {
        Kind::Removable => icons::USB,
        Kind::Network => icons::NETWORK_DRIVE,
        _ => icons::DRIVE,
    };
    p.text(
        rect.left_top() + vec2(16.0, 24.0),
        Align2::LEFT_CENTER,
        icon,
        theme::icon(18.0),
        lerp_color(MUTED, CYAN, sel),
    );
    let text_x = rect.left() + 46.0;
    let name = fitted_text(
        p,
        &drive_name(v, t),
        semibold(13.5),
        TEXT,
        rect.right() - text_x - 10.0,
        1,
    );
    p.galley(pos2(text_x, rect.top() + 9.0), name, TEXT);
    let media = media_text(v, t);
    let sub = match (media.is_empty(), v.fs.is_empty()) {
        (false, false) => format!("{media} · {}", v.fs),
        (true, false) => v.fs.clone(),
        _ => media.to_owned(),
    };
    p.text(
        pos2(text_x, rect.top() + 36.0),
        Align2::LEFT_CENTER,
        sub,
        regular(11.5),
        FAINT,
    );
    let used = disk::percent(v.used(), v.total);
    bar(
        p,
        Rect::from_min_size(
            pos2(rect.left() + 16.0, rect.top() + 50.0),
            vec2(rect.width() - 32.0, 6.0),
        ),
        used / 100.0,
        level_color(used),
    );
    let free = (t.disk_free_of)(&bytes(t, v.free), &bytes(t, v.total));
    let free = fitted_text(p, &free, regular(11.5), MUTED, rect.width() - 32.0, 1);
    p.galley(pos2(rect.left() + 16.0, rect.top() + 60.0), free, MUTED);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

enum Row {
    Folder(u32, usize),
    More(usize, usize),
}

fn folder_rows(tree: &Tree, at: u32, depth: usize, open: &HashSet<u32>, rows: &mut Vec<Row>) {
    let kids = tree.children(at);
    for &c in kids.iter().take(MAX_CHILDREN) {
        rows.push(Row::Folder(c, depth));
        if open.contains(&c) {
            folder_rows(tree, c, depth + 1, open, rows);
        }
    }
    if kids.len() > MAX_CHILDREN {
        rows.push(Row::More(depth, kids.len() - MAX_CHILDREN));
    }
}

fn kind_color(k: FileKind) -> Color32 {
    match k {
        FileKind::Videos => VIOLET,
        FileKind::Music => Color32::from_rgb(244, 114, 182),
        FileKind::Pictures => CYAN,
        FileKind::Documents => GREEN,
        FileKind::Archives => AMBER,
        FileKind::Programs => ACCENT_HI,
        FileKind::Other => MUTED,
    }
}

fn opt_outcome(o: OptOutcome, t: &'static Strings) -> (&'static str, Color32) {
    match o {
        OptOutcome::Done => (t.opt_done, GREEN),
        OptOutcome::Failed => (t.opt_failed, RED),
        OptOutcome::Cancelled => (t.opt_stopped, AMBER),
        OptOutcome::NeedsAdmin => (t.outcome_needs_admin, AMBER),
    }
}

fn action_title(a: Action, t: &'static Strings) -> &'static str {
    match a {
        Action::OptimizeDrive => t.opt_drive_title,
        Action::EnableTrim => t.opt_trim_title,
        Action::HibernateSmaller | Action::HibernateOff => t.opt_hiber_title,
        Action::Components => t.opt_components_title,
    }
}

/// Runs `job` on a worker thread; `done` is sent even if `job` panics, so the
/// page never stays busy.
fn spawn_job(
    shared: &Arc<Shared>,
    job: impl FnOnce(&Shared) + Send + 'static,
    done: impl FnOnce() -> DiskMsg + Send + 'static,
) {
    let shared = shared.clone();
    std::thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| job(&shared)));
        shared.send(Event::Disk(done()));
    });
}

impl App {
    fn disk_refresh(&mut self) {
        let d = &mut self.disk;
        d.volumes = volume::list();
        d.listed = true;
        let has = |l: &char| d.volumes.iter().any(|v| v.letter == *l);
        let pick = d
            .letter
            .filter(has)
            .or(self.settings.disk_drive.filter(has))
            .or_else(|| d.volumes.iter().find(|v| v.system).map(|v| v.letter))
            .or_else(|| d.volumes.first().map(|v| v.letter));
        if pick != self.disk.letter {
            self.disk_select(pick);
        }
    }

    fn disk_select(&mut self, letter: Option<char>) {
        let d = &mut self.disk;
        if d.busy() || letter == d.letter {
            return;
        }
        d.letter = letter;
        d.report = None;
        d.progress = None;
        d.expanded.clear();
        d.measured_for = None;
        d.found = Default::default();
        d.removed = Default::default();
        d.info_for = None;
        d.info_ready = false;
        d.opt_results.clear();
        if letter.is_some() && letter != self.settings.disk_drive {
            self.settings.disk_drive = letter;
            self.settings_changed();
        }
    }

    fn disk_scan(&mut self) {
        let Some(v) = self.disk.volume().cloned() else {
            return;
        };
        if self.disk.busy() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let d = &mut self.disk;
        d.scan_cancel = Some(cancel.clone());
        d.scanning = true;
        d.progress = Some(scan::Progress::default());
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let report = catch_unwind(AssertUnwindSafe(|| {
                scan::run(&v.root(), &cancel, &mut |p| {
                    shared.send(Event::Disk(DiskMsg::ScanProgress(p.clone())))
                })
            }));
            shared.send(Event::Disk(DiskMsg::ScanDone(report.ok().map(Box::new))));
        });
    }

    fn disk_measure(&mut self) {
        let Some(v) = self.disk.volume().cloned() else {
            return;
        };
        if self.disk.busy() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let d = &mut self.disk;
        d.measuring = true;
        d.measured_for = Some(v.letter);
        d.found = Default::default();
        d.removed = Default::default();
        d.clean_cancel = Some(cancel.clone());
        // The Windows handlers can be slow: show our own folders first.
        let mut cats: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|c| c.applies(&v))
            .collect();
        cats.sort_by_key(|c| matches!(c.source(), Source::Handler(_)));
        spawn_job(
            &self.shared,
            move |s| {
                for c in cats {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let status = clean::measure(c, &v, &cancel);
                    s.send(Event::Disk(DiskMsg::Measured(c, status)));
                }
            },
            || DiskMsg::MeasureDone,
        );
    }

    /// Categories that the cleanup would touch: chosen, shown for this drive, and with
    /// something found.
    fn disk_chosen(&self) -> Vec<Category> {
        let Some(v) = self.disk.volume() else {
            return Vec::new();
        };
        Category::ALL
            .into_iter()
            .filter(|c| c.applies(v) && self.settings.disk_clean[c.index()])
            .filter(|c| self.disk.removed[c.index()].is_none())
            .filter(|c| self.disk.found[c.index()].is_some_and(|s| s.bytes() > 0))
            .collect()
    }

    fn disk_clean(&mut self) {
        let Some(v) = self.disk.volume().cloned() else {
            return;
        };
        let cats = self.disk_chosen();
        if self.disk.busy() || cats.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let d = &mut self.disk;
        d.cleaning = true;
        d.free_before = volume::free_space(v.letter).unwrap_or(v.free);
        d.clean_cancel = Some(cancel.clone());
        let letter = v.letter;
        spawn_job(
            &self.shared,
            move |s| {
                for c in cats {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let status = clean::clean(c, &v, &cancel);
                    s.send(Event::Disk(DiskMsg::Cleaned(c, status)));
                }
            },
            move || DiskMsg::CleanDone(volume::free_space(letter)),
        );
    }

    fn disk_optimize(&mut self, action: Action) {
        let Some(letter) = self.disk.letter else {
            return;
        };
        if self.disk.busy() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let d = &mut self.disk;
        d.opt_busy = Some(action);
        d.opt_pct = None;
        d.opt_cancel = Some(cancel.clone());
        d.opt_results.retain(|(a, _)| *a != action);
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                optimize::run(action, letter, &cancel, &mut |p| {
                    shared.send(Event::Disk(DiskMsg::OptProgress(p)))
                })
            }))
            .unwrap_or(OptOutcome::Failed);
            shared.send(Event::Disk(DiskMsg::OptDone(action, outcome)));
        });
    }

    fn disk_load_info(&mut self) {
        let Some(v) = self.disk.volume().cloned() else {
            return;
        };
        self.disk.info_for = Some(v.letter);
        self.disk.info_ready = false;
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let trim = optimize::trim_enabled();
            let hiber = if v.system {
                optimize::hibernation_file(v.letter)
            } else {
                None
            };
            shared.send(Event::Disk(DiskMsg::OptInfo(v.letter, trim, hiber)));
        });
    }

    pub(crate) fn disk_event(&mut self, m: DiskMsg) {
        let t = tr();
        let d = &mut self.disk;
        match m {
            DiskMsg::ScanProgress(p) => {
                if d.scanning {
                    d.progress = Some(p);
                }
            }
            DiskMsg::ScanDone(r) => {
                d.scanning = false;
                d.progress = None;
                d.scan_cancel = None;
                d.expanded.clear();
                d.report = r.map(Arc::from);
                if let Some(r) = &d.report {
                    // Open the biggest folder one level, like a first click would.
                    if let Some(&first) = r.tree.children(0).first() {
                        d.expanded.insert(first);
                    }
                }
            }
            DiskMsg::Measured(c, s) => d.found[c.index()] = Some(s),
            DiskMsg::MeasureDone => {
                d.measuring = false;
                d.clean_cancel = None;
            }
            DiskMsg::Cleaned(c, s) => d.removed[c.index()] = Some(s),
            DiskMsg::CleanDone(after) => {
                d.cleaning = false;
                let stopped = d
                    .clean_cancel
                    .take()
                    .is_some_and(|c| c.load(Ordering::Relaxed));
                let freed: u64 = d.removed.iter().flatten().map(Status::bytes).sum();
                let before = d.free_before;
                let after = after.unwrap_or(before + freed);
                self.disk_refresh();
                if stopped {
                    let body = if freed > 0 {
                        (t.clean_done)(&bytes(t, freed))
                    } else {
                        t.clean_nothing_body.to_owned()
                    };
                    self.notify(t.clean_stopped, &body, Level::Warn);
                } else if freed > 0 {
                    let body = (t.clean_done_body)(&bytes(t, before), &bytes(t, after));
                    self.notify(&(t.clean_done)(&bytes(t, freed)), &body, Level::Ok);
                } else {
                    self.notify(t.clean_nothing, t.clean_nothing_body, Level::Warn);
                }
            }
            DiskMsg::OptProgress(p) => d.opt_pct = Some(p),
            DiskMsg::OptDone(action, outcome) => {
                d.opt_busy = None;
                d.opt_cancel = None;
                d.opt_pct = None;
                d.opt_results.retain(|(a, _)| *a != action);
                d.opt_results.push((action, outcome));
                if matches!(
                    action,
                    Action::EnableTrim | Action::HibernateSmaller | Action::HibernateOff
                ) {
                    d.info_for = None;
                }
                let (text, _) = opt_outcome(outcome, t);
                let level = match outcome {
                    OptOutcome::Done => Level::Ok,
                    OptOutcome::Failed => Level::Err,
                    _ => Level::Warn,
                };
                self.disk_refresh();
                self.notify(action_title(action, t), text, level);
            }
            DiskMsg::OptInfo(letter, trim, hiber) => {
                if d.letter == Some(letter) {
                    d.trim = trim;
                    d.hiber = hiber;
                    d.info_ready = true;
                }
            }
        }
    }

    // ------------------------------------------------------------------ page

    pub(crate) fn disk_page(&mut self, ui: &mut Ui) {
        let t = tr();
        Self::page_header(ui, t.nav_disk, t.disk_sub);
        if !self.disk.listed {
            self.disk_refresh();
        }
        let gap = 12.0;
        ui.spacing_mut().item_spacing.y = gap;
        self.drive_row(ui);

        let mut refresh = false;
        ui.horizontal(|ui| {
            segmented(
                ui,
                &mut self.disk.tab,
                &[
                    (Tab::Analyze, t.tab_analyze),
                    (Tab::Clean, t.tab_clean),
                    (Tab::Optimize, t.tab_optimize),
                ],
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = !self.disk.busy();
                ui.add_enabled_ui(enabled, |ui| {
                    refresh = ghost_button(ui, t.disk_refresh, Some(icons::REFRESH)).clicked();
                });
            });
        });
        if refresh {
            self.disk.listed = false;
            self.disk_refresh();
        }

        let Some(v) = self.disk.volume().cloned() else {
            return;
        };
        match self.disk.tab {
            Tab::Clean
                if v.can_clean()
                    && self.disk.measured_for != Some(v.letter)
                    && !self.disk.busy() =>
            {
                self.disk_measure()
            }
            Tab::Optimize if v.can_optimize() && self.disk.info_for != Some(v.letter) => {
                self.disk_load_info()
            }
            _ => {}
        }
        let h = (ui.available_height() - 1.0).max(200.0);
        let full_w = ui.available_width();
        let side_w = 240.0;
        match self.disk.tab {
            Tab::Analyze => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = vec2(gap, gap);
                    card_sized(ui, vec2(full_w - side_w - gap, h), |ui| {
                        self.analyze_list(ui)
                    });
                    card_sized(ui, vec2(side_w, h), |ui| self.analyze_side(ui, &v));
                });
            }
            Tab::Clean if !v.can_clean() => {
                card(ui, |ui| {
                    label(ui, t.opt_not_supported, regular(13.5), MUTED)
                });
            }
            Tab::Clean => {
                self.clean_summary(ui);
                let h = (ui.available_height() - 1.0).max(150.0);
                card_sized(ui, vec2(full_w, h), |ui| self.clean_list(ui, &v));
            }
            Tab::Optimize if !v.can_optimize() => {
                card(ui, |ui| {
                    label(ui, t.opt_not_supported, regular(13.5), MUTED)
                });
            }
            Tab::Optimize => self.optimize_tab(ui, &v),
        }
    }

    fn drive_row(&mut self, ui: &mut Ui) {
        let enabled = !self.disk.busy();
        let mut pick = None;
        egui::ScrollArea::horizontal()
            .id_salt("drives")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for v in &self.disk.volumes {
                        let selected = self.disk.letter == Some(v.letter);
                        if drive_tile(ui, v, selected, enabled || selected).clicked() {
                            pick = Some(v.letter);
                        }
                    }
                });
            });
        if let Some(l) = pick {
            self.disk_select(Some(l));
        }
    }

    // --------------------------------------------------------------- analyze

    fn analyze_side(&mut self, ui: &mut Ui, v: &Volume) {
        let t = tr();
        let mut start = false;
        let mut stop = false;
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(150.0, 150.0), Sense::hover());
            let pct = disk::percent(v.used(), v.total);
            ring(ui, r.center(), 62.0, 12.0, pct / 100.0, CYAN, VIOLET);
            let p = ui.painter();
            p.text(
                r.center() - vec2(0.0, 8.0),
                Align2::CENTER_CENTER,
                format!("{pct:.0}%"),
                bold(32.0),
                TEXT,
            );
            p.text(
                r.center() + vec2(0.0, 20.0),
                Align2::CENTER_CENTER,
                t.analyze_used,
                regular(12.5),
                MUTED,
            );
            label(
                ui,
                &(t.disk_free_of)(&bytes(t, v.free), &bytes(t, v.total)),
                regular(12.5),
                MUTED,
            );
            ui.add_space(4.0);
            let btn = vec2(ui.available_width(), 44.0);
            if self.disk.scanning {
                let stopping = self
                    .disk
                    .scan_cancel
                    .as_ref()
                    .is_some_and(|c| c.load(Ordering::Relaxed));
                let text = if stopping { t.stopping } else { t.stop };
                stop = stop_button(ui, btn, text, Some(icons::STOP), !stopping).clicked();
                if let Some(p) = &self.disk.progress {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(13.0).color(CYAN));
                        let line = (t.analyze_progress)(&(t.count)(p.files), &bytes(t, p.bytes));
                        label(ui, &line, regular(12.5), TEXT);
                    });
                    let w = ui.available_width();
                    let g = fitted_text(ui.painter(), &p.current, regular(11.5), FAINT, w, 2);
                    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
                    ui.painter()
                        .galley(pos2(r.center().x - g.size().x / 2.0, r.top()), g, FAINT);
                }
            } else {
                let text = if self.disk.report.is_some() {
                    t.analyze_again
                } else {
                    t.analyze_button
                };
                start = primary_button(ui, btn, text, Some(icons::SEARCH), true).clicked();
            }
        });
        if let Some(r) = &self.disk.report {
            ui.add_space(2.0);
            wrapped(
                ui,
                &(t.analyze_summary)(
                    &(t.count)(r.files),
                    &(t.count)(r.folders),
                    r.elapsed.as_secs_f64(),
                ),
                regular(12.0),
                MUTED,
            );
            if r.cancelled {
                wrapped(ui, t.analyze_stopped, regular(12.0), AMBER);
            } else {
                let unread = v.used().saturating_sub(r.total());
                if unread > v.used() / 100 {
                    wrapped(
                        ui,
                        &(t.analyze_unread)(&bytes(t, unread)),
                        regular(12.0),
                        FAINT,
                    );
                }
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                label(ui, icons::SHIELD, theme::icon(13.0), GREEN);
                wrapped(ui, t.analyze_read_only, regular(12.0), GREEN);
            });
        });
        if stop {
            if let Some(c) = &self.disk.scan_cancel {
                c.store(true, Ordering::Relaxed);
            }
        }
        if start {
            self.disk_scan();
        }
    }

    fn analyze_list(&mut self, ui: &mut Ui) {
        let t = tr();
        let Some(report) = self.disk.report.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 2.0 - 90.0).max(10.0));
                let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                ui.painter()
                    .circle_filled(r.center(), 28.0, ACCENT.gamma_multiply(0.2));
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    icons::SEARCH,
                    theme::icon(22.0),
                    ACCENT_HI,
                );
                ui.add_space(4.0);
                label(ui, t.analyze_empty_title, semibold(16.0), TEXT);
                ui.set_max_width(440.0);
                if self.disk.scanning {
                    label(ui, t.analyze_reading, regular(13.0), MUTED);
                } else {
                    wrapped(ui, t.analyze_empty_body, regular(13.0), MUTED);
                }
            });
            return;
        };
        segmented(
            ui,
            &mut self.disk.view,
            &[
                (View::Folders, t.view_folders),
                (View::Files, t.view_files),
                (View::Types, t.view_types),
            ],
        );
        ui.add_space(-4.0);
        match self.disk.view {
            View::Folders => self.folder_list(ui, &report),
            View::Files => file_list(ui, &report.top_files),
            View::Types => type_list(ui, &report),
        }
    }

    fn folder_list(&mut self, ui: &mut Ui, report: &Report) {
        let t = tr();
        let tree = &report.tree;
        let mut rows = Vec::new();
        folder_rows(tree, 0, 0, &self.disk.expanded, &mut rows);
        let mut toggle = None;
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("disk_folders")
            .auto_shrink([false, false])
            .show_rows(ui, FOLDER_ROW, rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let w = ui.available_width();
                for row in &rows[range] {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, FOLDER_ROW), Sense::click());
                    let p = ui.painter();
                    match *row {
                        Row::More(depth, n) => {
                            p.text(
                                pos2(rect.left() + 42.0 + depth as f32 * 16.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                (t.more_items)(n),
                                regular(12.0),
                                FAINT,
                            );
                        }
                        Row::Folder(i, depth) => {
                            let node = tree.node(i);
                            let parent = tree.node(node.parent).size.max(1);
                            let has_kids = !tree.children(i).is_empty();
                            if resp.hovered() {
                                p.rect_filled(rect, CornerRadius::same(7), CARD_HI);
                            }
                            let x0 = rect.left() + 4.0 + depth as f32 * 16.0;
                            let y = rect.center().y;
                            if has_kids {
                                let chevron = if self.disk.expanded.contains(&i) {
                                    icons::CHEVRON_DOWN
                                } else {
                                    icons::CHEVRON_RIGHT
                                };
                                p.text(
                                    pos2(x0 + 6.0, y),
                                    Align2::CENTER_CENTER,
                                    chevron,
                                    theme::icon(9.0),
                                    MUTED,
                                );
                            }
                            p.text(
                                pos2(x0 + 18.0, y),
                                Align2::LEFT_CENTER,
                                icons::FOLDER,
                                theme::icon(13.0),
                                AMBER,
                            );
                            let open_r = Rect::from_center_size(
                                pos2(rect.right() - 16.0, y),
                                vec2(28.0, 24.0),
                            );
                            let size_right = open_r.left() - 8.0;
                            let pct_right = size_right - 78.0;
                            let bar_r = Rect::from_min_size(
                                pos2(pct_right - 50.0 - 60.0, y - 3.0),
                                vec2(60.0, 6.0),
                            );
                            let name_x = x0 + 38.0;
                            let name = fitted_text(
                                p,
                                &node.name,
                                regular(13.0),
                                TEXT,
                                (bar_r.left() - name_x - 10.0).max(20.0),
                                1,
                            );
                            p.galley(pos2(name_x, y - name.size().y / 2.0), name, TEXT);
                            let frac = node.size as f32 / parent as f32;
                            bar(p, bar_r, frac, ACCENT_HI);
                            p.text(
                                pos2(pct_right, y),
                                Align2::RIGHT_CENTER,
                                t.decimal(format!("{:.1}%", frac * 100.0)),
                                regular(12.0),
                                MUTED,
                            );
                            p.text(
                                pos2(size_right, y),
                                Align2::RIGHT_CENTER,
                                bytes(t, node.size),
                                semibold(12.5),
                                TEXT,
                            );
                            let id = ui.scope_id().with(("open_dir", i));
                            if row_button(ui, open_r, id, icons::OPEN_FOLDER, t.open_folder) {
                                open = Some(i);
                            } else if resp.clicked() && has_kids {
                                toggle = Some(i);
                            }
                        }
                    }
                }
            });
        if let Some(i) = toggle {
            if !self.disk.expanded.remove(&i) {
                self.disk.expanded.insert(i);
            }
        }
        if let Some(i) = open {
            reveal::open_folder(&tree.path(i));
        }
    }

    // ----------------------------------------------------------------- clean

    fn clean_list(&mut self, ui: &mut Ui, v: &Volume) {
        let t = tr();
        card_title(ui, t.clean_title, Some(t.clean_sub));
        if !v.system {
            wrapped(
                ui,
                &(t.clean_only_bin)(volume::system_letter()),
                regular(12.5),
                AMBER,
            );
        }
        ui.add_space(2.0);
        let mut changed = false;
        let enabled = !self.disk.cleaning;
        egui::ScrollArea::vertical()
            .id_salt("disk_clean")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for c in Category::ALL.into_iter().filter(|c| c.applies(v)) {
                    let i = c.index();
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        ui.add_enabled_ui(enabled, |ui| {
                            ui.add_space(2.0);
                            changed |= toggle(ui, &mut self.settings.disk_clean[i]).changed();
                        });
                        let text_w = ui.available_width() - 190.0;
                        ui.allocate_ui_with_layout(
                            vec2(text_w, 0.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    label(ui, t.cat_titles[i], semibold(14.0), TEXT);
                                    if c.careful() {
                                        chip(ui, Some(icons::WARNING), t.cat_careful, AMBER);
                                    }
                                });
                                wrapped(ui, t.cat_descs[i], regular(12.5), MUTED);
                                let note = match (self.disk.removed[i], self.disk.found[i]) {
                                    (Some(Status::Done { skipped, .. }), _) if skipped > 0 => {
                                        Some(((t.cat_in_use)(&(t.count)(skipped)), FAINT))
                                    }
                                    (
                                        None,
                                        Some(Status::Done {
                                            blocked: Some(app), ..
                                        }),
                                    ) => Some(((t.cat_blocked)(app), AMBER)),
                                    _ => None,
                                };
                                if let Some((n, color)) = note {
                                    wrapped(ui, &n, regular(12.0), color);
                                }
                            },
                        );
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                            ui.add_space(4.0);
                            self.cat_status(ui, c);
                        });
                    });
                    ui.separator();
                }
            });
        if changed {
            self.settings_changed();
        }
    }

    fn cat_status(&self, ui: &mut Ui, c: Category) {
        let t = tr();
        let i = c.index();
        let d = &self.disk;
        let spinner = |ui: &mut Ui| {
            ui.add(egui::Spinner::new().size(14.0).color(CYAN));
        };
        if let Some(r) = d.removed[i] {
            let (text, color) = match r {
                Status::Done { bytes: b, .. } if b > 0 => ((t.cat_removed)(&bytes(t, b)), GREEN),
                Status::Done { .. } => (t.cat_none.to_owned(), FAINT),
                Status::Cancelled => (t.opt_stopped.to_owned(), AMBER),
                Status::Unavailable => (t.cat_unavailable.to_owned(), FAINT),
                Status::Failed if !self.elevated => (t.outcome_needs_admin.to_owned(), AMBER),
                Status::Failed => (t.cat_failed.to_owned(), RED),
            };
            label(ui, &text, semibold(12.5), color);
            return;
        }
        if d.cleaning && self.settings.disk_clean[i] && d.found[i].is_some_and(|s| s.bytes() > 0) {
            spinner(ui);
            return;
        }
        match d.found[i] {
            None if d.measuring => spinner(ui),
            None | Some(Status::Cancelled) => {
                label(ui, "—", regular(13.0), FAINT);
            }
            Some(Status::Done { bytes: b, .. }) if b > 0 => {
                let color = if self.settings.disk_clean[i] {
                    TEXT
                } else {
                    MUTED
                };
                label(ui, &bytes(t, b), semibold(14.0), color);
            }
            Some(Status::Done { .. }) => {
                label(ui, t.cat_none, regular(12.5), FAINT);
            }
            Some(Status::Unavailable) => {
                label(ui, t.cat_unavailable, regular(12.5), FAINT);
            }
            Some(Status::Failed) => {
                let text = if self.elevated {
                    t.cat_failed
                } else {
                    t.outcome_needs_admin
                };
                label(ui, text, regular(12.5), AMBER);
            }
        }
    }

    /// Bar above the list: what the chosen items free, and the buttons.
    fn clean_summary(&mut self, ui: &mut Ui) {
        let t = tr();
        let chosen = self.disk_chosen();
        let total: u64 = chosen
            .iter()
            .filter_map(|c| self.disk.found[c.index()])
            .map(|s| s.bytes())
            .sum();
        let (mut start, mut stop, mut check) = (false, false, false);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
                ui.painter()
                    .circle_filled(r.center(), 24.0, GREEN.gamma_multiply(0.15));
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    icons::BROOM,
                    theme::icon(20.0),
                    GREEN,
                );
                let btn_text = if total > 0 {
                    (t.clean_button)(&bytes(t, total))
                } else {
                    t.clean_nothing_selected.to_owned()
                };
                let btn_w = pill_width(ui, &btn_text, Some(icons::BROOM)).max(190.0);
                let check_w = pill_width(ui, t.clean_check, Some(icons::REFRESH));
                let text_w = (ui.available_width() - btn_w - check_w - 40.0).max(120.0);
                ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let color = if total > 0 { GREEN } else { MUTED };
                    label(
                        ui,
                        &(t.clean_total)(&bytes(t, total)),
                        semibold(18.0),
                        color,
                    );
                    if chosen.iter().any(|c| c.careful()) {
                        wrapped(ui, t.clean_warning, regular(12.0), AMBER);
                    } else {
                        label(ui, t.clean_total_sub, regular(12.0), FAINT);
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let btn = vec2(btn_w, 44.0);
                    // Measuring can take minutes (Windows Update), so it can be stopped too.
                    if self.disk.cleaning || self.disk.measuring {
                        let stopping = self
                            .disk
                            .clean_cancel
                            .as_ref()
                            .is_some_and(|c| c.load(Ordering::Relaxed));
                        let text = if stopping { t.stopping } else { t.stop };
                        stop = stop_button(ui, btn, text, Some(icons::STOP), !stopping).clicked();
                        let doing = if self.disk.cleaning {
                            t.clean_cleaning
                        } else {
                            t.clean_checking
                        };
                        label(ui, doing, regular(12.5), MUTED);
                        ui.add(egui::Spinner::new().size(13.0).color(CYAN));
                    } else {
                        start = primary_button(ui, btn, &btn_text, Some(icons::BROOM), total > 0)
                            .clicked();
                        check = ghost_button(ui, t.clean_check, Some(icons::REFRESH)).clicked();
                    }
                });
            });
        });
        if stop {
            if let Some(c) = &self.disk.clean_cancel {
                c.store(true, Ordering::Relaxed);
            }
        }
        if start {
            self.disk_clean();
        }
        if check {
            self.disk_measure();
        }
    }

    // -------------------------------------------------------------- optimize

    fn optimize_tab(&mut self, ui: &mut Ui, v: &Volume) {
        let t = tr();
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("disk_optimize")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                label(ui, t.optimize_sub, regular(12.5), MUTED);
                let desc = match v.media {
                    Media::Ssd => t.opt_drive_ssd,
                    Media::Hdd => t.opt_drive_hdd,
                    Media::Unknown => t.opt_drive_unknown,
                };
                clicked = clicked.or(self.opt_card(
                    ui,
                    icons::SPEED,
                    t.opt_drive_title,
                    desc,
                    None,
                    &[(Action::OptimizeDrive, t.opt_drive_button)],
                ));
                if v.media == Media::Ssd {
                    let state = if !self.disk.info_ready {
                        None
                    } else {
                        Some(match self.disk.trim {
                            Some(true) => (t.opt_trim_on, GREEN),
                            Some(false) => (t.opt_trim_off, AMBER),
                            None => (t.opt_trim_unknown, FAINT),
                        })
                    };
                    let buttons: &[(Action, &str)] = if self.disk.trim == Some(false) {
                        &[(Action::EnableTrim, t.opt_trim_button)]
                    } else {
                        &[]
                    };
                    clicked = clicked.or(self.opt_card(
                        ui,
                        icons::CHECK,
                        t.opt_trim_title,
                        "",
                        state,
                        buttons,
                    ));
                }
                if v.system {
                    let (desc, buttons): (String, &[(Action, &str)]) = match self.disk.hiber {
                        Some(size) => (
                            (t.opt_hiber_desc)(&bytes(t, size)),
                            &[
                                // The first is the highlighted one: the milder choice.
                                (Action::HibernateSmaller, t.opt_hiber_smaller),
                                (Action::HibernateOff, t.opt_hiber_disable),
                            ],
                        ),
                        None => (t.opt_hiber_none.to_owned(), &[]),
                    };
                    let note = self.disk.hiber.map(|_| (t.opt_hiber_note, FAINT));
                    if self.disk.info_ready {
                        clicked = clicked.or(self.opt_card(
                            ui,
                            icons::BOLT,
                            t.opt_hiber_title,
                            &desc,
                            note,
                            buttons,
                        ));
                    }
                    clicked = clicked.or(self.opt_card(
                        ui,
                        icons::BROOM,
                        t.opt_components_title,
                        t.opt_components_desc,
                        None,
                        &[(Action::Components, t.opt_components_button)],
                    ));
                }
            });
        if let Some(a) = clicked {
            self.disk_optimize(a);
        }
    }

    /// One optimization card: icon, title, description, an optional status line and
    /// the buttons (the first is the main one). Shows progress and the last result
    /// of its actions. Returns the action clicked.
    fn opt_card(
        &mut self,
        ui: &mut Ui,
        icon: &str,
        title: &str,
        desc: &str,
        state: Option<(&str, Color32)>,
        buttons: &[(Action, &str)],
    ) -> Option<Action> {
        let t = tr();
        let mine = |a: Action| buttons.iter().any(|(b, _)| *b == a);
        let busy_here = self.disk.opt_busy.filter(|a| mine(*a));
        let result = self
            .disk
            .opt_results
            .iter()
            .rev()
            .find(|(a, _)| mine(*a))
            .map(|(_, o)| *o);
        let enabled = !self.disk.busy();
        let mut clicked = None;
        let mut stop = false;
        card(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (r, _) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::hover());
                ui.painter()
                    .circle_filled(r.center(), 19.0, ACCENT.gamma_multiply(0.2));
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    icon,
                    theme::icon(16.0),
                    ACCENT_HI,
                );
                let buttons_w: f32 = buttons
                    .iter()
                    .map(|(_, text)| pill_width(ui, text, None).max(110.0) + 8.0)
                    .sum::<f32>()
                    .max(130.0);
                let text_w = ui.available_width() - buttons_w - 12.0;
                ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    label(ui, title, semibold(14.5), TEXT);
                    if !desc.is_empty() {
                        wrapped(ui, desc, regular(12.5), MUTED);
                    }
                    if let Some((s, color)) = state {
                        wrapped(ui, s, regular(12.5), color);
                    }
                    if let Some(a) = busy_here {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(13.0).color(CYAN));
                            let text = match self.disk.opt_pct {
                                Some(p) => {
                                    format!("{} {}", t.opt_working, t.decimal(format!("{p:.0}%")))
                                }
                                None => t.opt_working.to_owned(),
                            };
                            label(ui, &text, regular(12.5), TEXT);
                            if a.can_stop() && ghost_button(ui, t.stop, Some(icons::STOP)).clicked()
                            {
                                stop = true;
                            }
                        });
                        if let Some(p) = self.disk.opt_pct {
                            let w = ui.available_width();
                            let (r, _) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
                            bar(ui.painter(), r, p / 100.0, ACCENT_HI);
                        }
                    } else if let Some(o) = result {
                        let (text, color) = opt_outcome(o, t);
                        ui.horizontal(|ui| {
                            let icon = if o == OptOutcome::Done {
                                icons::CHECK
                            } else {
                                icons::WARNING
                            };
                            label(ui, icon, theme::icon(12.0), color);
                            label(ui, text, semibold(12.5), color);
                        });
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for (i, (action, text)) in buttons.iter().enumerate() {
                        let w = pill_width(ui, text, None).max(110.0);
                        let resp = if i == 0 {
                            primary_button(ui, vec2(w, 38.0), text, None, enabled)
                        } else {
                            ui.add_enabled_ui(enabled, |ui| ghost_button(ui, text, None))
                                .inner
                        };
                        if resp.clicked() && enabled {
                            clicked = Some(*action);
                        }
                    }
                });
            });
        });
        if stop {
            if let Some(c) = &self.disk.opt_cancel {
                c.store(true, Ordering::Relaxed);
            }
        }
        clicked
    }

    // -------------------------------------------------------------- fixtures

    /// Snapshot mode: made-up drives and results, so the pages render without scanning,
    /// measuring or changing anything, and without showing this computer's drives.
    #[cfg(debug_assertions)]
    pub(crate) fn load_disk_fixture(&mut self, page: &str) {
        let system = volume::system_letter();
        let gb = 1u64 << 30;
        if !self.disk.volumes.iter().any(|v| v.label == "Data") {
            let drive =
                |letter, label: &str, kind, media, fs: &str, total: f64, free: f64| Volume {
                    letter,
                    label: label.into(),
                    fs: fs.into(),
                    kind,
                    media,
                    total: (total * gb as f64) as u64,
                    free: (free * gb as f64) as u64,
                    system: letter == system,
                };
            let others: Vec<char> = ('D'..='Z').filter(|&l| l != system).take(2).collect();
            self.disk.volumes = vec![
                drive(system, "", Kind::Fixed, Media::Ssd, "NTFS", 931.5, 452.3),
                drive(
                    others[0],
                    "Data",
                    Kind::Fixed,
                    Media::Hdd,
                    "NTFS",
                    1863.0,
                    1210.4,
                ),
                drive(
                    others[1],
                    "Backup",
                    Kind::Removable,
                    Media::Unknown,
                    "exFAT",
                    476.9,
                    301.8,
                ),
            ];
            self.disk.listed = true;
            self.disk.letter = Some(system);
        }
        match page {
            "disk_analyze" if self.disk.report.is_none() => {
                self.disk.tab = Tab::Analyze;
                let n = |name: &str, parent: u32, size: f64| scan::Node {
                    name: name.into(),
                    parent,
                    size: (size * gb as f64) as u64,
                    files: 1000,
                };
                let nodes = vec![
                    n("C:\\", u32::MAX, 18.0),
                    n("Users", 0, 0.2),
                    n("Windows", 0, 31.0),
                    n("Program Files", 0, 24.5),
                    n("XboxGames", 0, 0.0),
                    n("ProgramData", 0, 7.8),
                    n("Program Files (x86)", 0, 11.2),
                    n("Ana", 1, 0.4),
                    n("Videos", 7, 64.3),
                    n("Downloads", 7, 38.9),
                    n("AppData", 7, 27.1),
                    n("Pictures", 7, 12.6),
                    n("Documents", 7, 4.2),
                    n("Forza Horizon 5", 4, 118.0),
                    n("Starfield", 4, 96.5),
                    n("Public", 1, 0.3),
                ];
                let file = |path: &str, size: f64| BigFile {
                    path: path.into(),
                    size: (size * gb as f64) as u64,
                };
                let mut kinds = [scan::KindTotal::default(); 7];
                for (k, (b, f)) in kinds.iter_mut().zip([
                    (71.0, 312),
                    (8.4, 4120),
                    (14.9, 38_000),
                    (5.1, 9_800),
                    (41.7, 220),
                    (288.0, 410_000),
                    (49.0, 830_000),
                ]) {
                    *k = scan::KindTotal {
                        bytes: (b * gb as f64) as u64,
                        files: f,
                    };
                }
                let report = Report {
                    root: "C:\\".into(),
                    tree: Tree::build(nodes),
                    top_files: vec![
                        file(r"C:\XboxGames\Forza Horizon 5\Content\media.pak", 38.2),
                        file(r"C:\Users\Ana\Videos\Ferias 2025.mp4", 12.8),
                        file(r"C:\hiberfil.sys", 12.7),
                        file(r"C:\Users\Ana\Downloads\Win11_24H2.iso", 5.8),
                        file(r"C:\pagefile.sys", 4.0),
                        file(r"C:\Users\Ana\Downloads\backup-fotos.zip", 3.1),
                    ],
                    kinds,
                    files: 1_284_331,
                    folders: 312_554,
                    denied: 12,
                    elapsed: std::time::Duration::from_secs(14),
                    cancelled: false,
                };
                self.disk.report = Some(Arc::new(report));
                self.disk.expanded = [1, 7, 4].into_iter().collect();
            }
            "disk_clean" if self.disk.measured_for.is_none() => {
                self.disk.tab = Tab::Clean;
                self.disk.measured_for = Some(system);
                let mb = 1u64 << 20;
                let done = |b: u64, blocked| {
                    Some(Status::Done {
                        bytes: b,
                        files: 0,
                        skipped: 0,
                        blocked,
                    })
                };
                let found = [
                    done(1_240 * mb, None),
                    done(342 * mb, None),
                    done(865 * mb, Some("Microsoft Edge")),
                    done(0, None),
                    done(2_150 * mb, None),
                    done(640 * mb, None),
                    done(12 * mb, None),
                    done(95 * mb, None),
                    Some(Status::Unavailable),
                    done(31 * mb, None),
                    done(410 * mb, None),
                    done(1_530 * mb, None),
                    done(0, None),
                    done(220 * mb, None),
                ];
                self.disk.found = found;
            }
            "disk_optimize" if !self.disk.info_ready => {
                self.disk.tab = Tab::Optimize;
                self.disk.info_for = Some(system);
                self.disk.info_ready = true;
                self.disk.trim = Some(true);
                self.disk.hiber = Some(12_700 * (1 << 20));
                self.disk.opt_results = vec![(Action::OptimizeDrive, OptOutcome::Done)];
            }
            _ => {}
        }
    }
}

fn file_list(ui: &mut Ui, files: &[BigFile]) {
    let t = tr();
    let mut show = None;
    egui::ScrollArea::vertical()
        .id_salt("disk_files")
        .auto_shrink([false, false])
        .show_rows(ui, FILE_ROW, files.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            for (i, f) in files.iter().enumerate().skip(range.start).take(range.len()) {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, FILE_ROW), Sense::hover());
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(rect, CornerRadius::same(7), CARD_HI);
                }
                let y = rect.center().y;
                let kind = FileKind::of_name(&f.path);
                p.text(
                    pos2(rect.left() + 16.0, y),
                    Align2::CENTER_CENTER,
                    icons::DOCUMENT,
                    theme::icon(14.0),
                    kind_color(kind),
                );
                let open_r = Rect::from_center_size(pos2(rect.right() - 16.0, y), vec2(28.0, 24.0));
                let size_right = open_r.left() - 8.0;
                let text_x = rect.left() + 34.0;
                let text_w = (size_right - 90.0 - text_x).max(20.0);
                let (folder, name) = f.path.rsplit_once('\\').unwrap_or(("", &f.path));
                let name_g = fitted_text(p, name, regular(13.0), TEXT, text_w, 1);
                p.galley(pos2(text_x, rect.top() + 3.0), name_g, TEXT);
                let folder_g = fitted_text(p, folder, regular(11.5), FAINT, text_w, 1);
                p.galley(pos2(text_x, rect.top() + 21.0), folder_g, FAINT);
                p.text(
                    pos2(size_right, y),
                    Align2::RIGHT_CENTER,
                    bytes(t, f.size),
                    semibold(12.5),
                    TEXT,
                );
                let id = ui.scope_id().with(("show_file", i));
                if row_button(ui, open_r, id, icons::OPEN_FOLDER, t.show_in_folder) {
                    show = Some(f.path.clone());
                }
            }
        });
    if let Some(p) = show {
        reveal::show_file(&p);
    }
}

fn type_list(ui: &mut Ui, r: &Report) {
    let t = tr();
    let mut kinds: Vec<(FileKind, scan::KindTotal)> = FileKind::ALL
        .into_iter()
        .zip(r.kinds)
        .filter(|(_, k)| k.files > 0)
        .collect();
    kinds.sort_by_key(|(_, k)| std::cmp::Reverse(k.bytes));
    let top = kinds.first().map_or(1, |(_, k)| k.bytes.max(1));
    egui::ScrollArea::vertical()
        .id_salt("disk_types")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            for (kind, k) in kinds {
                let (rect, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
                let p = ui.painter();
                let color = kind_color(kind);
                p.circle_filled(pos2(rect.left() + 12.0, rect.top() + 14.0), 5.0, color);
                p.text(
                    pos2(rect.left() + 26.0, rect.top() + 14.0),
                    Align2::LEFT_CENTER,
                    t.kinds[kind as usize],
                    semibold(13.0),
                    TEXT,
                );
                p.text(
                    pos2(rect.right() - 8.0, rect.top() + 14.0),
                    Align2::RIGHT_CENTER,
                    format!(
                        "{} · {}",
                        bytes(t, k.bytes),
                        (t.files_count)(&(t.count)(k.files))
                    ),
                    regular(12.5),
                    MUTED,
                );
                let bar_r = Rect::from_min_size(
                    pos2(rect.left() + 26.0, rect.top() + 28.0),
                    vec2(rect.width() - 34.0, 7.0),
                );
                bar(p, bar_r, k.bytes as f32 / top as f32, color);
            }
        });
}
