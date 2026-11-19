//! Значок в области уведомлений (только Windows) и иконка приложения.

use crate::i18n::{Key, t};
use std::sync::mpsc;

#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayCmd {
    Show,
    Quit,
}

pub struct Tray {
    #[cfg(windows)]
    icon: tray_icon::TrayIcon,
    rx: mpsc::Receiver<TrayCmd>,
}

impl Tray {
    #[cfg(windows)]
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let menu = Menu::new();
        let show = MenuItem::new(t(Key::Show), true, None);
        let quit = MenuItem::new(t(Key::Quit), true, None);
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])
            .ok()?;

        let (tx, rx) = mpsc::channel();
        let (show_id, quit_id) = (show.id().clone(), quit.id().clone());
        let (menu_tx, menu_ctx) = (tx.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = if e.id == show_id {
                TrayCmd::Show
            } else if e.id == quit_id {
                TrayCmd::Quit
            } else {
                return;
            };
            let _ = menu_tx.send(cmd);
            menu_ctx.request_repaint();
        }));
        let click_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let show = matches!(
                e,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick { .. }
            );
            if show {
                let _ = tx.send(TrayCmd::Show);
                click_ctx.request_repaint();
            }
        }));

        let size = crate::icons::TRAY_SIZE;
        let rgba = crate::icons::rgba(crate::icons::DARK.tray, size).unwrap_or_default();
        let icon = tray_icon::Icon::from_rgba(rgba, size, size).ok()?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("PgListener")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;
        Some(Self { icon: tray, rx })
    }

    #[cfg(not(windows))]
    pub fn new(_ctx: &egui::Context) -> Option<Self> {
        None
    }

    /// Меняет иконку в области уведомлений (при смене темы оформления).
    pub fn set_icon(&self, rgba: Vec<u8>, size: u32) {
        #[cfg(windows)]
        if let Ok(icon) = tray_icon::Icon::from_rgba(rgba, size, size) {
            let _ = self.icon.set_icon(Some(icon));
        }
        #[cfg(not(windows))]
        let _ = (rgba, size);
    }

    pub fn poll(&self) -> Option<TrayCmd> {
        self.rx.try_recv().ok()
    }
}
