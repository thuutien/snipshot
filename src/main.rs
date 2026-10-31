mod app;
mod capture;
mod editor;
mod icons;
mod overlay;
mod raise;
mod raster;
mod settings;
mod text;
mod updater;

use std::time::Duration;

use app::{SnipApp, SnipMode};
use eframe::egui;

const HELP: &str = "\
Snipshot - take a snip of your screen and mark it up

Usage: snipshot [MODE] [--delay SECONDS]

With no mode, opens the Snipshot window. With a mode, starts a snip right
away (handy for a keyboard shortcut).

Modes:
  -r, --rect         Rectangle snip
  -w, --window       Window snip
  -f, --fullscreen   Full-screen snip
  -l, --freeform     Freeform snip

Options:
  -d, --delay N      Wait N seconds before snipping
  -h, --help         Show this help
  -V, --version      Show the version
";

fn main() -> eframe::Result {
    let mut mode = None;
    let mut delay = 0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-r" | "--rect" | "--rectangle" => mode = Some(SnipMode::Rectangle),
            "-w" | "--window" => mode = Some(SnipMode::Window),
            "-f" | "--fullscreen" => mode = Some(SnipMode::Fullscreen),
            "-l" | "--freeform" => mode = Some(SnipMode::Freeform),
            "-d" | "--delay" => match args.next().and_then(|v| v.parse().ok()) {
                Some(seconds) => delay = seconds,
                None => usage_error("--delay needs a number of seconds"),
            },
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("snipshot {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => usage_error(&format!("unknown option '{other}'")),
        }
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Snipshot")
        // Inside a Flatpak the app id must match the exported desktop file so
        // docks and window lists show the right icon.
        .with_app_id(std::env::var("FLATPAK_ID").unwrap_or_else(|_| "snipshot".to_owned()))
        .with_inner_size([600.0, 210.0])
        .with_min_inner_size([520.0, 180.0])
        .with_icon(app_icon());

    // From the command line, capture before opening any window so the
    // snip overlay is the first thing on screen.
    let start = mode.map(|mode| {
        std::thread::sleep(Duration::from_secs(delay as u64));
        match capture::capture(mode.area()) {
            Ok(capture) => (mode, capture),
            Err(e) => {
                eprintln!("snipshot: {e}");
                std::process::exit(1);
            }
        }
    });
    if let Some((mode, capture)) = &start
        && *mode != SnipMode::Fullscreen
    {
        viewport = viewport
            .with_decorations(false)
            .with_position([capture.origin.0 as f32, capture.origin.1 as f32])
            .with_fullscreen(true)
            .with_always_on_top();
    }

    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Snipshot", options, Box::new(move |cc| {
        text::register_font(&cc.egui_ctx);
        let window_id = raise::x11_window_id(cc);
        Ok(Box::new(SnipApp::new(start, delay, window_id)))
    }))
}

fn app_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/snipshot-256.png"))
        .expect("embedded icon is a valid PNG")
        .to_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData { rgba: image.into_raw(), width, height }
}

fn usage_error(message: &str) -> ! {
    eprintln!("snipshot: {message}\n\n{HELP}");
    std::process::exit(2);
}
