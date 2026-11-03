//! sparkeys — a native piano MIDI visualizer with glowing falling notes,
//! particles and a live MIDI input mode.

mod app;
mod audio;
mod config;
mod live;
mod midi;
mod render;
mod timeline;
mod ui;

use winit::event_loop::EventLoop;

/// Shows a dialog for any panic (on top of the default report) so a bug never
/// makes the window vanish without explanation.
fn install_panic_dialog() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        log::error!("panic: {info}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("sparkeys crashed")
            .set_description(format!("An unexpected error occurred:\n\n{info}\n\nPlease report this."))
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }));
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("sparkeys=info,warn")).init();

    install_panic_dialog();
    let event_loop = EventLoop::new()?;
    let mut app = app::App::new(std::env::args_os().nth(1).map(Into::into));
    event_loop.run_app(&mut app)?;

    match app.into_fatal_error() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}
