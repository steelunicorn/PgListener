// В релизной сборке под Windows не открываем консольное окно.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bridge;
mod crypto;
mod logfile;
mod model;
mod storage;
mod tray;
mod worker;

use std::path::PathBuf;

/// Каталог исполняемого файла: здесь лежат база настроек и папка logs (portable-режим).
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn run(renderer: eframe::Renderer) -> eframe::Result {
    let options = eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_title("PgListener")
            .with_inner_size([1200.0, 750.0])
            .with_min_inner_size([700.0, 400.0])
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: tray::icon_rgba(64),
                width: 64,
                height: 64,
            })),
        ..Default::default()
    };
    eframe::run_native(
        "PgListener",
        options,
        Box::new(|cc| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            let storage = storage::Storage::open(&app_dir().join("pglistener.db"))
                .map_err(|e| format!("Не удалось открыть pglistener.db: {e:#}"))?;
            Ok(Box::new(app::App::new(cc, rt, storage)))
        }),
    )
}

fn main() {
    // Сначала wgpu (на Windows — DirectX 12), при неудаче — OpenGL.
    let result = match run(eframe::Renderer::Wgpu) {
        Err(eframe::Error::Wgpu(e)) => {
            eprintln!("wgpu недоступен ({e}), переключаюсь на OpenGL");
            run(eframe::Renderer::Glow)
        }
        r => r,
    };
    if let Err(e) = result {
        rfd::MessageDialog::new()
            .set_title("PgListener")
            .set_description(e.to_string())
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}
