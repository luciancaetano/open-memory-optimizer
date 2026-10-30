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

//! Drive analyzer: finds the folders and files that take the most space. Read only:
//! it never changes or deletes anything.
//!
//! Folders are listed in parallel with `NtQueryDirectoryFile(FileDirectoryInformation)`,
//! which returns each entry's allocated size without opening the file, so cloud
//! placeholders are never downloaded. Sizes are space on disk (allocation size).
//! Reparse points (junctions, symbolic links, mount points) are not followed, so
//! nothing is counted twice and the walk stays on the drive.
//!
//! Only folders are kept in memory (one [`Node`] each). Files are reduced to the
//! [`TOP_FILES`] biggest and a total per [`FileKind`].

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::win::{self, wide, OwnedHandle};

/// How many of the biggest files are kept.
pub const TOP_FILES: usize = 200;

const NO_PARENT: u32 = u32::MAX;
const STATUS_NO_MORE_FILES: i32 = 0x8000_0006_u32 as i32;
const FILE_DIRECTORY_INFORMATION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileKind {
    Videos,
    Music,
    Pictures,
    Documents,
    Archives,
    Programs,
    Other,
}

impl FileKind {
    pub const ALL: [FileKind; 7] = [
        FileKind::Videos,
        FileKind::Music,
        FileKind::Pictures,
        FileKind::Documents,
        FileKind::Archives,
        FileKind::Programs,
        FileKind::Other,
    ];

    /// Kind from a lower-case extension without the dot.
    pub fn from_ext(ext: &str) -> FileKind {
        match ext {
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "m4v" | "flv" | "mpg" | "mpeg"
            | "ts" | "m2ts" | "3gp" => FileKind::Videos,
            "mp3" | "flac" | "wav" | "aac" | "ogg" | "m4a" | "wma" | "opus" | "aiff" => {
                FileKind::Music
            }
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "heic" | "heif" | "tif" | "tiff"
            | "raw" | "cr2" | "nef" | "arw" | "dng" | "psd" | "svg" | "ico" => FileKind::Pictures,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
            | "txt" | "rtf" | "csv" | "md" | "epub" => FileKind::Documents,
            "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" | "cab" | "iso" | "img"
            | "vhd" | "vhdx" | "vmdk" | "wim" | "esd" => FileKind::Archives,
            "exe" | "dll" | "sys" | "msi" | "msix" | "appx" | "msp" | "pak" | "bin" | "dat"
            | "so" | "ocx" | "mui" | "pdb" | "jar" => FileKind::Programs,
            _ => FileKind::Other,
        }
    }

    /// Kind from a file name; the extension is compared without regard to case.
    pub fn of_name(name: &str) -> FileKind {
        match name.rsplit_once('.') {
            Some((_, ext)) if !ext.is_empty() && ext.len() <= 5 => {
                Self::from_ext(&ext.to_ascii_lowercase())
            }
            _ => FileKind::Other,
        }
    }

    /// Same as [`of_name`] on a UTF-16 name, without allocating.
    fn of_utf16(name: &[u16]) -> FileKind {
        let Some(dot) = name.iter().rposition(|&c| c == b'.' as u16) else {
            return FileKind::Other;
        };
        let ext = &name[dot + 1..];
        if ext.is_empty() || ext.len() > 5 {
            return FileKind::Other;
        }
        let mut buf = [0u8; 5];
        for (b, &c) in buf.iter_mut().zip(ext) {
            if c > 0x7f {
                return FileKind::Other;
            }
            *b = (c as u8).to_ascii_lowercase();
        }
        std::str::from_utf8(&buf[..ext.len()]).map_or(FileKind::Other, Self::from_ext)
    }
}

/// One folder. `size` and `files` include everything below it once the scan is done.
#[derive(Debug, Clone)]
pub struct Node {
    pub name: Box<str>,
    pub parent: u32,
    /// Space on disk, in bytes.
    pub size: u64,
    pub files: u64,
}

/// The folder tree. Node 0 is the drive root; children are sorted biggest first.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    nodes: Vec<Node>,
    children: Vec<Vec<u32>>,
}

impl Tree {
    /// Adds every folder's size and file count to its parents and sorts the children.
    /// Children always come after their parent in `nodes`.
    pub fn build(mut nodes: Vec<Node>) -> Tree {
        for i in (1..nodes.len()).rev() {
            let (parent, size, files) = (nodes[i].parent, nodes[i].size, nodes[i].files);
            if let Some(p) = nodes.get_mut(parent as usize) {
                p.size += size;
                p.files += files;
            }
        }
        let mut children = vec![Vec::new(); nodes.len()];
        for (i, n) in nodes.iter().enumerate().skip(1) {
            if let Some(c) = children.get_mut(n.parent as usize) {
                c.push(i as u32);
            }
        }
        for c in &mut children {
            c.sort_by_key(|&i| Reverse(nodes[i as usize].size));
        }
        Tree { nodes, children }
    }

    pub fn node(&self, i: u32) -> &Node {
        &self.nodes[i as usize]
    }

    pub fn children(&self, i: u32) -> &[u32] {
        &self.children[i as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Full path; the root is "C:\".
    pub fn path(&self, i: u32) -> String {
        let mut parts = Vec::new();
        let mut at = i;
        while at != NO_PARENT {
            let n = &self.nodes[at as usize];
            parts.push(&*n.name);
            at = n.parent;
        }
        parts.reverse();
        join_path(&parts)
    }
}

fn join_path(parts: &[&str]) -> String {
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 && !s.ends_with('\\') {
            s.push('\\');
        }
        s.push_str(p);
    }
    s
}

#[derive(Debug, Clone)]
pub struct BigFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct KindTotal {
    pub bytes: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub files: u64,
    pub bytes: u64,
    /// A folder being read right now.
    pub current: String,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub root: String,
    pub tree: Tree,
    /// Biggest first.
    pub top_files: Vec<BigFile>,
    /// In [`FileKind::ALL`] order.
    pub kinds: [KindTotal; 7],
    pub files: u64,
    pub folders: u64,
    /// Folders that could not be opened.
    pub denied: u64,
    pub elapsed: Duration,
    pub cancelled: bool,
}

impl Report {
    pub fn total(&self) -> u64 {
        if self.tree.is_empty() {
            0
        } else {
            self.tree.node(0).size
        }
    }
}

// ----------------------------------------------------------------- listing

/// One entry of a `FILE_DIRECTORY_INFORMATION` buffer.
pub(crate) struct RawEntry<'a> {
    /// UTF-16 name, as raw little-endian bytes.
    name: &'a [u8],
    pub attrs: u32,
    pub alloc: u64,
}

impl RawEntry<'_> {
    fn name_utf16(&self) -> Vec<u16> {
        self.name
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect()
    }

    pub fn name(&self) -> String {
        String::from_utf16_lossy(&self.name_utf16())
    }

    fn is_dot(&self) -> bool {
        matches!(self.name, [b'.', 0] | [b'.', 0, b'.', 0])
    }

    pub fn is_dir(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    pub fn is_reparse(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

/// Walks the entries of a buffer filled by `NtQueryDirectoryFile` with
/// `FileDirectoryInformation`: NextEntryOffset at 0, EndOfFile at 40, AllocationSize
/// at 48, FileAttributes at 56, FileNameLength at 60, FileName at 64.
pub(crate) fn parse_entries(buf: &[u8], mut f: impl FnMut(RawEntry)) {
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let mut at = 0usize;
    while at + 64 <= buf.len() {
        let e = &buf[at..];
        let next = u32_at(e, 0) as usize;
        let name_len = u32_at(e, 60) as usize;
        if 64 + name_len > e.len() {
            break;
        }
        let entry = RawEntry {
            name: &e[64..64 + name_len],
            attrs: u32_at(e, 56),
            alloc: u64_at(e, 48),
        };
        if !entry.is_dot() {
            f(entry);
        }
        if next == 0 {
            break;
        }
        at += next;
    }
}

type NtQueryDirectoryFile = unsafe extern "system" fn(
    handle: *mut c_void,
    event: *mut c_void,
    apc: *const c_void,
    apc_context: *const c_void,
    iosb: *mut [usize; 2],
    info: *mut c_void,
    length: u32,
    class: u32,
    single: u8,
    name: *const c_void,
    restart: u8,
) -> i32;

fn nt_query_directory_file() -> Option<NtQueryDirectoryFile> {
    static F: OnceLock<Option<usize>> = OnceLock::new();
    F.get_or_init(|| win::resolve("ntdll.dll", "NtQueryDirectoryFile").map(|p| p as usize))
        .map(|p| unsafe { std::mem::transmute::<usize, NtQueryDirectoryFile>(p) })
}

/// `\\?\` form so paths longer than MAX_PATH open too.
pub(crate) fn long_path(path: &str) -> Vec<u16> {
    if path.starts_with("\\\\") {
        wide(path)
    } else {
        wide(&format!("\\\\?\\{path}"))
    }
}

/// Lists one folder. `buf` is reused between calls. Err if the folder cannot be opened.
pub(crate) fn list_dir(path: &str, buf: &mut [u64], mut f: impl FnMut(RawEntry)) -> Result<(), ()> {
    let query = nt_query_directory_file().ok_or(())?;
    let p = long_path(path);
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    let h = OwnedHandle::new(h).ok_or(())?;
    let bytes = std::mem::size_of_val(buf);
    loop {
        let mut iosb = [0usize; 2];
        let status = unsafe {
            query(
                h.0,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                &mut iosb,
                buf.as_mut_ptr().cast(),
                bytes as u32,
                FILE_DIRECTORY_INFORMATION,
                0,
                std::ptr::null(),
                0,
            )
        };
        if status == STATUS_NO_MORE_FILES || status < 0 {
            return Ok(());
        }
        let filled = iosb[1].min(bytes);
        let raw = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), filled) };
        parse_entries(raw, &mut f);
    }
}

// -------------------------------------------------------------------- scan

struct Queue {
    jobs: Vec<(u32, String)>,
    active: usize,
}

struct Walk<'a> {
    queue: Mutex<Queue>,
    wake: Condvar,
    nodes: Mutex<Vec<Node>>,
    files: AtomicU64,
    bytes: AtomicU64,
    denied: AtomicU64,
    finished: AtomicUsize,
    current: Mutex<String>,
    cancel: &'a AtomicBool,
}

#[derive(Default)]
struct WorkerResult {
    kinds: [KindTotal; 7],
    top: BinaryHeap<Reverse<(u64, String)>>,
}

impl Walk<'_> {
    fn next_job(&self) -> Option<(u32, String)> {
        let mut q = self.queue.lock().unwrap();
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(job) = q.jobs.pop() {
                q.active += 1;
                return Some(job);
            }
            if q.active == 0 {
                self.wake.notify_all();
                return None;
            }
            q = self
                .wake
                .wait_timeout(q, Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }

    fn worker(&self) -> WorkerResult {
        let mut out = WorkerResult::default();
        let mut buf = vec![0u64; 64 * 1024 / 8];
        while let Some((index, path)) = self.next_job() {
            if let Ok(mut c) = self.current.try_lock() {
                c.clone_from(&path);
            }
            let mut subdirs: Vec<String> = Vec::new();
            let (mut files, mut bytes) = (0u64, 0u64);
            let listed = list_dir(&path, &mut buf, |e| {
                if e.is_dir() {
                    // Junctions and links are not followed: their target is counted
                    // where it really is, or not at all if it is on another drive.
                    if !e.is_reparse() {
                        subdirs.push(e.name());
                    }
                    return;
                }
                files += 1;
                bytes += e.alloc;
                let kind = FileKind::of_utf16(&e.name_utf16());
                let k = &mut out.kinds[kind as usize];
                k.bytes += e.alloc;
                k.files += 1;
                let full = out.top.len() >= TOP_FILES;
                if !full || out.top.peek().is_some_and(|Reverse((s, _))| e.alloc > *s) {
                    let p = join_path(&[&path, &e.name()]);
                    out.top.push(Reverse((e.alloc, p)));
                    if full {
                        out.top.pop();
                    }
                }
            });
            if listed.is_err() {
                self.denied.fetch_add(1, Ordering::Relaxed);
            }
            self.files.fetch_add(files, Ordering::Relaxed);
            self.bytes.fetch_add(bytes, Ordering::Relaxed);

            let mut jobs = Vec::with_capacity(subdirs.len());
            {
                let mut nodes = self.nodes.lock().unwrap();
                let me = &mut nodes[index as usize];
                me.size += bytes;
                me.files += files;
                for name in subdirs {
                    let child = nodes.len() as u32;
                    jobs.push((child, join_path(&[&path, &name])));
                    nodes.push(Node {
                        name: name.into_boxed_str(),
                        parent: index,
                        size: 0,
                        files: 0,
                    });
                }
            }
            let mut q = self.queue.lock().unwrap();
            q.jobs.extend(jobs);
            q.active -= 1;
            self.wake.notify_all();
        }
        self.finished.fetch_add(1, Ordering::Release);
        self.wake.notify_all();
        out
    }
}

/// Scans everything under `root` (e.g. "C:\"). `progress` is called about five times
/// a second on the calling thread. Setting `cancel` stops early; the report then holds
/// what was found so far and has `cancelled` set.
pub fn run(root: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(&Progress)) -> Report {
    let started = Instant::now();
    let root_name = root.trim_end_matches('\\');
    let root_name = if root_name.len() == 2 {
        format!("{root_name}\\")
    } else {
        root_name.to_owned()
    };
    let walk = Walk {
        queue: Mutex::new(Queue {
            jobs: vec![(0, root_name.clone())],
            active: 0,
        }),
        wake: Condvar::new(),
        nodes: Mutex::new(vec![Node {
            name: root_name.clone().into_boxed_str(),
            parent: NO_PARENT,
            size: 0,
            files: 0,
        }]),
        files: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        finished: AtomicUsize::new(0),
        current: Mutex::new(root_name.clone()),
        cancel,
    };
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(2, 8);
    let results: Vec<WorkerResult> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads).map(|_| s.spawn(|| walk.worker())).collect();
        while walk.finished.load(Ordering::Acquire) < threads {
            let q = walk.queue.lock().unwrap();
            drop(
                walk.wake
                    .wait_timeout(q, Duration::from_millis(200))
                    .unwrap(),
            );
            progress(&Progress {
                files: walk.files.load(Ordering::Relaxed),
                bytes: walk.bytes.load(Ordering::Relaxed),
                current: walk.current.lock().unwrap().clone(),
            });
        }
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });

    let mut kinds = [KindTotal::default(); 7];
    let mut top: Vec<(u64, String)> = Vec::new();
    for r in results {
        for (k, v) in kinds.iter_mut().zip(r.kinds) {
            k.bytes += v.bytes;
            k.files += v.files;
        }
        top.extend(r.top.into_iter().map(|Reverse(x)| x));
    }
    top.sort_by_key(|(size, _)| Reverse(*size));
    top.truncate(TOP_FILES);
    let nodes = walk.nodes.into_inner().unwrap();
    let folders = nodes.len() as u64 - 1;
    Report {
        root: root_name,
        tree: Tree::build(nodes),
        top_files: top
            .into_iter()
            .map(|(size, path)| BigFile { path, size })
            .collect(),
        kinds,
        files: walk.files.load(Ordering::Relaxed),
        folders,
        denied: walk.denied.load(Ordering::Relaxed),
        elapsed: started.elapsed(),
        cancelled: cancel.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh empty folder under %TEMP%, removed on drop.
    pub(crate) struct TempDir(pub std::path::PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "omo-test-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }

        pub(crate) fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes `len` non-zero bytes, so NTFS has to allocate clusters for them.
    pub(crate) fn write_file(path: &std::path::Path, len: usize) {
        std::fs::write(path, vec![0x5a; len]).unwrap();
    }

    #[test]
    fn kinds_from_names() {
        assert_eq!(FileKind::of_name("Movie.MKV"), FileKind::Videos);
        assert_eq!(FileKind::of_name("song.flac"), FileKind::Music);
        assert_eq!(FileKind::of_name("setup.exe"), FileKind::Programs);
        assert_eq!(FileKind::of_name("Win11.iso"), FileKind::Archives);
        assert_eq!(FileKind::of_name("README"), FileKind::Other);
        assert_eq!(FileKind::of_name("archive.toolong"), FileKind::Other);
        let utf16: Vec<u16> = "Foto.JPEG".encode_utf16().collect();
        assert_eq!(FileKind::of_utf16(&utf16), FileKind::Pictures);
        let accented: Vec<u16> = "a.vídeo".encode_utf16().collect();
        assert_eq!(FileKind::of_utf16(&accented), FileKind::Other);
    }

    fn entry_bytes(name: &str, attrs: u32, alloc: u64, last: bool) -> Vec<u8> {
        let name: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let len = (64 + name.len()).next_multiple_of(8);
        let mut e = vec![0u8; len];
        let next = if last { 0 } else { len as u32 };
        e[0..4].copy_from_slice(&next.to_le_bytes());
        e[48..56].copy_from_slice(&alloc.to_le_bytes());
        e[56..60].copy_from_slice(&attrs.to_le_bytes());
        e[60..64].copy_from_slice(&(name.len() as u32).to_le_bytes());
        e[64..64 + name.len()].copy_from_slice(&name);
        e
    }

    #[test]
    fn parses_directory_buffers_and_skips_dots() {
        let mut buf = entry_bytes(".", FILE_ATTRIBUTE_DIRECTORY, 0, false);
        buf.extend(entry_bytes("..", FILE_ATTRIBUTE_DIRECTORY, 0, false));
        buf.extend(entry_bytes("pasta", FILE_ATTRIBUTE_DIRECTORY, 0, false));
        buf.extend(entry_bytes("vídeo.mp4", 0x20, 4096, true));
        let mut seen = Vec::new();
        parse_entries(&buf, |e| seen.push((e.name(), e.is_dir(), e.alloc)));
        assert_eq!(
            seen,
            vec![
                ("pasta".to_owned(), true, 0),
                ("vídeo.mp4".to_owned(), false, 4096)
            ]
        );
    }

    #[test]
    fn tree_adds_sizes_up_and_sorts_children() {
        let n = |name: &str, parent, size| Node {
            name: name.into(),
            parent,
            size,
            files: 1,
        };
        let t = Tree::build(vec![
            n("C:\\", NO_PARENT, 1),
            n("small", 0, 10),
            n("big", 0, 50),
            n("deep", 1, 100),
        ]);
        assert_eq!(t.node(0).size, 161);
        assert_eq!(t.node(0).files, 4);
        assert_eq!(t.node(1).size, 110);
        assert_eq!(t.children(0), &[1, 2]); // "small" holds 110 with "deep"
        assert_eq!(t.path(3), "C:\\small\\deep");
        assert_eq!(t.path(0), "C:\\");
    }

    #[test]
    fn scans_a_folder_without_following_junctions() {
        let dir = TempDir::new("scan");
        let inner = dir.0.join("inner");
        std::fs::create_dir(&inner).unwrap();
        write_file(&inner.join("big.bin"), 1 << 20);
        write_file(&dir.0.join("small.txt"), 64 * 1024);
        let link = dir.0.join("link");
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&inner)
            .output()
            .is_ok_and(|o| o.status.success());

        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let r = run(dir.path(), &cancel, &mut |_| calls += 1);
        assert!(!r.cancelled && calls > 0);
        assert_eq!(r.files, 2);
        assert!(r.total() >= (1 << 20) + 64 * 1024);
        if made {
            // Counted once, through "inner" only.
            assert!(r.total() < 2 << 20, "junction was followed");
        }
        assert!(r.top_files[0].path.ends_with("inner\\big.bin"));
        let first = r.tree.children(0)[0];
        assert_eq!(r.tree.path(first), inner.to_str().unwrap());
        assert_eq!(r.kinds[FileKind::Documents as usize].files, 1);
    }

    /// Manual check on the real system drive:
    /// `cargo test --lib scan_system_drive -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scan_system_drive() {
        let root = format!("{}:\\", crate::disk::volume::system_letter());
        let r = run(&root, &AtomicBool::new(false), &mut |_| {});
        println!(
            "{} files, {} folders, {} denied, {:.1} GB in {:?}",
            r.files,
            r.folders,
            r.denied,
            r.total() as f64 / (1u64 << 30) as f64,
            r.elapsed
        );
        for f in r.top_files.iter().take(5) {
            println!("{:>10} MB  {}", f.size >> 20, f.path);
        }
        assert!(r.files > 1000 && !r.cancelled);
    }

    #[test]
    fn cancelled_scans_say_so() {
        let dir = TempDir::new("cancel");
        let cancel = AtomicBool::new(true);
        let r = run(dir.path(), &cancel, &mut |_| {});
        assert!(r.cancelled);
    }
}
