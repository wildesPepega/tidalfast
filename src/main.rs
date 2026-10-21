#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
//! tidalfast — a simple, native TIDAL client built with egui.

mod app;
mod audio;
mod cache;
mod config;
mod desktop;
mod discord;
mod eq;
mod import;
mod ipc;
mod link;
mod model;
mod net;
mod system;
mod tidal;
mod transition;
mod ui;
mod update;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,tidalfast=info")).init();

    update::clean_up();
    // a freshly updated copy starts a moment after the old one quits
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(ms) = args.iter().position(|a| a == "--delay-start").and_then(|i| args.get(i + 1)?.parse::<u64>().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(10_000)));
    }

    // `tidalfast <spotify/tidal link>`: hand over to a running instance if there is one.
    let link_arg = std::env::args().skip(1).find(|a| link::parse(a).is_some());
    // screenshot runs (debug builds) stay out of a running instance's way
    let screenshot = cfg!(debug_assertions) && std::env::var_os("TIDALFAST_SHOT").is_some();
    if !screenshot {
        if ipc::forward(link_arg.as_deref().unwrap_or("focus")) {
            return Ok(());
        }
        let handler = config::Settings::load().link_handler;
        std::thread::spawn(move || desktop::set_link_handler(handler));
    }

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1080.0, 720.0])
        .with_min_inner_size([960.0, 600.0])
        .with_title("tidalfast")
        .with_app_id("tidalfast");
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    eframe::run_native(
        "tidalfast",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, link_arg)))),
    )
}

/// The window/taskbar icon.
fn app_icon() -> Option<eframe::egui::IconData> {
    let image = image::load_from_memory(desktop::ICON_PNG).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some(eframe::egui::IconData { rgba: image.into_raw(), width, height })
}
