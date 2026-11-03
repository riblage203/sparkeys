//! Application shell: window lifecycle, input shortcuts and the frame loop.

mod audio;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::config::Config;

use crate::midi::{self, Song};
use crate::render::{Gpu, Renderer, SceneInput};
use crate::timeline::Transport;
use crate::ui::{EguiLayer, FALL_SECONDS_RANGE, OverlayState, draw_overlay};
use audio::AudioState;

/// Arrow-key seek step.
const SEEK_STEP_S: f64 = 5.0;
/// Longest frame delta fed to the transport, so a stall (window drag, file
/// dialog) doesn't make playback jump.
const MAX_FRAME_DT_S: f64 = 0.1;

/// Top-level winit application.
pub struct App {
    config: Config,
    notices: Vec<String>,
    transport: Transport,
    song: Option<Song>,
    show_tracks: bool,
    show_visual: bool,
    /// Changes whenever a different song is loaded.
    song_id: u64,
    audio: AudioState,
    /// File given on the command line, opened once the window exists.
    initial_file: Option<PathBuf>,
    running: Option<Running>,
    fatal: Option<anyhow::Error>,
}

/// State that exists once the window and GPU are up.
struct Running {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    egui: EguiLayer,
    start: Instant,
    stats: FrameStats,
}

impl App {
    /// Loads settings and prepares an app with no window yet (created on `resumed`).
    /// `initial_file` is a MIDI file to open at startup.
    pub fn new(initial_file: Option<PathBuf>) -> Self {
        let (mut config, warning) = Config::load();
        config.playback.fall_seconds =
            config.playback.fall_seconds.clamp(*FALL_SECONDS_RANGE.start(), *FALL_SECONDS_RANGE.end());
        config.audio.volume = config.audio.volume.clamp(0.0, 1.0);
        let mut notices: Vec<String> = warning.into_iter().collect();
        let audio = AudioState::new(&config.audio, &mut notices);
        Self {
            transport: Transport::new(config.playback.speed),
            config,
            notices,
            song: None,
            show_tracks: false,
            show_visual: false,
            song_id: 0,
            audio,
            initial_file,
            running: None,
            fatal: None,
        }
    }

    /// The unrecoverable startup error, if any, after the event loop exits.
    pub fn into_fatal_error(self) -> Option<anyhow::Error> {
        self.fatal
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<Running> {
        let attrs = Window::default_attributes()
            .with_title("sparkeys")
            .with_inner_size(LogicalSize::new(self.config.window.width, self.config.window.height))
            .with_min_inner_size(LogicalSize::new(640, 360));
        let window = Arc::new(event_loop.create_window(attrs)?);
        let gpu = Gpu::new(window.clone())?;
        let renderer = Renderer::new(&gpu);
        let egui = EguiLayer::new(&window, &gpu);
        Ok(Running { window, gpu, renderer, egui, start: Instant::now(), stats: FrameStats::new() })
    }

    /// Loads a MIDI file, replacing the current song. Errors become on-screen notices.
    fn open_file(&mut self, path: &Path) {
        match midi::load_song(path) {
            Ok(song) => {
                log::info!(
                    "loaded {}: {} notes, {} parts, {:.1} s",
                    path.display(),
                    song.notes.len(),
                    song.tracks.len(),
                    song.duration_s
                );
                self.transport.load(song.duration_s);
                self.song_id += 1;
                if let Some(running) = &self.running {
                    running.window.set_title(&format!("{} — sparkeys", song.title));
                }
                self.song = Some(song);
            }
            Err(err) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notices.push(format!("Could not open {name}: {err}"));
            }
        }
    }

    fn pick_soundfont(&mut self) {
        let picked =
            rfd::FileDialog::new().add_filter("SoundFont", &["sf2"]).set_title("Choose a SoundFont").pick_file();
        if let Some(path) = picked {
            self.audio.load_soundfont(path);
        }
    }

    fn pick_file(&mut self) {
        let picked =
            rfd::FileDialog::new().add_filter("MIDI files", &["mid", "midi"]).set_title("Open MIDI").pick_file();
        if let Some(path) = picked {
            self.open_file(&path);
        }
    }

    fn handle_shortcut(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let PhysicalKey::Code(code) = event.physical_key else { return };
        match code {
            KeyCode::ArrowLeft => self.transport.seek(self.transport.position() - SEEK_STEP_S),
            KeyCode::ArrowRight => self.transport.seek(self.transport.position() + SEEK_STEP_S),
            _ if event.repeat => {}
            KeyCode::Space if self.song.is_some() => self.transport.toggle(),
            KeyCode::Home => self.transport.seek(self.transport.start()),
            KeyCode::KeyH => self.config.ui.visible = !self.config.ui.visible,
            KeyCode::F3 => self.config.ui.show_fps = !self.config.ui.show_fps,
            KeyCode::F11 => {
                if let Some(running) = &self.running {
                    let window = &running.window;
                    let next = match window.fullscreen() {
                        Some(_) => None,
                        None => Some(Fullscreen::Borderless(None)),
                    };
                    window.set_fullscreen(next);
                }
            }
            _ => {}
        }
    }

    fn save_config(&mut self) {
        if let Some(running) = &self.running
            && running.window.fullscreen().is_none()
        {
            let size = running.window.inner_size().to_logical::<u32>(running.window.scale_factor());
            self.config.window.width = size.width;
            self.config.window.height = size.height;
        }
        self.config.playback.speed = self.transport.speed();
        if let Err(err) = self.config.save() {
            log::warn!("could not save settings: {err:#}");
        }
    }

    fn redraw(&mut self) {
        let Some(running) = &mut self.running else { return };
        let dt = running.stats.tick().min(MAX_FRAME_DT_S);
        self.audio.poll(&mut self.config.audio, &mut self.notices);
        self.audio.update_transport(
            &mut self.transport,
            self.song.as_ref(),
            self.song_id,
            self.config.audio.volume,
            dt,
        );

        let frame = match running.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                running.gpu.reconfigure();
                frame
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                running.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                running.gpu.reconfigure();
                running.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error while acquiring frame");
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let scene_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(running.gpu.scene_format()),
            ..Default::default()
        });
        let mut encoder =
            running.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        let scene = SceneInput {
            song: self.song.as_ref(),
            time_s: self.transport.position(),
            fall_seconds: self.config.playback.fall_seconds,
            dt_s: dt as f32,
            visual: &self.config.visual,
        };
        let time = running.start.elapsed().as_secs_f32();
        running.renderer.render(&running.gpu, &mut encoder, &scene_view, time, &scene);

        let renderer = &running.renderer;
        let track_color = |track: u16| renderer.track_color(track);
        let audio_status = self.audio.status();
        let mut overlay = OverlayState {
            ui_visible: self.config.ui.visible,
            show_fps: self.config.ui.show_fps,
            fps: running.stats.fps(),
            frame_ms: running.stats.frame_ms(),
            adapter: &running.gpu.adapter_info.name,
            notices: &mut self.notices,
            transport: &mut self.transport,
            song: self.song.as_mut(),
            fall_seconds: &mut self.config.playback.fall_seconds,
            show_tracks: &mut self.show_tracks,
            show_visual: &mut self.show_visual,
            visual: &mut self.config.visual,
            notes_drawn: running.renderer.notes_drawn(),
            track_color: &track_color,
            volume: &mut self.config.audio.volume,
            audio_status: &audio_status,
            sync_error_ms: self.audio.sync_error_ms(),
            open_requested: false,
            soundfont_requested: false,
        };
        let extra =
            running.egui.draw(&running.window, &running.gpu, &mut encoder, &view, |ui| draw_overlay(ui, &mut overlay));
        let open_requested = overlay.open_requested;
        let soundfont_requested = overlay.soundfont_requested;

        running.gpu.queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        running.window.pre_present_notify();
        running.gpu.queue.present(frame);
        running.window.request_redraw();

        if open_requested {
            self.pick_file();
        }
        if soundfont_requested {
            self.pick_soundfont();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(running) => {
                running.window.request_redraw();
                self.running = Some(running);
                if let Some(path) = self.initial_file.take() {
                    self.open_file(&path);
                }
            }
            Err(err) => {
                log::error!("startup failed: {err:#}");
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("sparkeys could not start")
                    .set_description(format!("{err:#}"))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                self.fatal = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed = match &mut self.running {
            Some(running) => running.egui.on_window_event(&running.window, &event),
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => {
                self.save_config();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(running) = &mut self.running {
                    running.gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::DroppedFile(path) => self.open_file(&path),
            WindowEvent::KeyboardInput { event, .. } if !consumed => self.handle_shortcut(&event),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

/// Frame timing: raw delta for the clock, smoothed for the FPS counter.
struct FrameStats {
    last: Instant,
    avg_dt: f32,
}

impl FrameStats {
    fn new() -> Self {
        Self { last: Instant::now(), avg_dt: 1.0 / 60.0 }
    }

    /// Marks a new frame and returns the seconds since the previous one.
    fn tick(&mut self) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.avg_dt += (dt as f32 - self.avg_dt) * 0.05;
        dt
    }

    fn fps(&self) -> f32 {
        1.0 / self.avg_dt.max(1e-6)
    }

    fn frame_ms(&self) -> f32 {
        self.avg_dt * 1000.0
    }
}
