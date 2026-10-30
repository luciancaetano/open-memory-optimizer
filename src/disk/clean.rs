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

//! Junk cleanup in two steps: [`measure`] shows what a category would free, then
//! [`clean`] removes it.
//!
//! Three kinds of categories:
//! - Folders of our own list ([`Source::Files`]). Safety rules: only files inside the
//!   listed folders, never through a junction or link, never the folder itself, only
//!   files older than the category's minimum age, files in use are skipped, and a
//!   browser cache is left alone while that browser is open.
//! - Windows' own Disk Cleanup handlers ([`Source::Handler`], see `volcache`).
//! - The Recycle Bin of the drive.
//!
//! Freed space is reported as what was really removed.

use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows_sys::Win32::UI::Shell::{SHEmptyRecycleBinW, SHQueryRecycleBinW, SHQUERYRBINFO};

use super::volcache;
use super::volume::Volume;
use crate::process;
use crate::win::wide;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const SHERB_NOCONFIRMATION: u32 = 0x1;
const SHERB_NOPROGRESSUI: u32 = 0x2;
const SHERB_NOSOUND: u32 = 0x4;

pub const COUNT: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    UserTemp,
    WindowsTemp,
    BrowserCache,
    AppCrashDumps,
    WindowsUpdate,
    DeliveryOptimization,
    ErrorReports,
    Thumbnails,
    MemoryDumps,
    SetupLogs,
    ShaderCache,
    RecycleBin,
    PreviousWindows,
    OldDrivers,
}

pub enum Source {
    /// Our own folder list; files younger than `min_age` stay.
    Files {
        min_age: Duration,
    },
    /// Disk Cleanup handlers, by their `VolumeCaches` key name.
    Handler(&'static [&'static str]),
    RecycleBin,
}

impl Category {
    /// Display order.
    pub const ALL: [Category; COUNT] = [
        Category::UserTemp,
        Category::WindowsTemp,
        Category::BrowserCache,
        Category::AppCrashDumps,
        Category::WindowsUpdate,
        Category::DeliveryOptimization,
        Category::ErrorReports,
        Category::Thumbnails,
        Category::MemoryDumps,
        Category::SetupLogs,
        Category::ShaderCache,
        Category::RecycleBin,
        Category::PreviousWindows,
        Category::OldDrivers,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Name in `MemoryConfig.ini`: `DiskClean<key>`.
    pub fn key(self) -> &'static str {
        match self {
            Category::UserTemp => "UserTemp",
            Category::WindowsTemp => "WindowsTemp",
            Category::BrowserCache => "BrowserCache",
            Category::AppCrashDumps => "AppCrashDumps",
            Category::WindowsUpdate => "WindowsUpdate",
            Category::DeliveryOptimization => "DeliveryOptimization",
            Category::ErrorReports => "ErrorReports",
            Category::Thumbnails => "Thumbnails",
            Category::MemoryDumps => "MemoryDumps",
            Category::SetupLogs => "SetupLogs",
            Category::ShaderCache => "ShaderCache",
            Category::RecycleBin => "RecycleBin",
            Category::PreviousWindows => "PreviousWindows",
            Category::OldDrivers => "OldDrivers",
        }
    }

    /// Something the user may want to keep: off by default and marked in the list.
    pub fn careful(self) -> bool {
        matches!(
            self,
            Category::ShaderCache
                | Category::RecycleBin
                | Category::PreviousWindows
                | Category::OldDrivers
        )
    }

    pub fn default_on(self) -> bool {
        !self.careful()
    }

    pub fn source(self) -> Source {
        match self {
            Category::UserTemp | Category::WindowsTemp => Source::Files { min_age: DAY },
            Category::BrowserCache | Category::AppCrashDumps | Category::ShaderCache => {
                Source::Files {
                    min_age: Duration::ZERO,
                }
            }
            Category::WindowsUpdate => Source::Handler(&["Update Cleanup"]),
            Category::DeliveryOptimization => Source::Handler(&["Delivery Optimization Files"]),
            Category::ErrorReports => Source::Handler(&["Windows Error Reporting Files"]),
            Category::Thumbnails => Source::Handler(&["Thumbnail Cache"]),
            Category::MemoryDumps => Source::Handler(&[
                "System error memory dump files",
                "System error minidump files",
            ]),
            Category::SetupLogs => Source::Handler(&[
                "Windows Upgrade Log Files",
                "Setup Log Files",
                "Windows Reset Log Files",
            ]),
            Category::PreviousWindows => Source::Handler(&[
                "Previous Installations",
                "Temporary Setup Files",
                "Upgrade Discarded Files",
            ]),
            Category::OldDrivers => Source::Handler(&["Device Driver Packages"]),
            Category::RecycleBin => Source::RecycleBin,
        }
    }

    /// Whether the category is shown for this drive. Windows and app junk lives on the
    /// system drive; any local drive has its own Recycle Bin.
    pub fn applies(self, v: &Volume) -> bool {
        match self.source() {
            Source::RecycleBin => v.can_clean(),
            _ => v.system && v.can_clean(),
        }
    }
}

/// Result of measuring or cleaning one category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done {
        bytes: u64,
        /// Files counted or removed; 0 when Windows does not say (handlers).
        files: u64,
        /// Files in use or protected, left in place.
        skipped: u64,
        /// A browser that was open, so its cache was left alone.
        blocked: Option<&'static str>,
    },
    /// Not on this Windows version.
    Unavailable,
    /// Windows refused (usually: not running as administrator).
    Failed,
    Cancelled,
}

impl Status {
    pub fn bytes(&self) -> u64 {
        match self {
            Status::Done { bytes, .. } => *bytes,
            _ => 0,
        }
    }

    fn done(t: Tally, blocked: Option<&'static str>) -> Status {
        Status::Done {
            bytes: t.bytes,
            files: t.files,
            skipped: t.skipped,
            blocked,
        }
    }
}

pub fn measure(cat: Category, volume: &Volume, cancel: &AtomicBool) -> Status {
    run(cat, volume, false, cancel)
}

pub fn clean(cat: Category, volume: &Volume, cancel: &AtomicBool) -> Status {
    run(cat, volume, true, cancel)
}

fn run(cat: Category, volume: &Volume, delete: bool, cancel: &AtomicBool) -> Status {
    let root = volume.root();
    let status = match cat.source() {
        Source::Files { min_age } => {
            let (roots, blocked) = file_roots(cat);
            let mut total = Tally::default();
            let mut unreadable = false;
            for r in roots.iter().filter(|r| on_drive(r, volume.letter)) {
                // A folder that exists but cannot be listed (no administrator rights).
                unreadable |= r.is_dir() && std::fs::read_dir(r).is_err();
                total.add(sweep(r, min_age, delete, cancel));
            }
            if unreadable && total.files == 0 && total.skipped == 0 {
                Status::Failed
            } else {
                Status::done(total, blocked)
            }
        }
        Source::Handler(names) => handlers(names, &root, delete, cancel),
        Source::RecycleBin => recycle_bin(&root, delete),
    };
    if cancel.load(Ordering::Relaxed) {
        Status::Cancelled
    } else {
        status
    }
}

fn handlers(names: &[&str], root: &str, delete: bool, cancel: &AtomicBool) -> Status {
    let (mut bytes, mut any, mut failed) = (0u64, false, false);
    for name in names {
        let r = if delete {
            volcache::purge(name, root, cancel)
        } else {
            volcache::space_used(name, root, cancel)
        };
        match r {
            Ok(b) => {
                bytes += b;
                any = true;
            }
            Err(volcache::Error::Missing) => {}
            Err(volcache::Error::Cancelled) => return Status::Cancelled,
            Err(volcache::Error::Failed) => failed = true,
        }
    }
    match (any, failed) {
        (true, _) => Status::Done {
            bytes,
            files: 0,
            skipped: 0,
            blocked: None,
        },
        (false, true) => Status::Failed,
        (false, false) => Status::Unavailable,
    }
}

fn recycle_bin(root: &str, delete: bool) -> Status {
    let root = wide(root);
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        i64Size: 0,
        i64NumItems: 0,
    };
    if unsafe { SHQueryRecycleBinW(root.as_ptr(), &mut info) } < 0 {
        return Status::Failed;
    }
    let found = Status::Done {
        bytes: info.i64Size.max(0) as u64,
        files: info.i64NumItems.max(0) as u64,
        skipped: 0,
        blocked: None,
    };
    if !delete || info.i64NumItems <= 0 {
        return found;
    }
    let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
    if unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), root.as_ptr(), flags) } < 0 {
        return Status::Failed;
    }
    found
}

// ------------------------------------------------------------------- files

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub bytes: u64,
    pub files: u64,
    pub skipped: u64,
}

impl Tally {
    fn add(&mut self, o: Tally) {
        self.bytes += o.bytes;
        self.files += o.files;
        self.skipped += o.skipped;
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn on_drive(p: &Path, letter: char) -> bool {
    match p.components().next() {
        Some(Component::Prefix(pre)) => pre
            .as_os_str()
            .to_str()
            .and_then(|s| s.chars().next())
            .is_some_and(|c| c.eq_ignore_ascii_case(&letter)),
        _ => false,
    }
}

/// Chromium-based browsers: (name, executable, `User Data` folder under %LOCALAPPDATA%).
const CHROMIUM: [(&str, &str, &str); 4] = [
    ("Google Chrome", "chrome.exe", r"Google\Chrome\User Data"),
    ("Microsoft Edge", "msedge.exe", r"Microsoft\Edge\User Data"),
    (
        "Brave",
        "brave.exe",
        r"BraveSoftware\Brave-Browser\User Data",
    ),
    ("Vivaldi", "vivaldi.exe", r"Vivaldi\User Data"),
];
/// Cache folders inside a Chromium profile. Cookies, history and passwords are elsewhere.
const CHROMIUM_PROFILE_CACHES: [&str; 3] = ["Cache", "Code Cache", "GPUCache"];
/// Cache folders shared by all profiles of a Chromium browser.
const CHROMIUM_SHARED_CACHES: [&str; 3] = ["ShaderCache", "GrShaderCache", "GraphiteDawnCache"];

fn subdirs(p: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

fn is_chromium_profile(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "Default" || n.starts_with("Profile ") || n == "Guest Profile")
}

/// Cache folders of every browser profile, leaving out browsers that are open. Returns
/// the folders and the name of an open browser, if any.
fn browser_roots(running: &dyn Fn(&str) -> bool) -> (Vec<PathBuf>, Option<&'static str>) {
    let mut roots = Vec::new();
    let mut blocked = None;
    let Some(local) = env_path("LOCALAPPDATA") else {
        return (roots, None);
    };
    let mut browsers: Vec<(&'static str, &str, Vec<PathBuf>)> = Vec::new();
    for (name, exe, data) in CHROMIUM {
        let data = local.join(data);
        let mut dirs: Vec<PathBuf> = CHROMIUM_SHARED_CACHES
            .iter()
            .map(|c| data.join(c))
            .collect();
        for profile in subdirs(&data)
            .into_iter()
            .filter(|p| is_chromium_profile(p))
        {
            dirs.extend(CHROMIUM_PROFILE_CACHES.iter().map(|c| profile.join(c)));
        }
        browsers.push((name, exe, dirs));
    }
    let opera = local.join(r"Opera Software\Opera Stable");
    browsers.push((
        "Opera",
        "opera.exe",
        CHROMIUM_PROFILE_CACHES
            .iter()
            .map(|c| opera.join(c))
            .collect(),
    ));
    let firefox = subdirs(&local.join(r"Mozilla\Firefox\Profiles"))
        .into_iter()
        .map(|p| p.join("cache2"))
        .collect();
    browsers.push(("Firefox", "firefox.exe", firefox));

    for (name, exe, dirs) in browsers {
        let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| d.is_dir()).collect();
        if dirs.is_empty() {
            continue;
        }
        if running(exe) {
            blocked.get_or_insert(name);
        } else {
            roots.extend(dirs);
        }
    }
    (roots, blocked)
}

/// Folders of a [`Source::Files`] category, and a browser that blocked some of them.
pub fn file_roots(cat: Category) -> (Vec<PathBuf>, Option<&'static str>) {
    let local = env_path("LOCALAPPDATA");
    let under_local = |subs: &[&str]| -> Vec<PathBuf> {
        local
            .iter()
            .flat_map(|l| subs.iter().map(move |s| l.join(s)))
            .collect()
    };
    let roots = match cat {
        Category::UserTemp => env_path("TEMP")
            .or_else(|| env_path("TMP"))
            .into_iter()
            .collect(),
        Category::WindowsTemp => env_path("SystemRoot")
            .map(|w| w.join("Temp"))
            .into_iter()
            .collect(),
        Category::AppCrashDumps => under_local(&["CrashDumps"]),
        Category::ShaderCache => {
            let mut v = under_local(&[
                "D3DSCache",
                r"NVIDIA\DXCache",
                r"NVIDIA\GLCache",
                r"AMD\DxCache",
                r"AMD\DxcCache",
                r"AMD\GLCache",
                r"AMD\VkCache",
            ]);
            if let Some(profile) = env_path("USERPROFILE") {
                let low = profile.join(r"AppData\LocalLow");
                v.push(low.join(r"NVIDIA\PerDriverVersion\DXCache"));
                v.push(low.join(r"Intel\ShaderCache"));
            }
            v
        }
        Category::BrowserCache => {
            let names: Vec<String> = process::list()
                .into_iter()
                .map(|p| p.name.to_ascii_lowercase())
                .collect();
            return browser_roots(&|exe| names.iter().any(|n| n == exe));
        }
        _ => Vec::new(),
    };
    (roots, None)
}

/// Folders that must never be cleaned as a whole, nor any folder above them.
fn protected() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "SystemRoot",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
        "PUBLIC",
    ]
    .iter()
    .filter_map(|v| env_path(v))
    .collect();
    if let Some(w) = env_path("SystemRoot") {
        v.push(w.join("System32"));
    }
    v
}

/// A folder may be cleaned only if it is absolute, at least two levels below the drive
/// root (a single level is fine for "Temp" or "Tmp"), and is not one of `protected`
/// nor above one of them.
pub fn safe_root(p: &Path, protected: &[PathBuf]) -> bool {
    let mut comps = p.components();
    if !matches!(comps.next(), Some(Component::Prefix(_)))
        || !matches!(comps.next(), Some(Component::RootDir))
    {
        return false;
    }
    let names: Vec<String> = comps
        .map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect::<Option<_>>()
        .unwrap_or_default();
    let deep_enough =
        names.len() >= 2 || (names.len() == 1 && (names[0] == "temp" || names[0] == "tmp"));
    if !deep_enough {
        return false;
    }
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    let me = lower(p);
    !protected.iter().any(|x| lower(x).starts_with(&me))
}

fn is_reparse(md: &std::fs::Metadata) -> bool {
    md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn old_enough(md: &std::fs::Metadata, min_age: Duration, now: SystemTime) -> bool {
    min_age.is_zero()
        || md
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= min_age)
}

/// Counts (and with `delete`, removes) the files under `root` that are older than
/// `min_age`. Never follows a junction or link, never removes `root` itself, and
/// removes empty subfolders only when they are old enough too.
pub fn sweep(root: &Path, min_age: Duration, delete: bool, cancel: &AtomicBool) -> Tally {
    let mut t = Tally::default();
    let Ok(md) = std::fs::symlink_metadata(root) else {
        return t;
    };
    if !md.is_dir() || is_reparse(&md) || !safe_root(root, &protected()) {
        return t;
    }
    let now = SystemTime::now();
    let mut stack = vec![root.to_path_buf()];
    let mut folders = Vec::new();
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            // On Windows this comes from the directory listing and does not follow links.
            let Ok(md) = e.metadata() else { continue };
            if is_reparse(&md) {
                continue;
            }
            let path = e.path();
            if md.is_dir() {
                stack.push(path.clone());
                folders.push((path, md));
                continue;
            }
            if !old_enough(&md, min_age, now) {
                continue;
            }
            if !delete || remove_file(&path, &md) {
                t.bytes += md.len();
                t.files += 1;
            } else {
                t.skipped += 1;
            }
        }
    }
    if delete {
        // Deepest first, so a folder that only held empty folders goes too.
        folders.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
        for (p, md) in folders {
            if old_enough(&md, min_age, now) {
                let _ = std::fs::remove_dir(&p); // fails while not empty
            }
        }
    }
    t
}

fn remove_file(path: &Path, md: &std::fs::Metadata) -> bool {
    if std::fs::remove_file(path).is_ok() {
        return true;
    }
    if md.permissions().readonly() {
        let mut p = md.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        return std::fs::set_permissions(path, p).is_ok() && std::fs::remove_file(path).is_ok();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::scan::tests::{write_file, TempDir};
    use crate::disk::volume::{Kind, Media};

    fn vol(letter: char, kind: Kind, system: bool) -> Volume {
        Volume {
            letter,
            label: String::new(),
            fs: "NTFS".into(),
            kind,
            media: Media::Ssd,
            total: 100,
            free: 50,
            system,
        }
    }

    #[test]
    fn categories_have_unique_keys_and_indexes() {
        for (i, c) in Category::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
            assert_eq!(
                Category::ALL.iter().filter(|o| o.key() == c.key()).count(),
                1
            );
        }
        assert!(!Category::RecycleBin.default_on());
        assert!(Category::UserTemp.default_on());
    }

    #[test]
    fn only_the_recycle_bin_applies_to_other_drives() {
        let data = vol('D', Kind::Fixed, false);
        let shown: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|c| c.applies(&data))
            .collect();
        assert_eq!(shown, vec![Category::RecycleBin]);
        let net = vol('Z', Kind::Network, false);
        assert!(Category::ALL.iter().all(|c| !c.applies(&net)));
        let sys = vol('C', Kind::Fixed, true);
        assert!(Category::ALL.iter().all(|c| c.applies(&sys)));
    }

    #[test]
    fn root_rules() {
        let protected = vec![
            PathBuf::from(r"C:\Windows"),
            PathBuf::from(r"C:\Windows\System32"),
            PathBuf::from(r"C:\Users\Ana"),
            PathBuf::from(r"C:\Users\Ana\AppData\Local"),
        ];
        let ok = |p: &str| safe_root(Path::new(p), &protected);
        assert!(ok(r"C:\Windows\Temp"));
        assert!(ok(r"C:\Users\Ana\AppData\Local\Temp"));
        assert!(ok(r"C:\Temp"));
        assert!(!ok(r"C:\"));
        assert!(!ok(r"C:\Games"));
        assert!(!ok(r"C:\Windows"));
        assert!(!ok(r"c:\users\ana")); // case does not matter
        assert!(!ok(r"C:\Users")); // above a protected folder
        assert!(!ok(r"C:\Users\Ana\..\Bob"));
        assert!(!ok(r"Temp\cache"));
    }

    #[test]
    fn drive_of_a_path() {
        assert!(on_drive(Path::new(r"c:\Windows\Temp"), 'C'));
        assert!(!on_drive(Path::new(r"D:\Temp"), 'C'));
    }

    #[test]
    fn sweep_respects_age_and_never_follows_junctions() {
        let dir = TempDir::new("sweep");
        let root = dir.0.join("cache");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        write_file(&root.join("a.tmp"), 1000);
        write_file(&root.join("sub").join("b.tmp"), 500);
        // A folder outside, reachable only through a junction inside the root.
        let outside = dir.0.join("keep");
        std::fs::create_dir(&outside).unwrap();
        write_file(&outside.join("precious.txt"), 10);
        let junction = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(root.join("link"))
            .arg(&outside)
            .output()
            .is_ok_and(|o| o.status.success());

        let cancel = AtomicBool::new(false);
        // Just written: too young for a one-day minimum age.
        assert_eq!(sweep(&root, DAY, true, &cancel), Tally::default());
        let found = sweep(&root, Duration::ZERO, false, &cancel);
        assert_eq!((found.bytes, found.files), (1500, 2));
        assert!(root.join("a.tmp").exists(), "measuring must not delete");

        let removed = sweep(&root, Duration::ZERO, true, &cancel);
        assert_eq!(
            (removed.bytes, removed.files, removed.skipped),
            (1500, 2, 0)
        );
        assert!(root.is_dir(), "the folder itself stays");
        assert!(!root.join("sub").exists(), "empty subfolders go");
        assert!(outside.join("precious.txt").exists());
        if junction {
            assert!(root.join("link").exists(), "the junction is left alone");
        }
    }

    #[test]
    fn files_in_use_are_skipped() {
        let dir = TempDir::new("inuse");
        let root = dir.0.join("cache").join("x");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("locked.tmp");
        write_file(&path, 100);
        // No FILE_SHARE_DELETE: the file cannot be removed while this is open.
        use std::os::windows::fs::OpenOptionsExt;
        let _hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let t = sweep(
            &dir.0.join("cache"),
            Duration::ZERO,
            true,
            &AtomicBool::new(false),
        );
        assert_eq!((t.files, t.skipped), (0, 1));
    }

    /// Manual check on the real system drive; only measures, never deletes:
    /// `cargo test --lib measure_system_drive -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_system_drive() {
        let v = crate::disk::volume::list()
            .into_iter()
            .find(|v| v.system)
            .unwrap();
        let cancel = AtomicBool::new(false);
        for c in Category::ALL {
            let started = std::time::Instant::now();
            let s = measure(c, &v, &cancel);
            println!("{:<22} {:?} ({:?})", c.key(), s, started.elapsed());
        }
    }

    #[test]
    fn open_browsers_are_left_alone() {
        let (_, blocked) = browser_roots(&|_| true);
        let (none_blocked_roots, none) = browser_roots(&|_| false);
        assert_eq!(none, None);
        // Blocked only if that browser has cache folders on this machine.
        assert_eq!(blocked.is_some(), !none_blocked_roots.is_empty());
    }
}
