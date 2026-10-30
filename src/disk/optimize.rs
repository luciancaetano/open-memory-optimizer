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

//! Drive optimization with the tools Windows ships:
//! - `defrag X: /O` lets the Windows optimizer pick the right job for the drive:
//!   TRIM for an SSD, defragmentation for a hard disk. Nothing of our own touches
//!   the file system layout.
//! - `fsutil behavior set DisableDeleteNotify 0` turns TRIM notifications back on.
//! - `powercfg /h /type reduced` or `/h off` shrinks or removes `hiberfil.sys`.
//! - `DISM /Online /Cleanup-Image /StartComponentCleanup` removes component versions
//!   replaced by updates. `/ResetBase` is never used, so updates stay removable.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    OptimizeDrive,
    EnableTrim,
    HibernateSmaller,
    HibernateOff,
    Components,
}

impl Action {
    /// Can be stopped halfway. DISM is not stopped once it starts servicing Windows.
    pub fn can_stop(self) -> bool {
        self == Action::OptimizeDrive
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed,
    Cancelled,
    NeedsAdmin,
}

fn system32(exe: &str) -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(exe)
}

/// Program and arguments for `action` on drive `letter`.
pub fn command_line(action: Action, letter: char) -> (&'static str, Vec<String>) {
    let drive = format!("{letter}:");
    let (exe, args): (&str, &[&str]) = match action {
        Action::OptimizeDrive => ("defrag.exe", &[&drive, "/O", "/U"]),
        Action::EnableTrim => (
            "fsutil.exe",
            &["behavior", "set", "DisableDeleteNotify", "0"],
        ),
        Action::HibernateSmaller => ("powercfg.exe", &["/h", "/type", "reduced"]),
        Action::HibernateOff => ("powercfg.exe", &["/h", "off"]),
        Action::Components => (
            "Dism.exe",
            &["/Online", "/Cleanup-Image", "/StartComponentCleanup"],
        ),
    };
    (exe, args.iter().map(|s| s.to_string()).collect())
}

/// The last percentage in tool output: "Retrim: 42% complete", "[=== 18.4% ===]" or,
/// in Portuguese Windows, "18,4%".
pub fn parse_percent(text: &str) -> Option<f32> {
    let end = text.rfind('%')?;
    let head = text[..end].trim_end();
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.' || *c == ',')
        .last()
        .map(|(i, _)| i)?;
    head[start..]
        .replace(',', ".")
        .parse::<f32>()
        .ok()
        .filter(|p| (0.0..=100.0).contains(p))
}

/// Runs `action`, reporting progress in percent when the tool prints it.
pub fn run(
    action: Action,
    letter: char,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Outcome {
    if !crate::privilege::is_elevated() {
        return Outcome::NeedsAdmin;
    }
    let (exe, args) = command_line(action, letter);
    let child = Command::new(system32(exe))
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    let Ok(mut child) = child else {
        return Outcome::Failed;
    };
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0
                || tx
                    .send(String::from_utf8_lossy(&buf[..n]).into_owned())
                    .is_err()
            {
                break;
            }
        }
    });
    let mut tail = String::new();
    let status = loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                tail.push_str(&chunk);
                // Keep only the last line or two; progress lines end with "\r".
                if tail.len() > 512 {
                    let mut cut = tail.len() - 256;
                    while !tail.is_char_boundary(cut) {
                        cut += 1;
                    }
                    tail.drain(..cut);
                }
                let last = tail
                    .rsplit(['\r', '\n'])
                    .find(|s| s.contains('%'))
                    .unwrap_or("");
                if let Some(p) = parse_percent(last) {
                    progress(p);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.wait(),
        }
        if action.can_stop() && cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Outcome::Cancelled;
        }
    };
    let _ = reader.join();
    match status {
        Ok(s) if s.success() => Outcome::Done,
        _ => Outcome::Failed,
    }
}

/// `fsutil behavior query DisableDeleteNotify`: "NTFS DisableDeleteNotify = 0 (Allowed)".
/// Returns whether TRIM notifications are on for NTFS.
pub fn parse_trim(output: &str) -> Option<bool> {
    let line = output
        .lines()
        .find(|l| l.trim_start().starts_with("NTFS DisableDeleteNotify"))
        .or_else(|| output.lines().find(|l| l.contains("DisableDeleteNotify")))?;
    let value = line.split_once('=')?.1.trim();
    let digit = value.chars().next()?.to_digit(10)?;
    Some(digit == 0)
}

/// Whether Windows tells SSDs about freed space. `None` if it cannot be read.
pub fn trim_enabled() -> Option<bool> {
    let out = Command::new(system32("fsutil.exe"))
        .args(["behavior", "query", "DisableDeleteNotify"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_trim(&String::from_utf8_lossy(&out.stdout))
}

/// Size of `X:\hiberfil.sys`, if hibernation keeps one on that drive.
pub fn hibernation_file(letter: char) -> Option<u64> {
    std::fs::metadata(format!("{letter}:\\hiberfil.sys"))
        .ok()
        .map(|m| m.len())
        .filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_in_any_language() {
        assert_eq!(parse_percent("Retrim:  42% complete..."), Some(42.0));
        assert_eq!(parse_percent("[=====      18.4%          ]"), Some(18.4));
        assert_eq!(parse_percent("Recorte: 18,5% concluído"), Some(18.5));
        assert_eq!(parse_percent("Done 100 %"), Some(100.0));
        assert_eq!(parse_percent("no number"), None);
        assert_eq!(parse_percent("%"), None);
    }

    #[test]
    fn trim_state() {
        let on = "NTFS DisableDeleteNotify = 0  (Allowed)\r\nReFS DisableDeleteNotify = 0  (Allowed)\r\n";
        assert_eq!(parse_trim(on), Some(true));
        let off = "NTFS DisableDeleteNotify = 1  (Disallowed)\r\nReFS DisableDeleteNotify = 0\r\n";
        assert_eq!(parse_trim(off), Some(false));
        assert_eq!(parse_trim("DisableDeleteNotify = 0"), Some(true));
        assert_eq!(parse_trim("Access denied"), None);
    }

    #[test]
    fn commands_never_use_reset_base_or_full_defrag_flags() {
        let (exe, args) = command_line(Action::OptimizeDrive, 'D');
        assert_eq!(exe, "defrag.exe");
        assert_eq!(args, vec!["D:", "/O", "/U"]);
        let (exe, args) = command_line(Action::Components, 'C');
        assert_eq!(exe, "Dism.exe");
        assert!(!args.iter().any(|a| a.eq_ignore_ascii_case("/ResetBase")));
        assert_eq!(command_line(Action::HibernateOff, 'C').1, vec!["/h", "off"]);
        assert!(Action::OptimizeDrive.can_stop() && !Action::Components.can_stop());
    }
}
