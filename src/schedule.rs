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

//! Scheduled task creation, port of the schtasks builder at 0xc78d50..0xc790c0:
//!   schtasks /create /tn "<name>" /tr "<exe>" /RL HIGHEST <trigger>

use std::path::Path;
use std::process::Command;

pub const TASK_NAME: &str = "OpenMemoryOptimizer Task";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// `/sc daily /st HH:MM`
    Daily(String),
    /// `/sc weekly /d Mon` ... `/d Sun`
    Weekly(String),
    /// `/sc monthly /mo 1 /d N`
    Monthly(u8),
    /// `/sc onidle /i 10`
    OnIdle,
    /// `/sc onlogon`
    OnLogon,
}

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

impl Trigger {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let arg = |i: usize| args.get(i).map(String::as_str);
        match arg(0) {
            Some("daily") => {
                let t = arg(1).ok_or("daily needs a time, e.g. daily 03:00")?;
                let valid = t.split_once(':').is_some_and(|(h, m)| {
                    h.parse::<u8>().is_ok_and(|h| h < 24)
                        && m.len() == 2
                        && m.parse::<u8>().is_ok_and(|m| m < 60)
                });
                valid
                    .then(|| Trigger::Daily(t.to_string()))
                    .ok_or_else(|| format!("bad time {t:?}"))
            }
            Some("weekly") => {
                let d = arg(1).ok_or("weekly needs a day, e.g. weekly Mon")?;
                DAYS.iter()
                    .find(|day| day.eq_ignore_ascii_case(d))
                    .map(|day| Trigger::Weekly(day.to_string()))
                    .ok_or_else(|| format!("bad day {d:?}, use one of {DAYS:?}"))
            }
            Some("monthly") => {
                let d = arg(1).ok_or("monthly needs a day of month, e.g. monthly 1")?;
                d.parse::<u8>()
                    .ok()
                    .filter(|d| (1..=31).contains(d))
                    .map(Trigger::Monthly)
                    .ok_or_else(|| format!("bad day of month {d:?}"))
            }
            Some("idle") => Ok(Trigger::OnIdle),
            Some("logon") => Ok(Trigger::OnLogon),
            _ => Err(
                "trigger must be one of: daily HH:MM | weekly DAY | monthly N | idle | logon"
                    .into(),
            ),
        }
    }

    fn args(&self) -> Vec<String> {
        let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match self {
            Trigger::Daily(t) => v(&["/sc", "daily", "/st", t]),
            Trigger::Weekly(d) => v(&["/sc", "weekly", "/d", d]),
            Trigger::Monthly(d) => v(&["/sc", "monthly", "/mo", "1", "/d", &d.to_string()]),
            Trigger::OnIdle => v(&["/sc", "onidle", "/i", "10"]),
            Trigger::OnLogon => v(&["/sc", "onlogon"]),
        }
    }
}

/// The original's `/tr` argument is assembled at runtime and was not recovered,
/// so the task starts this program with `--optimize`: one silent manual-mode pass.
/// `/f` is added so re-scheduling does not block on schtasks' overwrite prompt.
pub fn create_args(exe: &Path, trigger: &Trigger) -> Vec<String> {
    let mut args = vec![
        "/create".to_string(),
        "/tn".to_string(),
        TASK_NAME.to_string(),
        "/tr".to_string(),
        format!("\"{}\" --optimize", exe.display()),
        "/RL".to_string(),
        "HIGHEST".to_string(),
    ];
    args.extend(trigger.args());
    args.push("/f".to_string());
    args
}

pub fn create_command(exe: &Path, trigger: &Trigger) -> Command {
    let mut cmd = Command::new("schtasks");
    cmd.args(create_args(exe, trigger));
    cmd
}

pub fn delete_command() -> Command {
    let mut cmd = Command::new("schtasks");
    cmd.args(["/delete", "/tn", TASK_NAME, "/f"]);
    cmd
}

pub fn create(exe: &Path, trigger: &Trigger) -> std::io::Result<std::process::ExitStatus> {
    create_command(exe, trigger).status()
}

pub fn delete() -> std::io::Result<std::process::ExitStatus> {
    delete_command().status()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_triggers() {
        assert_eq!(
            Trigger::parse(&s(&["daily", "03:30"])),
            Ok(Trigger::Daily("03:30".into()))
        );
        assert_eq!(
            Trigger::parse(&s(&["weekly", "fri"])),
            Ok(Trigger::Weekly("Fri".into()))
        );
        assert_eq!(
            Trigger::parse(&s(&["monthly", "15"])),
            Ok(Trigger::Monthly(15))
        );
        assert_eq!(Trigger::parse(&s(&["idle"])), Ok(Trigger::OnIdle));
        assert!(Trigger::parse(&s(&["daily", "25:00"])).is_err());
        assert!(Trigger::parse(&s(&["monthly", "0"])).is_err());
    }

    #[test]
    fn builds_original_command_shape() {
        let a = create_args(Path::new(r"C:\x\OpenMemoryOptimizer.exe"), &Trigger::OnIdle);
        assert_eq!(
            a.join(" "),
            format!(
                r#"/create /tn {TASK_NAME} /tr "C:\x\OpenMemoryOptimizer.exe" --optimize /RL HIGHEST /sc onidle /i 10 /f"#
            )
        );
    }
}
