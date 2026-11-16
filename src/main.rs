mod analysis;
mod audio;
mod decode;
mod keys;
mod library;
mod recorder;
mod ui;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};

fn main() {
    if let Err(e) = audio::start() {
        eprintln!("Failed to open audio output: {e}");
        std::process::exit(1);
    }

    let window = WindowBuilder::new()
        .with_title("DJ")
        .with_inner_size(LogicalSize::new(1440.0, 920.0))
        .with_min_inner_size(LogicalSize::new(1100.0, 700.0));

    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            Config::new()
                .with_window(window)
                .with_background_color((14, 15, 18, 255))
                .with_disable_context_menu(true),
        )
        .launch(ui::App);
}

#[cfg(test)]
mod tests;
