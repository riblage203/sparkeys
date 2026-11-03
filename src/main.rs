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

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("sparkeys=info,warn")).init();

    let event_loop = EventLoop::new()?;
    let mut app = app::App::new(std::env::args_os().nth(1).map(Into::into));
    event_loop.run_app(&mut app)?;

    match app.into_fatal_error() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}
