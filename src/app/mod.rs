//! Application shell: window lifecycle, input shortcuts and the frame loop.

mod audio;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::config::Config;
use crate::live::{LiveInput, LiveState};

use crate::midi::{self, Song};
use crate::render::{Gpu, LiveView, Renderer, SceneInput};
use crate::timeline::Transport;
use crate::ui::{EguiLayer, FALL_SECONDS_RANGE, OverlayState, draw_overlay};
pub use audio::AudioState;

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
    show_settings: bool,
    /// Changes whenever a different song is loaded.
    song_id: u64,
    audio: AudioState,
    live: LiveInput,
    live_state: LiveState,
    modifiers: ModifiersState,
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
        let mut audio = AudioState::new(&config.audio, &mut notices);
        let mut live = LiveInput::new(config.live.device.clone());
        let mut live_state = LiveState::new();
        live.set_enabled(config.live.enabled, &mut live_state, &mut audio);
        Self {
            transport: Transport::new(config.playback.speed),
            config,
            notices,
            song: None,
            show_tracks: false,
            show_settings: false,
            song_id: 0,
            audio,
            live,
            live_state,
            modifiers: ModifiersState::empty(),
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

    /// Switches between song playback and live input.
    fn set_live_mode(&mut self, enabled: bool) {
        self.config.live.enabled = enabled;
        if enabled && self.transport.is_playing() {
            self.transport.toggle();
        }
        self.live.set_enabled(enabled, &mut self.live_state, &mut self.audio);
    }

    fn handle_shortcut(&mut self, event: &KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let pressed = event.state == ElementState::Pressed;
        let live = self.config.live.enabled;
        // The computer-keyboard input gets first pick (note keys, ←/→ octave).
        if live && self.live.key_event(code, pressed, event.repeat, self.modifiers.shift_key()) {
            return;
        }
        if !pressed {
            return;
        }
        // Loop keys follow the typed letter, so they are right on any layout.
        if !live
            && !event.repeat
            && let Key::Character(text) = &event.logical_key
        {
            match text.to_lowercase().as_str() {
                "a" => return self.transport.set_loop_a(),
                "b" => return self.transport.set_loop_b(),
                "l" => return self.transport.toggle_loop(),
                _ => {}
            }
        }
        match code {
            KeyCode::ArrowLeft if !live => self.transport.seek(self.transport.position() - SEEK_STEP_S),
            KeyCode::ArrowRight if !live => self.transport.seek(self.transport.position() + SEEK_STEP_S),
            _ if event.repeat => {}
            KeyCode::Space if !live && self.song.is_some() => self.transport.toggle(),
            KeyCode::Home if !live => self.transport.seek(self.transport.start()),
            // F1 too, because H is a note key when typing notes in live mode.
            KeyCode::KeyH | KeyCode::F1 => self.config.ui.visible = !self.config.ui.visible,
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
        if self.config.live.enabled {
            self.live.update(&mut self.live_state, &mut self.audio, &mut self.notices);
            self.live_state.prune(self.config.playback.fall_seconds + 1.0);
        }

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
            wgpu::CurrentSurfaceTexture::Outdated => {
                running.gpu.reconfigure();
                running.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                if let Err(err) = running.gpu.recreate_surface() {
                    log::error!("{err:#}");
                }
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

        let live_mode = self.config.live.enabled;
        let scene = SceneInput {
            song: if live_mode { None } else { self.song.as_ref() },
            time_s: self.transport.position(),
            fall_seconds: self.config.playback.fall_seconds,
            dt_s: dt as f32,
            visual: &self.config.visual,
            live: live_mode.then(|| LiveView {
                notes: self.live_state.notes(),
                hits: self.live_state.hits(),
                time_s: self.live_state.now_s(),
                pedal: self.live_state.pedal_down(),
            }),
            split_key: self.config.live.split_key,
        };
        let time = running.start.elapsed().as_secs_f32();
        running.renderer.render(&running.gpu, &mut encoder, &scene_view, time, &scene);
        self.live_state.clear_hits();
        let mut live_choice = live_mode;
        let mut midi_choice = self.live.wanted().map(str::to_owned);
        let mut audio_device = self.config.audio.output_device.clone();
        let mut buffer_frames = self.config.audio.buffer_frames;
        let settings_were_open = self.show_settings;

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
            show_settings: &mut self.show_settings,
            visual: &mut self.config.visual,
            display: &mut self.config.display,
            split_key: &mut self.config.live.split_key,
            audio_devices: self.audio.devices(),
            audio_device: &mut audio_device,
            buffer_frames: &mut buffer_frames,
            notes_drawn: running.renderer.notes_drawn(),
            track_color: &track_color,
            volume: &mut self.config.audio.volume,
            audio_status: &audio_status,
            sync_error_ms: self.audio.sync_error_ms(),
            live_mode: &mut live_choice,
            midi_choices: self.live.choices(),
            midi_choice: &mut midi_choice,
            midi_status: self.live.status(),
            midi_connected: self.live.is_connected(),
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
        if self.config.display.limit_fps {
            let fps = match self.config.display.max_fps {
                0 => running
                    .window
                    .current_monitor()
                    .and_then(|m| m.refresh_rate_millihertz())
                    .map_or(60, |mhz| mhz / 1000),
                n => n,
            };
            pace_frame(running.stats.frame_start(), fps);
        }
        running.window.request_redraw();

        if self.show_settings && !settings_were_open {
            self.audio.refresh_devices();
        }
        if audio_device != self.config.audio.output_device || buffer_frames != self.config.audio.buffer_frames {
            self.config.audio.output_device = audio_device.clone();
            self.config.audio.buffer_frames = buffer_frames;
            self.audio.reconfigure(audio_device, buffer_frames, &mut self.notices);
        }
        if live_choice != live_mode {
            self.set_live_mode(live_choice);
        }
        if midi_choice.as_deref() != self.live.wanted() {
            self.live.select(midi_choice, &mut self.live_state, &mut self.audio);
        }
        self.config.live.device = self.live.wanted().map(str::to_owned);

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
            WindowEvent::DroppedFile(path) => {
                if self.config.live.enabled {
                    self.set_live_mode(false);
                }
                self.open_file(&path);
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Focused(false) => self.live.release_keys(),
            WindowEvent::KeyboardInput { event, .. } if !consumed => self.handle_shortcut(&event),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

/// Sleeps until `fps` would allow the next frame, measured from `frame_start`.
/// Sleeps coarsely, then spins the last millisecond for an even cadence.
fn pace_frame(frame_start: Instant, fps: u32) {
    let budget = std::time::Duration::from_secs_f64(1.0 / f64::from(fps.clamp(10, 1000)));
    let deadline = frame_start + budget;
    let coarse = deadline.saturating_duration_since(Instant::now()).saturating_sub(std::time::Duration::from_millis(1));
    if !coarse.is_zero() {
        std::thread::sleep(coarse);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
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

    /// When the current frame started (last [`Self::tick`]).
    fn frame_start(&self) -> Instant {
        self.last
    }

    fn fps(&self) -> f32 {
        1.0 / self.avg_dt.max(1e-6)
    }

    fn frame_ms(&self) -> f32 {
        self.avg_dt * 1000.0
    }
}
