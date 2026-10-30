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

//! Debug-only visual check: `MEMOPT_SNAPSHOT=<dir> OpenMemoryOptimizer` opens the window
//! off-screen without activating it, saves a PNG of every page from egui's own
//! framebuffer, then exits. Nothing is captured from the desktop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

pub fn requested_dir() -> Option<PathBuf> {
    std::env::var_os("MEMOPT_SNAPSHOT").map(PathBuf::from)
}

pub struct Snap {
    dir: PathBuf,
    pages: Vec<&'static str>,
    index: usize,
    ready_at: Instant,
    in_flight: Option<Arc<AtomicBool>>,
}

pub enum Step {
    /// Show this page (index into the list given to `new`).
    Show(usize),
    Wait,
    Finished,
}

impl Snap {
    pub fn new(dir: PathBuf, pages: Vec<&'static str>) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            dir,
            pages,
            index: 0,
            // Let the history graph and animations fill in on the first page.
            ready_at: Instant::now() + Duration::from_secs(4),
            in_flight: None,
        }
    }

    pub fn drive(&mut self, ctx: &egui::Context) -> Step {
        ctx.request_repaint_after(Duration::from_millis(50));
        if self.index >= self.pages.len() {
            return Step::Finished;
        }
        if let Some(done) = &self.in_flight {
            if !done.load(Ordering::Acquire) {
                return Step::Wait;
            }
            self.in_flight = None;
            self.index += 1;
            self.ready_at = Instant::now() + Duration::from_millis(1200);
            return if self.index >= self.pages.len() {
                Step::Finished
            } else {
                Step::Show(self.index)
            };
        }
        if Instant::now() < self.ready_at {
            return Step::Show(self.index);
        }
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let path = self.dir.join(format!("{}.png", self.pages[self.index]));
        let repaint = ctx.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
            egui::ScreenshotCallback::new(move |img| {
                let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let _ = write_png(&path, img.size[0] as u32, img.size[1] as u32, &rgba);
                flag.store(true, Ordering::Release);
                repaint.request_repaint();
            }),
        ));
        self.in_flight = Some(done);
        Step::Wait
    }
}

fn crc32(chunks: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for data in chunks {
        for &b in *data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
    }
    !crc
}

/// Uncompressed (stored-deflate) RGBA PNG; fine for a debug artifact.
fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        z.push(blocks.peek().is_none() as u8);
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        png.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, png)
}
