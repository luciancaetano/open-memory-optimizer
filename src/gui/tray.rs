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

//! System-tray icon and menu (the original's "Minimize to System tray").

use eframe::egui;
use std::sync::mpsc::Sender;

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMsg {
    Show,
    Optimize,
    Quit,
}

pub struct Tray {
    pub icon: TrayIcon,
    // Menu items must outlive the menu.
    _items: Vec<MenuItem>,
}

pub fn create(tx: Sender<Event>, ctx: egui::Context) -> Option<Tray> {
    let size = 32;
    let icon = Icon::from_rgba(crate::theme::app_icon_rgba(size), size, size).ok()?;

    let open = MenuItem::new("Open OpenMemoryOptimizer", true, None);
    let optimize = MenuItem::new("Free up memory", true, None);
    let quit = MenuItem::new("Exit", true, None);
    let menu = Menu::new();
    menu.append(&open).ok()?;
    menu.append(&optimize).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;

    let ids: [(MenuId, TrayMsg); 3] = [
        (open.id().clone(), TrayMsg::Show),
        (optimize.id().clone(), TrayMsg::Optimize),
        (quit.id().clone(), TrayMsg::Quit),
    ];
    {
        let tx = tx.clone();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some((_, msg)) = ids.iter().find(|(id, _)| *id == e.id) {
                let _ = tx.send(Event::Tray(*msg));
                ctx.request_repaint();
            }
        }));
    }
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let show = matches!(
            e,
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } | TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
        );
        if show {
            let _ = tx.send(Event::Tray(TrayMsg::Show));
            ctx.request_repaint();
        }
    }));

    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("OpenMemoryOptimizer")
        .with_icon(icon)
        .build()
        .ok()?;
    Some(Tray {
        icon,
        _items: vec![open, optimize, quit],
    })
}
