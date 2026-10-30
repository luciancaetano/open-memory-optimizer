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

//! INI settings, same section/key names and defaults as the original loader
//! (0xc90e30..0xc91060, TIniFile.ReadBool/ReadInteger on [MemoryOptimizer]).

use std::path::Path;

pub const SECTION: &str = "MemoryOptimizer";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// AutoOpt (default 1): master switch for the 60-second auto timer.
    pub auto_opt: bool,
    /// LowmemOpt (default 0): optimize when free RAM drops below `low_mem_percent`.
    pub lowmem_opt: bool,
    /// Idle (default 1): low-memory trigger additionally requires CPU <= 5 %.
    pub idle_only: bool,
    /// LowMem (default 30): threshold in percent; values outside 0..=100 become 30.
    pub low_mem_percent: i64,
    /// periodOpt (default 0): optimize every `period_minutes`.
    pub period_opt: bool,
    /// periodMinute (default 25).
    pub period_minutes: i64,
    /// ClearBoard (default 1).
    pub clear_clipboard: bool,
    /// ShowNotice (default 1).
    pub show_notice: bool,
    /// DefragMem (default 0).
    pub defrag_mem: bool,
    /// TrimWorkingSets (default 1). Not in the original, which never stores it and
    /// forces it on when loading settings (0xc91029).
    pub trim_working_sets: bool,
    /// ClearStandy (sic, default 0).
    pub clear_standby: bool,
    /// MinToTray (default 1): closing the GUI window hides it to the tray.
    pub min_to_tray: bool,
    /// Last choices on the Schedule page, so it shows them again on the next start.
    /// Not in the original. The task itself lives in Task Scheduler.
    pub schedule: ScheduleForm,
}

/// Schedule page fields: SchedKind (0 daily, 1 weekly, 2 monthly, 3 idle, 4 sign-in),
/// SchedHour, SchedMinute, SchedWeekday (0 = Monday) and SchedMonthDay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleForm {
    pub kind: u8,
    pub hour: u8,
    pub minute: u8,
    pub weekday: u8,
    pub month_day: u8,
}

impl Default for ScheduleForm {
    fn default() -> Self {
        Self {
            kind: 0,
            hour: 3,
            minute: 0,
            weekday: 0,
            month_day: 1,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_lookup(|_| None)
    }
}

impl Settings {
    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        // TIniFile.ReadInteger falls back to the default when the value does not parse.
        let int = |key: &str, default: i64| -> i64 {
            get(key)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        };
        let boolean = |key: &str, default: bool| int(key, default as i64) != 0;
        let ranged = |key: &str, range: std::ops::RangeInclusive<u8>, default: u8| -> u8 {
            u8::try_from(int(key, default as i64))
                .ok()
                .filter(|v| range.contains(v))
                .unwrap_or(default)
        };
        let form = ScheduleForm::default();

        let mut s = Settings {
            auto_opt: boolean("AutoOpt", true),
            lowmem_opt: boolean("LowmemOpt", false),
            idle_only: boolean("Idle", true),
            low_mem_percent: int("LowMem", 30),
            period_opt: boolean("periodOpt", false),
            period_minutes: int("periodMinute", 25),
            clear_clipboard: boolean("ClearBoard", true),
            show_notice: boolean("ShowNotice", true),
            defrag_mem: boolean("DefragMem", false),
            trim_working_sets: boolean("TrimWorkingSets", true),
            clear_standby: boolean("ClearStandy", false),
            min_to_tray: boolean("MinToTray", true),
            schedule: ScheduleForm {
                kind: ranged("SchedKind", 0..=4, form.kind),
                hour: ranged("SchedHour", 0..=23, form.hour),
                minute: ranged("SchedMinute", 0..=59, form.minute),
                weekday: ranged("SchedWeekday", 0..=6, form.weekday),
                month_day: ranged("SchedMonthDay", 1..=31, form.month_day),
            },
        };
        if !(0..=100).contains(&s.low_mem_percent) {
            s.low_mem_percent = 30;
        }
        // Exactly one auto trigger: if neither is chosen, low-memory mode is used.
        if !s.lowmem_opt && !s.period_opt {
            s.lowmem_opt = true;
        }
        s
    }

    pub fn parse(ini: &str) -> Self {
        let values = parse_section(ini, SECTION);
        Self::from_lookup(|key| {
            values
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.clone())
        })
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|s| Self::parse(&s))
            .unwrap_or_default()
    }

    pub fn to_ini(&self) -> String {
        let b = |v: bool| if v { "1" } else { "0" };
        format!(
            "[{SECTION}]\r\nAutoOpt={}\r\nLowmemOpt={}\r\nIdle={}\r\nLowMem={}\r\nperiodOpt={}\r\nperiodMinute={}\r\nClearBoard={}\r\nShowNotice={}\r\nDefragMem={}\r\nClearStandy={}\r\nMinToTray={}\r\nTrimWorkingSets={}\r\nSchedKind={}\r\nSchedHour={}\r\nSchedMinute={}\r\nSchedWeekday={}\r\nSchedMonthDay={}\r\n",
            b(self.auto_opt),
            b(self.lowmem_opt),
            b(self.idle_only),
            self.low_mem_percent,
            b(self.period_opt),
            self.period_minutes,
            b(self.clear_clipboard),
            b(self.show_notice),
            b(self.defrag_mem),
            b(self.clear_standby),
            b(self.min_to_tray),
            b(self.trim_working_sets),
            self.schedule.kind,
            self.schedule.hour,
            self.schedule.minute,
            self.schedule.weekday,
            self.schedule.month_day,
        )
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_ini())
    }
}

fn parse_section(ini: &str, section: &str) -> Vec<(String, String)> {
    let mut in_section = false;
    let mut out = Vec::new();
    for line in ini.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.trim().eq_ignore_ascii_case(section);
        } else if in_section {
            if let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_original() {
        let s = Settings::default();
        assert!(s.auto_opt && s.idle_only && s.clear_clipboard && s.show_notice);
        assert!(s.trim_working_sets);
        assert!(!s.defrag_mem && !s.clear_standby && !s.period_opt);
        // LowmemOpt defaults to 0 but is forced on because periodOpt is also 0.
        assert!(s.lowmem_opt);
        assert_eq!((s.low_mem_percent, s.period_minutes), (30, 25));
    }

    #[test]
    fn parses_keys_case_insensitively_and_clamps() {
        let s = Settings::parse(
            "[General]\nDarkMode=1\n[MemoryOptimizer]\nperiodopt=1\nperiodMinute=10\nLowMem=150\nCLEARSTANDY=1\nDefragMem=junk\n",
        );
        assert!(s.period_opt && !s.lowmem_opt);
        assert_eq!(s.period_minutes, 10);
        assert_eq!(s.low_mem_percent, 30);
        assert!(s.clear_standby);
        assert!(!s.defrag_mem); // unparsable -> default
    }

    #[test]
    fn roundtrip() {
        let s = Settings {
            defrag_mem: true,
            low_mem_percent: 42,
            trim_working_sets: false,
            schedule: ScheduleForm {
                kind: 1,
                hour: 22,
                minute: 45,
                weekday: 6,
                month_day: 31,
            },
            ..Settings::default()
        };
        assert_eq!(Settings::parse(&s.to_ini()), s);
    }

    #[test]
    fn files_from_the_original_keep_the_old_defaults() {
        // No TrimWorkingSets or Sched* keys: trimming stays on, schedule form is default.
        let s = Settings::parse("[MemoryOptimizer]\nAutoOpt=1\nClearStandy=1\n");
        assert!(s.trim_working_sets);
        assert_eq!(s.schedule, ScheduleForm::default());
    }

    #[test]
    fn out_of_range_schedule_values_fall_back() {
        let s = Settings::parse(
            "[MemoryOptimizer]\nSchedKind=9\nSchedHour=24\nSchedMinute=-1\nSchedWeekday=3\nSchedMonthDay=0\n",
        );
        let d = ScheduleForm::default();
        assert_eq!(
            (s.schedule.kind, s.schedule.hour, s.schedule.minute),
            (d.kind, d.hour, d.minute)
        );
        assert_eq!(s.schedule.weekday, 3);
        assert_eq!(s.schedule.month_day, d.month_day);
    }
}
