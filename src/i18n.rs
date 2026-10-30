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

//! Interface text in English and Brazilian Portuguese. Not in the original, which is
//! English only.
//!
//! Each language is one [`Strings`] table, so a missing translation does not compile.
//! Texts that contain values are functions, which lets each language order its words.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::win;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    PtBr,
}

impl Lang {
    /// Value of the `Language` setting.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::PtBr => "pt-BR",
        }
    }

    /// Accepts "en", "pt-BR" and their bare or regional variants, ignoring case.
    pub fn from_code(code: &str) -> Option<Lang> {
        let code = code.trim().to_ascii_lowercase();
        let primary = code.split(['-', '_']).next().unwrap_or("");
        match primary {
            "en" => Some(Lang::En),
            "pt" => Some(Lang::PtBr),
            _ => None,
        }
    }

    /// Windows LANGID to language: Portuguese (any region) or English for the rest.
    pub fn from_langid(id: u16) -> Lang {
        const LANG_PORTUGUESE: u16 = 0x16;
        if id & 0x3ff == LANG_PORTUGUESE {
            Lang::PtBr
        } else {
            Lang::En
        }
    }

    /// The Windows display language of the signed-in user.
    pub fn system() -> Lang {
        Self::from_langid(unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() })
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::En => &EN,
            Lang::PtBr => &PT_BR,
        }
    }
}

/// The saved choice (`None` follows Windows) turned into a language.
pub fn resolve(choice: Option<Lang>) -> Lang {
    choice.unwrap_or_else(Lang::system)
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == Lang::PtBr as u8 {
        Lang::PtBr
    } else {
        Lang::En
    }
}

/// Text of the current language.
pub fn tr() -> &'static Strings {
    current().strings()
}

pub struct Strings {
    decimal_sep: &'static str,

    // Title bar and sidebar
    pub admin_mode: &'static str,
    pub limited_mode: &'static str,
    pub nav_overview: &'static str,
    pub nav_settings: &'static str,
    pub nav_schedule: &'static str,
    pub nav_about: &'static str,
    pub auto_off: &'static str,
    pub auto_below: fn(i64) -> String,
    pub auto_every: fn(i64) -> String,

    // Overview
    pub overview_sub: &'static str,
    pub checking_memory: &'static str,
    pub gauge_in_use: &'static str,
    pub stop: &'static str,
    pub stopping: &'static str,
    pub free_up_memory: &'static str,
    pub status_healthy: &'static str,
    pub status_healthy_sub: &'static str,
    pub status_busy: &'static str,
    pub status_busy_sub: &'static str,
    pub status_low: &'static str,
    pub status_low_sub: &'static str,
    pub total_memory: &'static str,
    pub total_memory_sub: &'static str,
    pub in_use: &'static str,
    pub in_use_sub: fn(u32) -> String,
    pub available: &'static str,
    pub available_sub: &'static str,
    pub status: &'static str,
    pub cached: &'static str,
    pub cached_sub: &'static str,
    pub cached_unavailable: &'static str,
    pub processor: &'static str,
    pub cpu_resting: &'static str,
    pub cpu_working: &'static str,
    pub memory_use: &'static str,
    pub last_2_minutes: &'static str,

    // Last cleanup card
    pub last_cleanup: &'static str,
    pub no_cleanup_yet: &'static str,
    pub mode_auto: &'static str,
    pub mode_manual: &'static str,
    /// Time, mode, seconds.
    pub result_meta: fn(&str, &str, f64) -> String,
    pub step_clipboard: &'static str,
    pub step_deep_clean: &'static str,
    pub step_open_apps: &'static str,
    pub step_cached_memory: &'static str,
    pub step_system_cache: &'static str,
    pub step_result: &'static str,
    pub outcome_cleared: &'static str,
    pub outcome_clear_failed: &'static str,
    pub outcome_done: &'static str,
    pub outcome_partly_done: &'static str,
    pub outcome_apps_trimmed: fn(u32) -> String,
    pub outcome_unreachable: &'static str,
    pub outcome_needs_admin: &'static str,
    pub outcome_unsupported: &'static str,
    pub outcome_skipped_limits: &'static str,

    // Progress under the Stop button
    pub progress_preparing: &'static str,
    pub progress_auto: &'static str,
    pub progress_clipboard: &'static str,
    pub progress_defrag: &'static str,
    pub progress_trim: &'static str,
    pub progress_standby: &'static str,
    pub progress_file_cache: &'static str,

    // Notifications
    pub save_failed: &'static str,
    pub nothing_to_clean: &'static str,
    pub nothing_to_clean_body: &'static str,
    pub already_running: &'static str,
    pub already_running_body: &'static str,
    pub cleanup_stopped: &'static str,
    pub freed_before_stop: fn(&str) -> String,
    pub nothing_freed: &'static str,
    pub nothing_freed_body: &'static str,
    pub freed: fn(&str) -> String,
    /// Before, after, seconds.
    pub available_change: fn(&str, &str, f64) -> String,

    // Settings
    pub settings_sub: &'static str,
    pub what_to_clean: &'static str,
    pub what_to_clean_sub: &'static str,
    pub trim_title: &'static str,
    pub trim_desc: &'static str,
    pub standby_title: &'static str,
    pub standby_desc: &'static str,
    pub defrag_title: &'static str,
    pub defrag_desc: &'static str,
    pub clipboard_title: &'static str,
    pub clipboard_desc: &'static str,
    pub auto_title: &'static str,
    pub auto_sub: &'static str,
    pub auto_toggle: &'static str,
    pub auto_toggle_desc: &'static str,
    pub auto_low_memory: &'static str,
    pub auto_timer: &'static str,
    pub clean_below: &'static str,
    pub idle_title: &'static str,
    pub idle_desc: &'static str,
    pub clean_every: &'static str,
    pub general: &'static str,
    pub notice_title: &'static str,
    pub notice_desc: &'static str,
    pub tray_title: &'static str,
    pub tray_desc: &'static str,
    pub language: &'static str,
    pub language_desc: &'static str,
    pub language_auto: &'static str,

    // Schedule
    pub schedule_sub: &'static str,
    pub when_to_clean: &'static str,
    pub sched_daily: &'static str,
    pub sched_weekly: &'static str,
    pub sched_monthly: &'static str,
    pub sched_idle: &'static str,
    pub sched_logon: &'static str,
    pub sched_time: &'static str,
    /// Monday first.
    pub weekdays: [&'static str; 7],
    pub sched_month_day: &'static str,
    pub sched_idle_desc: &'static str,
    pub sched_logon_desc: &'static str,
    pub create_schedule: &'static str,
    pub remove_schedule: &'static str,
    pub sched_created: &'static str,
    pub sched_create_failed: &'static str,
    pub sched_removed: &'static str,
    pub sched_nothing_to_remove: &'static str,

    // About
    pub version_line: fn(&str) -> String,
    pub what_happens: &'static str,
    pub about_clipboard: &'static str,
    pub about_deep_clean: &'static str,
    pub about_open_apps: &'static str,
    pub about_cached_memory: &'static str,
    pub about_system_cache: &'static str,
    pub about_result: &'static str,
    pub good_to_know: &'static str,
    pub good_to_know_body: &'static str,
    pub license: &'static str,
    pub license_body: &'static str,
    pub read_gpl: &'static str,

    // Game Booster
    pub nav_booster: &'static str,
    pub booster_sub: &'static str,
    pub booster_pick: &'static str,
    pub booster_search: &'static str,
    pub booster_open_apps: &'static str,
    pub booster_other_processes: &'static str,
    pub booster_no_match: &'static str,
    pub booster_refresh: &'static str,
    pub booster_empty_title: &'static str,
    pub booster_empty_body: &'static str,
    pub booster_on: &'static str,
    pub booster_off: &'static str,
    pub booster_closed: &'static str,
    pub booster_closed_hint: &'static str,
    /// Executable name, PID.
    pub booster_process_line: fn(&str, u32) -> String,
    pub tile_cpu: &'static str,
    pub tile_cpu_sub: &'static str,
    pub tile_memory: &'static str,
    /// Private memory.
    pub tile_memory_sub: fn(&str) -> String,
    pub tile_gpu: &'static str,
    pub tile_gpu_sub: &'static str,
    pub tile_vram: &'static str,
    pub tile_vram_sub: &'static str,
    pub tile_unavailable: &'static str,
    pub tile_disk: &'static str,
    /// Read rate, write rate.
    pub tile_disk_sub: fn(&str, &str) -> String,
    pub tile_uptime: &'static str,
    /// Number of threads.
    pub tile_uptime_sub: fn(u32) -> String,
    /// Seconds as "1 h 05 min", "12 min" or "45 s".
    pub duration: fn(u64) -> String,
    pub per_second: &'static str,
    pub boost_options: &'static str,
    pub boost_options_sub: &'static str,
    pub booster_safe: &'static str,
    pub tech_priority_title: &'static str,
    pub tech_priority_desc: &'static str,
    pub tech_power_title: &'static str,
    pub tech_power_desc: &'static str,
    pub tech_plan_title: &'static str,
    pub tech_plan_desc: &'static str,
    pub tech_background_title: &'static str,
    pub tech_background_desc: &'static str,
    pub tech_memory_title: &'static str,
    pub tech_memory_desc: &'static str,
    pub tech_cores_title: &'static str,
    pub tech_cores_desc: &'static str,
    pub tech_keep_title: &'static str,
    pub tech_keep_desc: &'static str,
    pub boost_button: &'static str,
    pub boost_busy: &'static str,
    pub restore_button: &'static str,
    pub boost_result: &'static str,
    pub boost_result_empty: &'static str,
    pub boost_note: &'static str,
    pub outcome_boost_applied: &'static str,
    pub outcome_boost_already: &'static str,
    pub outcome_boost_failed: &'static str,
    pub outcome_boost_not_available: &'static str,
    pub outcome_boost_calmed: fn(u32) -> String,
    pub outcome_boost_freed: fn(&str) -> String,
    pub boost_done: &'static str,
    /// Game name.
    pub boost_done_body: fn(&str) -> String,
    pub boost_gone: &'static str,
    pub boost_gone_body: &'static str,
    pub boost_restored: &'static str,
    pub boost_restored_body: &'static str,
    /// Game name.
    pub boost_exited_body: fn(&str) -> String,

    // Disk: drive picker and tabs
    pub nav_disk: &'static str,
    pub disk_sub: &'static str,
    pub disk_local: &'static str,
    /// Free, total.
    pub disk_free_of: fn(&str, &str) -> String,
    pub media_ssd: &'static str,
    pub media_hdd: &'static str,
    pub media_removable: &'static str,
    pub media_network: &'static str,
    pub disk_refresh: &'static str,
    pub tab_analyze: &'static str,
    pub tab_clean: &'static str,
    pub tab_optimize: &'static str,
    /// A whole number with this language's thousands separator.
    pub count: fn(u64) -> String,

    // Disk: analyze
    pub analyze_button: &'static str,
    pub analyze_again: &'static str,
    pub analyze_empty_title: &'static str,
    pub analyze_empty_body: &'static str,
    pub analyze_reading: &'static str,
    /// Files (already formatted), size.
    pub analyze_progress: fn(&str, &str) -> String,
    /// Files, folders (already formatted), seconds.
    pub analyze_summary: fn(&str, &str, f64) -> String,
    pub analyze_stopped: &'static str,
    /// Size not found in any folder.
    pub analyze_unread: fn(&str) -> String,
    pub analyze_read_only: &'static str,
    pub analyze_used: &'static str,
    pub view_folders: &'static str,
    pub view_files: &'static str,
    pub view_types: &'static str,
    pub open_folder: &'static str,
    pub show_in_folder: &'static str,
    /// Number of files, already formatted.
    pub files_count: fn(&str) -> String,
    /// Number of smaller folders not listed.
    pub more_items: fn(usize) -> String,
    /// In `disk::scan::FileKind::ALL` order.
    pub kinds: [&'static str; 7],

    // Disk: clean up
    pub clean_title: &'static str,
    pub clean_sub: &'static str,
    pub clean_check: &'static str,
    pub clean_checking: &'static str,
    pub clean_cleaning: &'static str,
    /// Size to free.
    pub clean_button: fn(&str) -> String,
    pub clean_nothing_selected: &'static str,
    /// Size that can be freed.
    pub clean_total: fn(&str) -> String,
    pub clean_total_sub: &'static str,
    /// In `disk::clean::Category::ALL` order.
    pub cat_titles: [&'static str; crate::disk::clean::COUNT],
    pub cat_descs: [&'static str; crate::disk::clean::COUNT],
    pub cat_careful: &'static str,
    pub cat_none: &'static str,
    /// Browser name.
    pub cat_blocked: fn(&str) -> String,
    pub cat_unavailable: &'static str,
    pub cat_failed: &'static str,
    /// Size removed.
    pub cat_removed: fn(&str) -> String,
    /// Number of files in use (already formatted).
    pub cat_in_use: fn(&str) -> String,
    /// Letter of the Windows drive.
    pub clean_only_bin: fn(char) -> String,
    /// Size freed.
    pub clean_done: fn(&str) -> String,
    /// Free space before, after.
    pub clean_done_body: fn(&str, &str) -> String,
    pub clean_nothing: &'static str,
    pub clean_nothing_body: &'static str,
    pub clean_stopped: &'static str,
    pub clean_warning: &'static str,

    // Disk: optimize
    pub optimize_sub: &'static str,
    pub opt_drive_title: &'static str,
    pub opt_drive_ssd: &'static str,
    pub opt_drive_hdd: &'static str,
    pub opt_drive_unknown: &'static str,
    pub opt_drive_button: &'static str,
    pub opt_trim_title: &'static str,
    pub opt_trim_on: &'static str,
    pub opt_trim_off: &'static str,
    pub opt_trim_unknown: &'static str,
    pub opt_trim_button: &'static str,
    pub opt_hiber_title: &'static str,
    /// Size of the hibernate file.
    pub opt_hiber_desc: fn(&str) -> String,
    pub opt_hiber_none: &'static str,
    pub opt_hiber_smaller: &'static str,
    pub opt_hiber_disable: &'static str,
    pub opt_hiber_note: &'static str,
    pub opt_components_title: &'static str,
    pub opt_components_desc: &'static str,
    pub opt_components_button: &'static str,
    pub opt_working: &'static str,
    pub opt_done: &'static str,
    pub opt_failed: &'static str,
    pub opt_stopped: &'static str,
    pub opt_not_supported: &'static str,

    // Tray menu
    pub tray_open: &'static str,
    pub tray_exit: &'static str,
}

/// `n` with `sep` between groups of three digits: 1234567 -> "1,234,567".
fn grouped(n: u64, sep: char) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

fn duration_text(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {:02} min", secs / 3600, secs / 60 % 60),
    }
}

impl Strings {
    /// A number formatted with `format!` ("1.5"), with this language's decimal separator.
    pub fn decimal(&self, s: String) -> String {
        if self.decimal_sep == "." {
            s
        } else {
            s.replace('.', self.decimal_sep)
        }
    }

    /// KB as "512 KB", "2.00 MB" or "3.00 GB", with this language's decimal separator.
    pub fn size(&self, kb: i64) -> String {
        self.decimal(win::format_kb(kb))
    }
}

pub static EN: Strings = Strings {
    decimal_sep: ".",

    admin_mode: "Admin mode",
    limited_mode: "Limited mode",
    nav_overview: "Overview",
    nav_settings: "Settings",
    nav_schedule: "Schedule",
    nav_about: "About",
    auto_off: "Auto clean is off",
    auto_below: |pct| format!("Auto clean below {pct}% free"),
    auto_every: |min| format!("Auto clean every {min} min"),

    overview_sub: "Your computer's memory, updated live",
    checking_memory: "Checking your memory…",
    gauge_in_use: "in use",
    stop: "Stop",
    stopping: "Stopping…",
    free_up_memory: "Free up memory",
    status_healthy: "Healthy",
    status_healthy_sub: "plenty of free memory",
    status_busy: "Getting busy",
    status_busy_sub: "memory is filling up",
    status_low: "Running low",
    status_low_sub: "try freeing up memory",
    total_memory: "Total memory",
    total_memory_sub: "installed in this PC",
    in_use: "In use",
    in_use_sub: |pct| format!("{pct}% of total"),
    available: "Available",
    available_sub: "ready for your apps",
    status: "Status",
    cached: "Cached",
    cached_sub: "kept ready by Windows",
    cached_unavailable: "not available on this PC",
    processor: "Processor",
    cpu_resting: "resting",
    cpu_working: "working",
    memory_use: "Memory use",
    last_2_minutes: "last 2 minutes",

    last_cleanup: "Last cleanup",
    no_cleanup_yet: "No cleanup yet. Click “Free up memory” to start.",
    mode_auto: "automatic",
    mode_manual: "manual",
    result_meta: |at, mode, secs| format!("{at} · {mode} · took {secs:.0} s"),
    step_clipboard: "Clipboard",
    step_deep_clean: "Deep clean",
    step_open_apps: "Open apps",
    step_cached_memory: "Cached memory",
    step_system_cache: "System cache",
    step_result: "Result",
    outcome_cleared: "Cleared",
    outcome_clear_failed: "Couldn't clear it",
    outcome_done: "Done",
    outcome_partly_done: "Partly done",
    outcome_apps_trimmed: |n| format!("{n} apps slimmed down"),
    outcome_unreachable: "Couldn't reach them",
    outcome_needs_admin: "Needs administrator rights",
    outcome_unsupported: "Not supported here",
    outcome_skipped_limits: "Skipped (custom limits set)",

    progress_preparing: "Getting ready…",
    progress_auto: "Automatic cleanup…",
    progress_clipboard: "Clearing the clipboard…",
    progress_defrag: "Deep cleaning… this can take a while",
    progress_trim: "Freeing memory from open apps…",
    progress_standby: "Clearing cached memory…",
    progress_file_cache: "Tidying up the system cache…",

    save_failed: "Couldn't save your settings",
    nothing_to_clean: "Nothing to clean",
    nothing_to_clean_body: "Turn on at least one cleanup option in Settings.",
    already_running: "Already running",
    already_running_body: "A cleanup is already in progress.",
    cleanup_stopped: "Cleanup stopped",
    freed_before_stop: |size| format!("Freed {size} before stopping."),
    nothing_freed: "No extra memory was freed",
    nothing_freed_body: "No extra memory was freed.",
    freed: |size| format!("Freed {size} of memory"),
    available_change: |before, after, secs| {
        format!("Available memory: {before} → {after} (took {secs:.0} s)")
    },

    settings_sub: "Changes are saved automatically",
    what_to_clean: "What to clean",
    what_to_clean_sub: "Used when you click “Free up memory”",
    trim_title: "Free memory from open apps",
    trim_desc: "Asks your running apps to give back memory they aren't using right now.",
    standby_title: "Clear cached memory",
    standby_desc: "Removes files Windows keeps in memory for quick access. Needs administrator rights.",
    defrag_title: "Deep clean",
    defrag_desc: "Briefly uses up to 1 GB of memory to push unused data out. Can take a few minutes.",
    clipboard_title: "Clear clipboard",
    clipboard_desc: "Empties whatever you last copied.",
    auto_title: "Automatic cleanup",
    auto_sub: "A light cleanup that runs in the background",
    auto_toggle: "Clean automatically",
    auto_toggle_desc: "Keeps working while the app is in the system tray.",
    auto_low_memory: "When memory is low",
    auto_timer: "On a timer",
    clean_below: "Clean when free memory drops below",
    idle_title: "Only when the computer is resting",
    idle_desc: "Waits until your computer isn't busy. Checked every 5 minutes.",
    clean_every: "Clean every",
    general: "General",
    notice_title: "Show a notification",
    notice_desc: "Tells you how much memory was freed after each cleanup.",
    tray_title: "Keep in the system tray",
    tray_desc: "Minimizing or closing the window hides it next to the clock. Click the tray icon to bring it back, or right-click it to exit.",
    language: "Language",
    language_desc: "Automatic uses the language of Windows.",
    language_auto: "Automatic",

    schedule_sub: "Clean up automatically at set times, even when the app is closed",
    when_to_clean: "When to clean",
    sched_daily: "Daily",
    sched_weekly: "Weekly",
    sched_monthly: "Monthly",
    sched_idle: "When idle",
    sched_logon: "At sign-in",
    sched_time: "Time",
    weekdays: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    sched_month_day: "Day of the month",
    sched_idle_desc: "Runs after your computer has been unused for 10 minutes.",
    sched_logon_desc: "Runs every time you sign in to Windows.",
    create_schedule: "Create schedule",
    remove_schedule: "Remove schedule",
    sched_created: "Schedule created.",
    sched_create_failed: "Couldn't create the schedule. Try running the app as administrator.",
    sched_removed: "Schedule removed.",
    sched_nothing_to_remove: "There's no schedule to remove.",

    version_line: |v| format!("Version {v} · a simple memory optimizer for Windows"),
    what_happens: "What happens during a cleanup",
    about_clipboard: "Empties whatever you last copied (if turned on).",
    about_deep_clean: "Briefly uses spare memory to push unused data out (if turned on).",
    about_open_apps: "Asks every running app to give back memory it isn't using.",
    about_cached_memory: "Removes files Windows kept in memory for quick access (if turned on).",
    about_system_cache: "Trims the memory Windows uses to speed up opening files.",
    about_result: "Waits a few seconds, then shows how much memory became available.",
    good_to_know: "Good to know",
    good_to_know_body: "OpenMemoryOptimizer always shows the real amount of memory freed. Freed memory fills up again as you \
                        keep using your apps, and that's normal. Windows already manages memory well on its own, \
                        so a cleanup helps most when your computer is running low on memory.",
    license: "License",
    license_body: "Copyright © 2026 Lucian Caetano. OpenMemoryOptimizer is free software: you can \
                   share and change it under the GNU General Public License, version 3 or later. \
                   It comes with no warranty.",
    read_gpl: "Read the GNU GPL v3",

    nav_booster: "Game Booster",
    booster_sub: "Give a game more of your computer while you play",
    booster_pick: "Choose a game…",
    booster_search: "Search by name",
    booster_open_apps: "Open apps",
    booster_other_processes: "Everything else running",
    booster_no_match: "Nothing matches your search.",
    booster_refresh: "Refresh list",
    booster_empty_title: "Choose the game you want to boost",
    booster_empty_body: "Start your game, then pick it in the list above. You will see how much of your \
                         computer it uses, and you can boost it with one click.",
    booster_on: "Boosted",
    booster_off: "Not boosted",
    booster_closed: "Closed",
    booster_closed_hint: "The game was closed. Open it again and it is picked here by itself.",
    booster_process_line: |name, pid| format!("{name} · ID {pid}"),
    tile_cpu: "Processor",
    tile_cpu_sub: "of the whole processor",
    tile_memory: "Memory",
    tile_memory_sub: |private| format!("{private} only for this game"),
    tile_gpu: "Graphics",
    tile_gpu_sub: "graphics card in use",
    tile_vram: "Video memory",
    tile_vram_sub: "on the graphics card",
    tile_unavailable: "not available",
    tile_disk: "Disk",
    tile_disk_sub: |read, write| format!("reading {read} · writing {write}"),
    tile_uptime: "Playing for",
    tile_uptime_sub: |threads| format!("{threads} tasks running"),
    duration: duration_text,
    per_second: "/s",
    boost_options: "What Game Booster does",
    boost_options_sub: "Everything is undone when you restore or close the game",
    booster_safe: "Never reads or changes the game's memory or files, and leaves anti-cheat alone. \
                   It only changes how Windows shares your computer, like Task Manager does.",
    tech_priority_title: "Put the game first",
    tech_priority_desc: "The processor serves the game before other apps.",
    tech_power_title: "Never slow the game down to save energy",
    tech_power_desc: "Windows keeps the game on its fastest settings, even on battery.",
    tech_plan_title: "Maximum performance power mode",
    tech_plan_desc: "Switches Windows to its high performance mode while you play.",
    tech_background_title: "Calm down other apps (optional)",
    tech_background_desc: "Apps in the background, like browsers and chat, get less processor and disk \
                           time. Voice chat or downloads may slow down while you play.",
    tech_memory_title: "Free memory before playing",
    tech_memory_desc: "Frees memory from other apps and the cache, without touching the game.",
    tech_cores_title: "Use the fastest cores (optional)",
    tech_cores_desc: "On processors with fast and efficient cores, suggests the fast ones for the game.",
    tech_keep_title: "Keep the game's memory",
    tech_keep_desc: "Always on: automatic cleanups leave the game alone while it is boosted.",
    boost_button: "Boost game",
    boost_busy: "Boosting…",
    restore_button: "Restore",
    boost_result: "Result",
    boost_result_empty: "Press Boost game to start. You can undo everything at any time.",
    boost_note: "Changes to these options apply the next time you boost.",
    outcome_boost_applied: "Done",
    outcome_boost_already: "Already on",
    outcome_boost_failed: "Windows did not allow it for this game",
    outcome_boost_not_available: "Not available on this computer",
    outcome_boost_calmed: |n| format!("{n} apps calmed down"),
    outcome_boost_freed: |size| format!("{size} freed"),
    boost_done: "Game boosted",
    boost_done_body: |name| format!("{name} now comes first. Everything goes back to normal when it closes."),
    boost_gone: "Game not found",
    boost_gone_body: "It may have closed. Choose it again in the list.",
    boost_restored: "Back to normal",
    boost_restored_body: "All Game Booster changes were undone.",
    boost_exited_body: |name| format!("{name} closed, so all Game Booster changes were undone."),

    nav_disk: "Disk",
    disk_sub: "See what fills your drives and free up space",
    disk_local: "Local Disk",
    disk_free_of: |free, total| format!("{free} free of {total}"),
    media_ssd: "SSD",
    media_hdd: "Hard disk",
    media_removable: "Removable",
    media_network: "Network",
    disk_refresh: "Refresh drives",
    tab_analyze: "Analyze",
    tab_clean: "Clean up",
    tab_optimize: "Optimize",
    count: |n| grouped(n, ','),

    analyze_button: "Analyze drive",
    analyze_again: "Analyze again",
    analyze_empty_title: "Find out what takes up space",
    analyze_empty_body: "Looks through the whole drive and lists the biggest folders and files. \
                         Nothing is changed or deleted: you can only open the folders.",
    analyze_reading: "Reading the drive…",
    analyze_progress: |files, size| format!("{files} files · {size}"),
    analyze_summary: |files, folders, secs| {
        format!("{files} files in {folders} folders · {}", duration_text(secs.round() as u64))
    },
    analyze_stopped: "Stopped early: showing what was found so far.",
    analyze_unread: |size| format!("{size} is used by Windows itself or by folders that could not be opened."),
    analyze_read_only: "Read only: nothing is changed or deleted here.",
    analyze_used: "used",
    view_folders: "Biggest folders",
    view_files: "Biggest files",
    view_types: "By type",
    open_folder: "Open folder",
    show_in_folder: "Show in folder",
    files_count: |n| format!("{n} files"),
    more_items: |n| format!("…and {n} smaller folders"),
    kinds: [
        "Videos",
        "Music",
        "Pictures",
        "Documents",
        "Archives and disk images",
        "Programs and game data",
        "Other",
    ],

    clean_title: "What can be removed",
    clean_sub: "Only files that are safe to remove. Windows and your apps make them again when needed.",
    clean_check: "Check again",
    clean_checking: "Checking…",
    clean_cleaning: "Cleaning…",
    clean_button: |size| format!("Clean up {size}"),
    clean_nothing_selected: "Choose what to clean",
    clean_total: |size| format!("{size} can be freed"),
    clean_total_sub: "In the items you chose",
    cat_titles: [
        "Your temporary files",
        "Windows temporary files",
        "Browser cache",
        "App crash reports",
        "Windows Update leftovers",
        "Update downloads",
        "Windows error reports",
        "Picture previews",
        "System crash files",
        "Setup and upgrade records",
        "Game graphics cache",
        "Recycle Bin",
        "Previous Windows installation",
        "Old device drivers",
    ],
    cat_descs: [
        "Leftovers that apps saved for a moment and did not remove. Only files older than one day.",
        "Leftovers from Windows and installers. Only files older than one day.",
        "Copies of web pages and pictures kept to open sites faster. History, passwords and \
         sign-ins are kept.",
        "Files that apps saved when they closed unexpectedly.",
        "Old copies of system files that updates replaced.",
        "Updates Windows already installed and kept to share. Windows can download them again.",
        "Problem reports that were already sent or are no longer needed.",
        "Small previews of photos and videos. File Explorer makes them again when needed.",
        "Memory copies Windows saved after a blue screen.",
        "Records from installing or upgrading Windows.",
        "Graphics prepared ahead of time for games. A game may stutter for a moment the first \
         time while it makes them again.",
        "Files you deleted on this drive. They cannot be brought back after this.",
        "Lets you go back to the Windows version you had before. After this you cannot go back.",
        "Older versions of the drivers of your devices. After this you cannot roll a driver back.",
    ],
    cat_careful: "Check first",
    cat_none: "Nothing to remove",
    cat_blocked: |app| format!("Close {app} to clean its cache"),
    cat_unavailable: "Not on this computer",
    cat_failed: "Windows did not allow it",
    cat_removed: |size| format!("{size} removed"),
    cat_in_use: |n| format!("{n} in use, left in place"),
    clean_only_bin: |letter| {
        format!(
            "Junk files from Windows and apps are on the Windows drive ({letter}:). \
             On this drive only the Recycle Bin can be emptied."
        )
    },
    clean_done: |size| format!("Freed {size} of disk space"),
    clean_done_body: |before, after| format!("Free space went from {before} to {after}."),
    clean_nothing: "Nothing was removed",
    clean_nothing_body: "The files were in use or already gone.",
    clean_stopped: "Cleanup stopped",
    clean_warning: "Items marked Check first cannot be undone.",

    optimize_sub: "Uses the tools built into Windows",
    opt_drive_title: "Optimize drive",
    opt_drive_ssd: "Tells the SSD which space is free (TRIM), so it stays fast. Safe to run at any time.",
    opt_drive_hdd: "Puts the pieces of each file back together, so the hard disk reads them faster. \
                    It can take a while; you can keep working.",
    opt_drive_unknown: "Windows picks the right job for this drive.",
    opt_drive_button: "Optimize",
    opt_trim_title: "Free-space notices for SSDs (TRIM)",
    opt_trim_on: "On: Windows tells SSDs when space is freed.",
    opt_trim_off: "Off: turning it on keeps SSDs fast.",
    opt_trim_unknown: "Could not be checked.",
    opt_trim_button: "Turn on",
    opt_hiber_title: "Hibernate file",
    opt_hiber_desc: |size| format!("Windows keeps {size} on this drive for Hibernate and Fast Startup."),
    opt_hiber_none: "Hibernate is off, so no space is kept for it.",
    opt_hiber_smaller: "Make smaller",
    opt_hiber_disable: "Turn off",
    opt_hiber_note: "Make smaller keeps Fast Startup. Turn off also removes Hibernate and Fast Startup.",
    opt_components_title: "Old Windows parts",
    opt_components_desc: "Removes old versions of Windows parts that updates replaced. It takes several \
                          minutes and cannot be stopped once it starts. Updates can still be removed later.",
    opt_components_button: "Clean up",
    opt_working: "Working…",
    opt_done: "Done",
    opt_failed: "Windows could not finish it",
    opt_stopped: "Stopped",
    opt_not_supported: "Network drives can only be analyzed.",

    tray_open: "Open OpenMemoryOptimizer",
    tray_exit: "Exit",
};

pub static PT_BR: Strings = Strings {
    decimal_sep: ",",

    admin_mode: "Modo administrador",
    limited_mode: "Modo limitado",
    nav_overview: "Visão geral",
    nav_settings: "Configurações",
    nav_schedule: "Agendamento",
    nav_about: "Sobre",
    auto_off: "Sem limpeza automática",
    auto_below: |pct| format!("Limpa abaixo de {pct}% livre"),
    auto_every: |min| format!("Limpa a cada {min} min"),

    overview_sub: "A memória do seu computador, atualizada ao vivo",
    checking_memory: "Verificando a memória…",
    gauge_in_use: "em uso",
    stop: "Parar",
    stopping: "Parando…",
    free_up_memory: "Liberar memória",
    status_healthy: "Saudável",
    status_healthy_sub: "bastante memória livre",
    status_busy: "Ficando cheia",
    status_busy_sub: "a memória está enchendo",
    status_low: "Pouca memória",
    status_low_sub: "tente liberar memória",
    total_memory: "Memória total",
    total_memory_sub: "instalada neste PC",
    in_use: "Em uso",
    in_use_sub: |pct| format!("{pct}% do total"),
    available: "Disponível",
    available_sub: "pronta para seus apps",
    status: "Situação",
    cached: "Em cache",
    cached_sub: "mantida pronta pelo Windows",
    cached_unavailable: "indisponível neste PC",
    processor: "Processador",
    cpu_resting: "em repouso",
    cpu_working: "trabalhando",
    memory_use: "Uso de memória",
    last_2_minutes: "últimos 2 minutos",

    last_cleanup: "Última limpeza",
    no_cleanup_yet: "Nenhuma limpeza ainda. Clique em “Liberar memória” para começar.",
    mode_auto: "automática",
    mode_manual: "manual",
    result_meta: |at, mode, secs| format!("{at} · {mode} · levou {secs:.0} s"),
    step_clipboard: "Área de transferência",
    step_deep_clean: "Limpeza profunda",
    step_open_apps: "Apps abertos",
    step_cached_memory: "Memória em cache",
    step_system_cache: "Cache do sistema",
    step_result: "Resultado",
    outcome_cleared: "Limpeza concluída",
    outcome_clear_failed: "Não foi possível limpar",
    outcome_done: "Concluída",
    outcome_partly_done: "Concluída em parte",
    outcome_apps_trimmed: |n| format!("{n} apps liberaram memória"),
    outcome_unreachable: "Não foi possível acessá-los",
    outcome_needs_admin: "Requer direitos de administrador",
    outcome_unsupported: "Não disponível neste PC",
    outcome_skipped_limits: "Ignorado (há limites personalizados)",

    progress_preparing: "Preparando…",
    progress_auto: "Limpeza automática…",
    progress_clipboard: "Limpando a área de transferência…",
    progress_defrag: "Limpeza profunda… isso pode demorar",
    progress_trim: "Liberando memória dos apps abertos…",
    progress_standby: "Limpando a memória em cache…",
    progress_file_cache: "Arrumando o cache do sistema…",

    save_failed: "Não foi possível salvar as configurações",
    nothing_to_clean: "Nada para limpar",
    nothing_to_clean_body: "Ative pelo menos uma opção de limpeza em Configurações.",
    already_running: "Já em andamento",
    already_running_body: "Uma limpeza já está em andamento.",
    cleanup_stopped: "Limpeza interrompida",
    freed_before_stop: |size| format!("{size} liberados antes de parar."),
    nothing_freed: "Nenhuma memória extra foi liberada",
    nothing_freed_body: "Nenhuma memória extra foi liberada.",
    freed: |size| format!("{size} de memória liberados"),
    available_change: |before, after, secs| {
        format!("Memória disponível: {before} → {after} (levou {secs:.0} s)")
    },

    settings_sub: "As alterações são salvas automaticamente",
    what_to_clean: "O que limpar",
    what_to_clean_sub: "Usado ao clicar em “Liberar memória”",
    trim_title: "Liberar memória dos apps abertos",
    trim_desc: "Pede aos apps em execução que devolvam a memória que não estão usando agora.",
    standby_title: "Limpar a memória em cache",
    standby_desc: "Remove arquivos que o Windows mantém na memória para acesso rápido. Requer direitos de administrador.",
    defrag_title: "Limpeza profunda",
    defrag_desc: "Usa por um instante até 1 GB de memória para tirar dados sem uso. Pode levar alguns minutos.",
    clipboard_title: "Limpar a área de transferência",
    clipboard_desc: "Apaga o que você copiou por último.",
    auto_title: "Limpeza automática",
    auto_sub: "Uma limpeza leve que roda em segundo plano",
    auto_toggle: "Limpar automaticamente",
    auto_toggle_desc: "Continua funcionando enquanto o app está na bandeja do sistema.",
    auto_low_memory: "Com pouca memória",
    auto_timer: "Em intervalos",
    clean_below: "Limpar quando a memória livre ficar abaixo de",
    idle_title: "Só quando o computador estiver em repouso",
    idle_desc: "Espera o computador ficar livre. Verificado a cada 5 minutos.",
    clean_every: "Limpar a cada",
    general: "Geral",
    notice_title: "Mostrar uma notificação",
    notice_desc: "Informa quanta memória foi liberada após cada limpeza.",
    tray_title: "Manter na bandeja do sistema",
    tray_desc: "Minimizar ou fechar a janela a esconde ao lado do relógio. Clique no ícone da bandeja para trazê-la de volta, ou clique com o botão direito para sair.",
    language: "Idioma",
    language_desc: "Automático usa o idioma do Windows.",
    language_auto: "Automático",

    schedule_sub: "Limpe automaticamente em horários definidos, mesmo com o app fechado",
    when_to_clean: "Quando limpar",
    sched_daily: "Diário",
    sched_weekly: "Semanal",
    sched_monthly: "Mensal",
    sched_idle: "Quando ocioso",
    sched_logon: "Ao entrar",
    sched_time: "Horário",
    weekdays: ["Seg", "Ter", "Qua", "Qui", "Sex", "Sáb", "Dom"],
    sched_month_day: "Dia do mês",
    sched_idle_desc: "Roda depois que o computador fica 10 minutos sem uso.",
    sched_logon_desc: "Roda sempre que você entra no Windows.",
    create_schedule: "Criar agendamento",
    remove_schedule: "Remover agendamento",
    sched_created: "Agendamento criado.",
    sched_create_failed: "Não foi possível criar o agendamento. Tente abrir o app como administrador.",
    sched_removed: "Agendamento removido.",
    sched_nothing_to_remove: "Não há agendamento para remover.",

    version_line: |v| format!("Versão {v} · um otimizador de memória simples para Windows"),
    what_happens: "O que acontece durante uma limpeza",
    about_clipboard: "Apaga o que você copiou por último (se ativado).",
    about_deep_clean: "Usa por um instante a memória livre para tirar dados sem uso (se ativado).",
    about_open_apps: "Pede a cada app em execução que devolva a memória que não está usando.",
    about_cached_memory: "Remove arquivos que o Windows manteve na memória para acesso rápido (se ativado).",
    about_system_cache: "Reduz a memória que o Windows usa para abrir arquivos mais rápido.",
    about_result: "Espera alguns segundos e mostra quanta memória ficou disponível.",
    good_to_know: "Bom saber",
    good_to_know_body: "O OpenMemoryOptimizer sempre mostra a quantidade real de memória liberada. A memória liberada \
                        volta a ser ocupada conforme você usa seus apps, e isso é normal. O Windows já gerencia bem \
                        a memória sozinho, então uma limpeza ajuda mais quando o computador está com pouca memória.",
    license: "Licença",
    license_body: "Copyright © 2026 Lucian Caetano. O OpenMemoryOptimizer é software livre: você pode \
                   compartilhá-lo e modificá-lo sob a Licença Pública Geral GNU, versão 3 ou posterior. \
                   Ele não vem com nenhuma garantia.",
    read_gpl: "Ler a GNU GPL v3",

    nav_booster: "Game Booster",
    booster_sub: "Dê ao jogo mais do seu computador enquanto você joga",
    booster_pick: "Escolha um jogo…",
    booster_search: "Buscar pelo nome",
    booster_open_apps: "Apps abertos",
    booster_other_processes: "Tudo o que está em execução",
    booster_no_match: "Nada corresponde à sua busca.",
    booster_refresh: "Atualizar lista",
    booster_empty_title: "Escolha o jogo que você quer acelerar",
    booster_empty_body: "Abra o jogo e escolha-o na lista acima. Você verá quanto do computador ele usa \
                         e poderá acelerá-lo com um clique.",
    booster_on: "Acelerado",
    booster_off: "Não acelerado",
    booster_closed: "Fechado",
    booster_closed_hint: "O jogo foi fechado. Abra-o de novo e ele será escolhido aqui sozinho.",
    booster_process_line: |name, pid| format!("{name} · ID {pid}"),
    tile_cpu: "Processador",
    tile_cpu_sub: "do processador inteiro",
    tile_memory: "Memória",
    tile_memory_sub: |private| format!("{private} só deste jogo"),
    tile_gpu: "Gráficos",
    tile_gpu_sub: "placa de vídeo em uso",
    tile_vram: "Memória de vídeo",
    tile_vram_sub: "na placa de vídeo",
    tile_unavailable: "não disponível",
    tile_disk: "Disco",
    tile_disk_sub: |read, write| format!("lendo {read} · gravando {write}"),
    tile_uptime: "Jogando há",
    tile_uptime_sub: |threads| format!("{threads} tarefas em execução"),
    duration: duration_text,
    per_second: "/s",
    boost_options: "O que o Game Booster faz",
    boost_options_sub: "Tudo é desfeito quando você restaura ou fecha o jogo",
    booster_safe: "Nunca lê nem altera a memória ou os arquivos do jogo e não mexe no anti-cheat. \
                   Só muda como o Windows divide o computador, como o Gerenciador de Tarefas faz.",
    tech_priority_title: "Colocar o jogo em primeiro lugar",
    tech_priority_desc: "O processador atende o jogo antes dos outros apps.",
    tech_power_title: "Nunca deixar o jogo lento para economizar energia",
    tech_power_desc: "O Windows mantém o jogo nas configurações mais rápidas, mesmo na bateria.",
    tech_plan_title: "Modo de energia de desempenho máximo",
    tech_plan_desc: "Coloca o Windows no modo de alto desempenho enquanto você joga.",
    tech_background_title: "Acalmar os outros apps (opcional)",
    tech_background_desc: "Apps em segundo plano, como navegadores e chat, recebem menos tempo de \
                           processador e disco. Chat de voz ou downloads podem ficar mais lentos enquanto \
                           você joga.",
    tech_memory_title: "Liberar memória antes de jogar",
    tech_memory_desc: "Libera memória dos outros apps e do cache, sem mexer no jogo.",
    tech_cores_title: "Usar os núcleos mais rápidos (opcional)",
    tech_cores_desc: "Em processadores com núcleos rápidos e econômicos, sugere os rápidos para o jogo.",
    tech_keep_title: "Manter a memória do jogo",
    tech_keep_desc: "Sempre ativo: as limpezas automáticas não mexem no jogo enquanto ele está acelerado.",
    boost_button: "Acelerar jogo",
    boost_busy: "Acelerando…",
    restore_button: "Restaurar",
    boost_result: "Resultado",
    boost_result_empty: "Toque em Acelerar jogo para começar. Você pode desfazer tudo a qualquer momento.",
    boost_note: "Mudanças nestas opções valem na próxima vez que você acelerar.",
    outcome_boost_applied: "Feito",
    outcome_boost_already: "Já estava ativo",
    outcome_boost_failed: "O Windows não permitiu para este jogo",
    outcome_boost_not_available: "Não disponível neste computador",
    outcome_boost_calmed: |n| format!("{n} apps acalmados"),
    outcome_boost_freed: |size| format!("{size} liberados"),
    boost_done: "Jogo acelerado",
    boost_done_body: |name| format!("{name} agora vem primeiro. Tudo volta ao normal quando ele fechar."),
    boost_gone: "Jogo não encontrado",
    boost_gone_body: "Talvez ele tenha fechado. Escolha-o de novo na lista.",
    boost_restored: "Tudo voltou ao normal",
    boost_restored_body: "Todas as mudanças do Game Booster foram desfeitas.",
    boost_exited_body: |name| format!("{name} fechou, então todas as mudanças do Game Booster foram desfeitas."),

    nav_disk: "Disco",
    disk_sub: "Veja o que ocupa seus discos e libere espaço",
    disk_local: "Disco Local",
    disk_free_of: |free, total| format!("{free} livres de {total}"),
    media_ssd: "SSD",
    media_hdd: "HD",
    media_removable: "Removível",
    media_network: "Rede",
    disk_refresh: "Atualizar discos",
    tab_analyze: "Analisar",
    tab_clean: "Limpar",
    tab_optimize: "Otimizar",
    count: |n| grouped(n, '.'),

    analyze_button: "Analisar disco",
    analyze_again: "Analisar de novo",
    analyze_empty_title: "Descubra o que ocupa espaço",
    analyze_empty_body: "Percorre o disco inteiro e lista as maiores pastas e arquivos. \
                         Nada é alterado nem apagado: você só pode abrir as pastas.",
    analyze_reading: "Lendo o disco…",
    analyze_progress: |files, size| format!("{files} arquivos · {size}"),
    analyze_summary: |files, folders, secs| {
        format!("{files} arquivos em {folders} pastas · {}", duration_text(secs.round() as u64))
    },
    analyze_stopped: "Parado antes do fim: mostrando o que foi encontrado até agora.",
    analyze_unread: |size| format!("{size} são usados pelo próprio Windows ou por pastas que não puderam ser abertas."),
    analyze_read_only: "Somente leitura: nada é alterado nem apagado aqui.",
    analyze_used: "usados",
    view_folders: "Maiores pastas",
    view_files: "Maiores arquivos",
    view_types: "Por tipo",
    open_folder: "Abrir pasta",
    show_in_folder: "Mostrar na pasta",
    files_count: |n| format!("{n} arquivos"),
    more_items: |n| format!("…e mais {n} pastas menores"),
    kinds: [
        "Vídeos",
        "Músicas",
        "Imagens",
        "Documentos",
        "Compactados e imagens de disco",
        "Programas e dados de jogos",
        "Outros",
    ],

    clean_title: "O que pode ser removido",
    clean_sub: "Só arquivos que podem ser removidos com segurança. O Windows e seus aplicativos os criam de novo quando precisam.",
    clean_check: "Verificar de novo",
    clean_checking: "Verificando…",
    clean_cleaning: "Limpando…",
    clean_button: |size| format!("Limpar {size}"),
    clean_nothing_selected: "Escolha o que limpar",
    clean_total: |size| format!("{size} podem ser liberados"),
    clean_total_sub: "Nos itens que você escolheu",
    cat_titles: [
        "Seus arquivos temporários",
        "Arquivos temporários do Windows",
        "Cache dos navegadores",
        "Relatórios de falha de aplicativos",
        "Sobras do Windows Update",
        "Downloads de atualizações",
        "Relatórios de erro do Windows",
        "Miniaturas de imagens",
        "Arquivos de falha do sistema",
        "Registros de instalação e atualização",
        "Cache de gráficos dos jogos",
        "Lixeira",
        "Instalação anterior do Windows",
        "Drivers antigos",
    ],
    cat_descs: [
        "Sobras que os aplicativos guardaram por um momento e não apagaram. Só arquivos com mais de um dia.",
        "Sobras do Windows e de instaladores. Só arquivos com mais de um dia.",
        "Cópias de páginas e imagens guardadas para abrir sites mais rápido. Histórico, senhas e \
         logins são mantidos.",
        "Arquivos que os aplicativos salvaram quando fecharam de repente.",
        "Cópias antigas de arquivos do sistema que as atualizações substituíram.",
        "Atualizações que o Windows já instalou e guardou para compartilhar. Ele pode baixá-las de novo.",
        "Relatórios de problemas já enviados ou que não são mais necessários.",
        "Pequenas prévias de fotos e vídeos. O Explorador de Arquivos as cria de novo quando precisa.",
        "Cópias da memória que o Windows salvou depois de uma tela azul.",
        "Registros da instalação ou atualização do Windows.",
        "Gráficos preparados com antecedência para os jogos. Um jogo pode travar um pouco na \
         primeira vez enquanto os refaz.",
        "Arquivos que você apagou neste disco. Depois disso, não dá para recuperá-los.",
        "Permite voltar para a versão do Windows que você tinha antes. Depois disso, não dá para voltar.",
        "Versões antigas dos drivers dos seus dispositivos. Depois disso, não dá para voltar um driver.",
    ],
    cat_careful: "Confira antes",
    cat_none: "Nada para remover",
    cat_blocked: |app| format!("Feche o {app} para limpar o cache dele"),
    cat_unavailable: "Não existe neste computador",
    cat_failed: "O Windows não permitiu",
    cat_removed: |size| format!("{size} removidos"),
    cat_in_use: |n| format!("{n} em uso, mantidos"),
    clean_only_bin: |letter| {
        format!(
            "Os arquivos inúteis do Windows e dos aplicativos ficam no disco do Windows ({letter}:). \
             Neste disco só dá para esvaziar a Lixeira."
        )
    },
    clean_done: |size| format!("{size} de espaço liberados"),
    clean_done_body: |before, after| format!("O espaço livre foi de {before} para {after}."),
    clean_nothing: "Nada foi removido",
    clean_nothing_body: "Os arquivos estavam em uso ou já tinham sido apagados.",
    clean_stopped: "Limpeza interrompida",
    clean_warning: "Itens marcados com Confira antes não podem ser desfeitos.",

    optimize_sub: "Usa as ferramentas que já vêm no Windows",
    opt_drive_title: "Otimizar disco",
    opt_drive_ssd: "Avisa o SSD sobre o espaço livre (TRIM) para ele continuar rápido. Pode ser usado a qualquer hora.",
    opt_drive_hdd: "Junta os pedaços de cada arquivo para o HD lê-los mais rápido. Pode demorar; \
                    você pode continuar usando o computador.",
    opt_drive_unknown: "O Windows escolhe o trabalho certo para este disco.",
    opt_drive_button: "Otimizar",
    opt_trim_title: "Avisos de espaço livre para SSDs (TRIM)",
    opt_trim_on: "Ativado: o Windows avisa os SSDs quando o espaço é liberado.",
    opt_trim_off: "Desativado: ativar mantém os SSDs rápidos.",
    opt_trim_unknown: "Não foi possível verificar.",
    opt_trim_button: "Ativar",
    opt_hiber_title: "Arquivo de hibernação",
    opt_hiber_desc: |size| format!("O Windows reserva {size} neste disco para a Hibernação e a Inicialização Rápida."),
    opt_hiber_none: "A hibernação está desativada, então nenhum espaço é reservado para ela.",
    opt_hiber_smaller: "Reduzir",
    opt_hiber_disable: "Desativar",
    opt_hiber_note: "Reduzir mantém a Inicialização Rápida. Desativar remove a Hibernação e a Inicialização Rápida.",
    opt_components_title: "Partes antigas do Windows",
    opt_components_desc: "Remove versões antigas de partes do Windows que as atualizações substituíram. Leva \
                          vários minutos e não pode ser interrompido depois de começar. As atualizações ainda \
                          podem ser removidas depois.",
    opt_components_button: "Limpar",
    opt_working: "Trabalhando…",
    opt_done: "Concluído",
    opt_failed: "O Windows não conseguiu terminar",
    opt_stopped: "Interrompido",
    opt_not_supported: "Discos de rede só podem ser analisados.",

    tray_open: "Abrir o OpenMemoryOptimizer",
    tray_exit: "Sair",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_roundtrip_and_accept_variants() {
        for lang in [Lang::En, Lang::PtBr] {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
        assert_eq!(Lang::from_code(" PT_br "), Some(Lang::PtBr));
        assert_eq!(Lang::from_code("pt-PT"), Some(Lang::PtBr));
        assert_eq!(Lang::from_code("en-GB"), Some(Lang::En));
        assert_eq!(Lang::from_code("auto"), None);
        assert_eq!(Lang::from_code(""), None);
    }

    #[test]
    fn langid_picks_portuguese_for_any_region() {
        assert_eq!(Lang::from_langid(0x0416), Lang::PtBr); // pt-BR
        assert_eq!(Lang::from_langid(0x0816), Lang::PtBr); // pt-PT
        assert_eq!(Lang::from_langid(0x0409), Lang::En); // en-US
        assert_eq!(Lang::from_langid(0x0407), Lang::En); // de-DE falls back to English
    }

    #[test]
    fn sizes_use_the_language_decimal_separator() {
        assert_eq!(EN.size(3 * 1024 * 1024 / 2), "1.50 GB");
        assert_eq!(PT_BR.size(3 * 1024 * 1024 / 2), "1,50 GB");
        assert_eq!(PT_BR.size(512), "512 KB");
        assert_eq!(PT_BR.decimal(format!("{:.1}", 42.5)), "42,5");
    }

    #[test]
    fn texts_with_values_include_them() {
        for t in [&EN, &PT_BR] {
            assert!((t.auto_below)(30).contains("30%"));
            assert!((t.auto_every)(25).contains("25 min"));
            assert!((t.outcome_apps_trimmed)(189).contains("189"));
            assert!((t.freed)("1,23 GB").contains("1,23 GB"));
            let change = (t.available_change)("1 GB", "2 GB", 3.4);
            assert!(change.contains("1 GB") && change.contains("2 GB") && change.contains("3 s"));
            assert!((t.version_line)("9.9.9").contains("9.9.9"));
            assert!((t.result_meta)("12:34", t.mode_auto, 3.0).contains("12:34"));
            assert!((t.booster_process_line)("game.exe", 42).contains("game.exe"));
            assert!((t.tile_disk_sub)("1 MB/s", "2 MB/s").contains("2 MB/s"));
            assert!((t.outcome_boost_calmed)(12).contains("12"));
            assert!((t.boost_exited_body)("My Game").contains("My Game"));
            assert_eq!(t.nav_booster, "Game Booster");
        }
    }

    #[test]
    fn disk_texts() {
        assert_eq!((EN.count)(1_234_567), "1,234,567");
        assert_eq!((PT_BR.count)(1_234_567), "1.234.567");
        assert_eq!((EN.count)(999), "999");
        assert_eq!((EN.count)(0), "0");
        for t in [&EN, &PT_BR] {
            assert!((t.clean_only_bin)('C').contains("C:"));
            assert!((t.cat_blocked)("Firefox").contains("Firefox"));
            assert!((t.analyze_summary)("10", "2", 75.0).contains("1 min"));
            assert!(t
                .cat_titles
                .iter()
                .chain(&t.cat_descs)
                .all(|s| !s.is_empty()));
        }
    }

    #[test]
    fn durations() {
        assert_eq!(duration_text(45), "45 s");
        assert_eq!(duration_text(12 * 60 + 5), "12 min");
        assert_eq!(duration_text(3600 + 5 * 60), "1 h 05 min");
    }
}
