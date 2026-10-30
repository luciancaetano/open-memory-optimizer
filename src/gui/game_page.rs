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

//! Game Booster page: pick a process, watch its numbers, boost it and undo it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{pos2, vec2, Align, Align2, Layout, Sense, Ui};

use open_memory_optimizer::booster::{self as boost, Outcome, Session, Technique};
use open_memory_optimizer::i18n::{tr, Strings};
use open_memory_optimizer::process::{self, ProcInfo, ProcStats, ProcessMonitor};

use super::{App, Event, Level, Shared};
use crate::theme::{self, icons, *};
use crate::widgets::*;

/// Seconds of history in the tile charts.
const HISTORY: usize = 60;
const LIST_MAX_AGE: Duration = Duration::from_secs(5);

/// The process chosen in the picker.
pub struct Picked {
    pub pid: u32,
    pub name: String,
    pub title: Option<String>,
}

impl Picked {
    fn display(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Default)]
pub struct GamePage {
    procs: Vec<ProcInfo>,
    listed_at: Option<Instant>,
    filter: String,
    picked: Option<Picked>,
    history: VecDeque<ProcStats>,
    exited: bool,
    /// Bumped on every pick, so samples of an earlier pick are ignored.
    generation: u64,
    stop: Option<Arc<AtomicBool>>,
    /// Undone on drop, which also covers closing the app.
    session: Option<Session>,
    results: Vec<(Technique, Outcome)>,
    /// The results belong to a boost that is still on.
    boosted: bool,
    busy: bool,
}

impl GamePage {
    #[cfg(debug_assertions)]
    pub fn has_pick(&self) -> bool {
        self.picked.is_some()
    }
}

impl Drop for GamePage {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Release);
        }
    }
}

/// Sleeps `ms` in short slices; false if `stop` was set meanwhile.
fn wait(stop: &AtomicBool, ms: u64) -> bool {
    for _ in 0..ms / 100 {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !stop.load(Ordering::Acquire)
}

/// Samples the picked process once a second until it exits or another one is
/// picked. After it exits, waits for the same executable to start again and
/// reports it, so the page follows a game that was closed and reopened.
fn spawn_monitor(
    shared: Arc<Shared>,
    pid: u32,
    name: String,
    generation: u64,
    stop: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        if let Some(mut m) = ProcessMonitor::open(pid) {
            while let Some(s) = m.sample() {
                shared.send(Event::GameStats(generation, Some(s)));
                if !wait(&stop, 1000) {
                    return;
                }
            }
        }
        shared.send(Event::GameStats(generation, None));
        while wait(&stop, 2000) {
            if let Some(p) = process::find_relaunch(&process::list(), &name, pid) {
                shared.send(Event::GameReturned(generation, p));
                return;
            }
        }
    });
}

fn tech_title(tech: Technique, t: &'static Strings) -> &'static str {
    match tech {
        Technique::Priority => t.tech_priority_title,
        Technique::NoPowerSaving => t.tech_power_title,
        Technique::PowerPlan => t.tech_plan_title,
        Technique::Background => t.tech_background_title,
        Technique::FreeMemory => t.tech_memory_title,
        Technique::PerformanceCores => t.tech_cores_title,
        Technique::KeepMemory => t.tech_keep_title,
    }
}

fn outcome_text(o: Outcome, t: &'static Strings) -> String {
    match o {
        Outcome::Applied => t.outcome_boost_applied.into(),
        Outcome::AlreadyOn => t.outcome_boost_already.into(),
        Outcome::Failed => t.outcome_boost_failed.into(),
        Outcome::NotAvailable => t.outcome_boost_not_available.into(),
        Outcome::Calmed(n) => (t.outcome_boost_calmed)(n),
        Outcome::Freed(kb) if kb > 0 => (t.outcome_boost_freed)(&t.size(kb)),
        Outcome::Freed(_) => t.outcome_boost_applied.into(),
    }
}

fn outcome_color(o: Outcome) -> egui::Color32 {
    match o {
        Outcome::Failed => RED,
        Outcome::NotAvailable => AMBER,
        _ => GREEN,
    }
}

/// Picker row: window title (or executable) and, faded, the executable.
fn row_text(p: &ProcInfo) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |font, color| egui::TextFormat::simple(font, color);
    job.append(p.display_name(), 0.0, fmt(regular(13.5), TEXT));
    if p.title.is_some() {
        job.append(&p.name, 10.0, fmt(regular(12.0), FAINT));
    }
    job
}

impl App {
    fn refresh_processes(&mut self) {
        self.game.procs = process::list();
        self.game.listed_at = Some(Instant::now());
    }

    fn game_pick(&mut self, p: &ProcInfo) {
        let g = &mut self.game;
        if g.session.is_some() || g.busy {
            return;
        }
        if let Some(stop) = g.stop.take() {
            stop.store(true, Ordering::Release);
        }
        g.picked = Some(Picked {
            pid: p.pid,
            name: p.name.clone(),
            title: p.title.clone(),
        });
        g.history.clear();
        g.exited = false;
        g.results.clear();
        g.boosted = false;
        g.generation += 1;
        let stop = Arc::new(AtomicBool::new(false));
        g.stop = Some(stop.clone());
        spawn_monitor(
            self.shared.clone(),
            p.pid,
            p.name.clone(),
            g.generation,
            stop,
        );
    }

    fn game_boost(&mut self) {
        if self.game.busy || self.game.session.is_some() {
            return;
        }
        let Some(p) = &self.game.picked else { return };
        if self.game.exited {
            // Closed since it was picked: boost it again if it was reopened.
            let (name, old) = (p.name.clone(), p.pid);
            self.refresh_processes();
            match process::find_relaunch(&self.game.procs, &name, old) {
                Some(again) => self.game_pick(&again),
                None => {
                    let t = tr();
                    self.notify(t.boost_gone, t.boost_gone_body, Level::Warn);
                    return;
                }
            }
        }
        let Some(p) = &self.game.picked else { return };
        self.game.busy = true;
        let (pid, opts) = (p.pid, boost::Options::from_settings(&self.settings));
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            // Always answer, even if applying fails, so the button never stays busy.
            let session = std::panic::catch_unwind(|| boost::apply(pid, opts))
                .ok()
                .flatten()
                .map(Box::new);
            shared.send(Event::Boosted(pid, session));
        });
    }

    /// The closed game was started again: follow the new process.
    pub(crate) fn game_returned(&mut self, generation: u64, p: ProcInfo) {
        let g = &self.game;
        if generation == g.generation && g.exited && g.session.is_none() && !g.busy {
            self.game_pick(&p);
        }
    }

    /// Undoes the boost. `exited` picks the "the game closed" wording.
    fn game_restore(&mut self, exited: bool) {
        let had = self.game.session.take().map(|mut s| s.restore()).is_some();
        if !had && !self.game.boosted {
            return;
        }
        self.game.boosted = false;
        let t = tr();
        let name = self
            .game
            .picked
            .as_ref()
            .map_or(String::new(), |p| p.display().to_owned());
        let body = if exited {
            (t.boost_exited_body)(&name)
        } else {
            t.boost_restored_body.to_owned()
        };
        self.notify(t.boost_restored, &body, Level::Ok);
    }

    pub(crate) fn game_stats(&mut self, generation: u64, s: Option<ProcStats>) {
        if generation != self.game.generation {
            return;
        }
        match s {
            Some(s) => {
                self.game.history.push_back(s);
                while self.game.history.len() > HISTORY {
                    self.game.history.pop_front();
                }
            }
            None => {
                self.game.exited = true;
                self.game_restore(true);
            }
        }
    }

    pub(crate) fn game_boosted(&mut self, pid: u32, s: Option<Box<Session>>) {
        self.game.busy = false;
        let t = tr();
        let Some(s) = s else {
            self.notify(t.boost_gone, t.boost_gone_body, Level::Warn);
            return;
        };
        let current = self.game.picked.as_ref().is_some_and(|p| p.pid == pid);
        if !current || self.game.exited {
            // Closed or switched while boosting: undo right away (dropping restores).
            drop(s);
            return;
        }
        self.game.results = s.results.clone();
        self.game.session = Some(*s);
        self.game.boosted = true;
        let name = self.game.picked.as_ref().map_or("", |p| p.display());
        let level = if self.game.results.iter().all(|(_, o)| o.is_ok()) {
            Level::Ok
        } else {
            Level::Warn
        };
        let body = (t.boost_done_body)(name);
        self.notify(t.boost_done, &body, level);
    }

    /// Snapshot mode: this app itself stands in for a boosted game.
    #[cfg(debug_assertions)]
    pub(crate) fn load_game_fixture(&mut self) {
        self.refresh_processes();
        let own = std::process::id();
        let mut p = self
            .game
            .procs
            .iter()
            .find(|p| p.pid == own)
            .cloned()
            .unwrap_or_else(|| ProcInfo {
                pid: own,
                parent: 0,
                name: "OpenMemoryOptimizer.exe".into(),
                threads: 1,
                title: None,
            });
        p.title = Some("Cyber Racer 2077".into());
        p.name = "CyberRacer2077.exe".into();
        self.game_pick(&p);
        self.game.boosted = true;
        self.game.results = vec![
            (Technique::KeepMemory, Outcome::Applied),
            (Technique::Priority, Outcome::Applied),
            (Technique::NoPowerSaving, Outcome::Applied),
            (Technique::PowerPlan, Outcome::AlreadyOn),
            (Technique::PerformanceCores, Outcome::NotAvailable),
            (Technique::FreeMemory, Outcome::Freed(842_000)),
        ];
    }

    // ------------------------------------------------------------------ page

    pub(crate) fn game_page(&mut self, ui: &mut Ui) {
        let t = tr();
        Self::page_header(ui, t.nav_booster, t.booster_sub);
        if self.game.listed_at.is_none() {
            self.refresh_processes();
        }
        let gap = 12.0;
        let full_w = ui.available_width();
        ui.spacing_mut().item_spacing.y = gap;

        self.picker_card(ui);

        let tile_h = 96.0;
        let tiles_h = tile_h * 2.0 + gap;
        if self.game.picked.is_some() {
            self.tiles(ui, full_w, tile_h, gap);
        } else {
            card_sized(ui, vec2(full_w, tiles_h), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(24.0);
                    let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                    ui.painter()
                        .circle_filled(r.center(), 28.0, ACCENT.gamma_multiply(0.2));
                    ui.painter().text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        icons::GAME,
                        theme::icon(24.0),
                        ACCENT_HI,
                    );
                    ui.add_space(4.0);
                    label(ui, t.booster_empty_title, semibold(16.0), TEXT);
                    ui.set_max_width(460.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(t.booster_empty_body)
                                .font(regular(13.0))
                                .color(MUTED),
                        )
                        .wrap(),
                    );
                });
            });
        }

        let h = (ui.available_height() - 1.0).max(200.0);
        let right_w = 290.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            card_sized(ui, vec2(full_w - right_w - gap, h), |ui| self.options(ui));
            card_sized(ui, vec2(right_w, h), |ui| self.action(ui));
        });
    }

    fn picker_card(&mut self, ui: &mut Ui) {
        let t = tr();
        let enabled = self.game.session.is_none() && !self.game.busy;
        let mut choice: Option<ProcInfo> = None;
        let mut refresh = false;
        let mut opened = false;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let text = self
                    .game
                    .picked
                    .as_ref()
                    .map_or(t.booster_pick, |p| p.display())
                    .to_owned();
                let g = &mut self.game;
                let (procs, filter, picked) = (&g.procs, &mut g.filter, g.picked.as_ref());
                ui.add_enabled_ui(enabled, |ui| {
                    let r = egui::ComboBox::from_id_salt("game_picker")
                        .width(380.0)
                        .height(380.0)
                        .truncate()
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .selected_text(egui::RichText::new(text).font(regular(14.0)).color(TEXT))
                        .show_ui(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(filter)
                                    .hint_text(t.booster_search)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.add_space(4.0);
                            egui::ScrollArea::vertical()
                                .max_height(320.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    let mut any = false;
                                    for (windowed, heading) in [
                                        (true, t.booster_open_apps),
                                        (false, t.booster_other_processes),
                                    ] {
                                        let rows: Vec<&ProcInfo> = procs
                                            .iter()
                                            .filter(|p| p.title.is_some() == windowed)
                                            .filter(|p| process::matches(p, filter))
                                            .collect();
                                        if rows.is_empty() {
                                            continue;
                                        }
                                        any = true;
                                        label(ui, heading, semibold(12.0), MUTED);
                                        for p in rows {
                                            let sel = picked.is_some_and(|x| x.pid == p.pid);
                                            if ui.selectable_label(sel, row_text(p)).clicked() {
                                                choice = Some(p.clone());
                                                ui.close();
                                            }
                                        }
                                        ui.add_space(6.0);
                                    }
                                    if !any {
                                        label(ui, t.booster_no_match, regular(13.0), MUTED);
                                    }
                                });
                        });
                    opened = r.response.clicked();
                });
                ui.add_enabled_ui(enabled, |ui| {
                    refresh |= ghost_button(ui, t.booster_refresh, Some(icons::REFRESH)).clicked();
                });
                if self.game.picked.is_some() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.game.boosted {
                            chip(ui, Some(icons::BOLT), t.booster_on, GREEN);
                        } else if self.game.exited {
                            chip(ui, None, t.booster_closed, FAINT);
                        } else {
                            chip(ui, None, t.booster_off, MUTED);
                        }
                    });
                }
            });
            if let Some(p) = &self.game.picked {
                ui.add_space(-4.0);
                label(
                    ui,
                    &(t.booster_process_line)(&p.name, p.pid),
                    regular(12.0),
                    FAINT,
                );
            }
        });
        let stale = self
            .game
            .listed_at
            .is_none_or(|at| at.elapsed() > LIST_MAX_AGE);
        if refresh || opened && stale {
            self.refresh_processes();
        }
        if let Some(p) = choice {
            self.game_pick(&p);
        }
    }

    fn tiles(&self, ui: &mut Ui, full_w: f32, h: f32, gap: f32) {
        let t = tr();
        let w = (full_w - 2.0 * gap) / 3.0;
        let size = vec2(w, h);
        let hist = &self.game.history;
        let last = hist.back();
        let series = |f: &dyn Fn(&ProcStats) -> f32| hist.iter().map(f).collect::<Vec<f32>>();
        let top = |v: &[f32]| v.iter().copied().fold(0.0, f32::max) * 1.6;
        let dash = "—".to_owned();
        let kb = |b: u64| t.size((b / 1024) as i64);
        let rate = |b: f64| format!("{}{}", kb(b as u64), t.per_second);
        let pct = |v: f32| {
            if v < 10.0 {
                t.decimal(format!("{v:.1}%"))
            } else {
                format!("{v:.0}%")
            }
        };

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let cpu = series(&|s| s.cpu_pct);
            spark_tile(
                ui,
                size,
                t.tile_cpu,
                &last.map_or(dash.clone(), |s| pct(s.cpu_pct)),
                t.tile_cpu_sub,
                CYAN,
                &cpu,
                HISTORY,
                100.0,
            );
            let mem = series(&|s| s.working_set as f32);
            spark_tile(
                ui,
                size,
                t.tile_memory,
                &last.map_or(dash.clone(), |s| kb(s.working_set)),
                &last.map_or(String::new(), |s| (t.tile_memory_sub)(&kb(s.private_bytes))),
                VIOLET,
                &mem,
                HISTORY,
                top(&mem),
            );
            let gpu_known = last.is_some_and(|s| s.gpu_pct.is_some());
            let gpu = series(&|s| s.gpu_pct.unwrap_or(0.0));
            spark_tile(
                ui,
                size,
                t.tile_gpu,
                &last.and_then(|s| s.gpu_pct).map_or(dash.clone(), pct),
                if gpu_known || last.is_none() {
                    t.tile_gpu_sub
                } else {
                    t.tile_unavailable
                },
                GREEN,
                &gpu,
                HISTORY,
                100.0,
            );
        });
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let vram = series(&|s| s.vram.unwrap_or(0) as f32);
            spark_tile(
                ui,
                size,
                t.tile_vram,
                &last.and_then(|s| s.vram).map_or(dash.clone(), kb),
                if last.is_none() || last.is_some_and(|s| s.vram.is_some()) {
                    t.tile_vram_sub
                } else {
                    t.tile_unavailable
                },
                ACCENT_HI,
                &vram,
                HISTORY,
                top(&vram),
            );
            let disk = series(&|s| (s.read_bps + s.write_bps) as f32);
            spark_tile(
                ui,
                size,
                t.tile_disk,
                &last.map_or(dash.clone(), |s| rate(s.read_bps + s.write_bps)),
                &last.map_or(String::new(), |s| {
                    (t.tile_disk_sub)(&rate(s.read_bps), &rate(s.write_bps))
                }),
                AMBER,
                &disk,
                HISTORY,
                top(&disk).max(1024.0 * 1024.0),
            );
            spark_tile(
                ui,
                size,
                t.tile_uptime,
                &last.map_or(dash.clone(), |s| (t.duration)(s.uptime_secs)),
                &last.map_or(String::new(), |s| (t.tile_uptime_sub)(s.threads)),
                RED,
                &[],
                HISTORY,
                0.0,
            );
        });
    }

    fn options(&mut self, ui: &mut Ui) {
        let t = tr();
        card_title(ui, t.boost_options, Some(t.boost_options_sub));
        ui.add_space(2.0);
        let mut changed = false;
        let boosted = self.game.session.is_some();
        egui::ScrollArea::vertical()
            .id_salt("boost_options")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    label(ui, icons::SHIELD, theme::icon(14.0), GREEN);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(t.booster_safe)
                                .font(regular(12.5))
                                .color(GREEN),
                        )
                        .wrap(),
                    );
                });
                if boosted {
                    label(ui, t.boost_note, regular(12.0), AMBER);
                }
                let o = &mut self.settings.booster;
                for (title, desc, value) in [
                    (
                        t.tech_priority_title,
                        t.tech_priority_desc,
                        &mut o.high_priority,
                    ),
                    (
                        t.tech_power_title,
                        t.tech_power_desc,
                        &mut o.no_power_saving,
                    ),
                    (t.tech_plan_title, t.tech_plan_desc, &mut o.power_plan),
                    (t.tech_memory_title, t.tech_memory_desc, &mut o.free_memory),
                    (
                        t.tech_background_title,
                        t.tech_background_desc,
                        &mut o.calm_background,
                    ),
                    (
                        t.tech_cores_title,
                        t.tech_cores_desc,
                        &mut o.performance_cores,
                    ),
                ] {
                    changed |= setting_toggle(ui, title, desc, value);
                    ui.separator();
                }
                // Technique 7 is always on: shown with a check instead of a switch.
                ui.horizontal_top(|ui| {
                    let w = ui.available_width();
                    ui.allocate_ui_with_layout(
                        vec2(w - 64.0, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            label(ui, t.tech_keep_title, semibold(14.0), TEXT);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(t.tech_keep_desc)
                                        .font(regular(12.5))
                                        .color(MUTED),
                                )
                                .wrap(),
                            );
                        },
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(12.0);
                        label(ui, icons::CHECK, theme::icon(16.0), GREEN);
                    });
                });
            });
        if changed {
            self.settings_changed();
        }
    }

    fn action(&mut self, ui: &mut Ui) {
        let t = tr();
        let btn = vec2(ui.available_width(), 46.0);
        if self.game.busy {
            primary_button(ui, btn, t.boost_busy, Some(icons::BOLT), false);
        } else if self.game.boosted {
            if stop_button(ui, btn, t.restore_button, Some(icons::UNDO), true).clicked() {
                self.game_restore(false);
            }
        } else {
            let can = self.game.picked.is_some();
            if primary_button(ui, btn, t.boost_button, Some(icons::BOLT), can).clicked() {
                self.game_boost();
            }
        }
        if self.game.exited && !self.game.busy {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(t.booster_closed_hint)
                        .font(regular(12.0))
                        .color(AMBER),
                )
                .wrap(),
            );
        }
        ui.add_space(6.0);
        card_title(ui, t.boost_result, None);
        if self.game.results.is_empty() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(t.boost_result_empty)
                        .font(regular(13.0))
                        .color(MUTED),
                )
                .wrap(),
            );
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("boost_results")
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                for &(tech, outcome) in &self.game.results {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        let (d, _) = ui.allocate_exact_size(vec2(7.0, 16.0), Sense::hover());
                        ui.painter().circle_filled(
                            pos2(d.center().x, d.center().y),
                            3.5,
                            outcome_color(outcome),
                        );
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(tech_title(tech, t))
                                        .font(semibold(12.5))
                                        .color(TEXT),
                                )
                                .wrap(),
                            );
                            label(ui, &outcome_text(outcome, t), regular(12.0), MUTED);
                        });
                    });
                }
            });
    }
}
