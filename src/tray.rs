//! Значок в области уведомлений (только Windows) и иконка приложения.

use std::sync::mpsc;

#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayCmd {
    Show,
    Quit,
}

/// Иконка: синий круг с белым кольцом и точкой. Возвращает RGBA size×size.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    let c = (size as f32 - 1.0) / 2.0;
    let r = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt() / r;
            // Сглаживание края круга.
            let alpha = ((1.0 - d) * r).clamp(0.0, 1.0);
            let white = (0.45..0.62).contains(&d) || d < 0.2;
            let (cr, cg, cb) = if white { (255, 255, 255) } else { (47, 111, 181) };
            px.extend_from_slice(&[cr, cg, cb, (alpha * 255.0) as u8]);
        }
    }
    px
}

pub struct Tray {
    #[cfg(windows)]
    _icon: tray_icon::TrayIcon,
    rx: mpsc::Receiver<TrayCmd>,
}

impl Tray {
    #[cfg(windows)]
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let menu = Menu::new();
        let show = MenuItem::new("Показать", true, None);
        let quit = MenuItem::new("Выход", true, None);
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

        let icon = tray_icon::Icon::from_rgba(icon_rgba(32), 32, 32).ok()?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("PgListener")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;
        Some(Self { _icon: tray, rx })
    }

    #[cfg(not(windows))]
    pub fn new(_ctx: &egui::Context) -> Option<Self> {
        None
    }

    pub fn poll(&self) -> Option<TrayCmd> {
        self.rx.try_recv().ok()
    }
}
