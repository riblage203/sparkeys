//! The overlay panels drawn each frame.

use egui::{Align2, Color32, RichText};

use crate::config::{ColorMode, DisplayConfig, PaletteKind, VisualConfig};
use crate::midi::Song;
use crate::timeline::{MAX_SPEED, MIN_SPEED, Transport};

/// Range of the "fall speed" setting: seconds of music visible above the hit line.
pub const FALL_SECONDS_RANGE: std::ops::RangeInclusive<f64> = 0.5..=10.0;

/// Everything the overlay reads and edits this frame.
pub struct OverlayState<'a> {
    /// Whether the main panels are visible (H).
    pub ui_visible: bool,
    /// Whether the FPS counter is visible (F3).
    pub show_fps: bool,
    /// Smoothed frames per second.
    pub fps: f32,
    /// Smoothed frame time in milliseconds.
    pub frame_ms: f32,
    /// GPU adapter name, shown with the FPS counter.
    pub adapter: &'a str,
    /// Messages for the user (invalid files, missing devices, ...). Dismissed entries are removed.
    pub notices: &'a mut Vec<String>,
    /// Playback state.
    pub transport: &'a mut Transport,
    /// The loaded song, if any (track visibility is edited in place).
    pub song: Option<&'a mut Song>,
    /// Seconds visible above the hit line.
    pub fall_seconds: &'a mut f64,
    /// Whether the track list window is open.
    pub show_tracks: &'a mut bool,
    /// Whether the settings window is open.
    pub show_settings: &'a mut bool,
    /// Visual settings, edited live.
    pub visual: &'a mut VisualConfig,
    /// Frame pacing settings.
    pub display: &'a mut DisplayConfig,
    /// Hand split key (MIDI note).
    pub split_key: &'a mut u8,
    /// Output devices to choose from.
    pub audio_devices: &'a [String],
    /// Chosen output device (`None` = system default).
    pub audio_device: &'a mut Option<String>,
    /// Requested audio buffer size in frames.
    pub buffer_frames: &'a mut u32,
    /// Note bars drawn last frame (stats overlay).
    pub notes_drawn: usize,
    /// Linear RGB color of a part, for swatches.
    pub track_color: &'a dyn Fn(u16) -> [f32; 3],
    /// Output volume, 0..=1.
    pub volume: &'a mut f32,
    /// SoundFont and output device description.
    pub audio_status: &'a str,
    /// Visual minus audio clock in ms, shown with the FPS counter.
    pub sync_error_ms: Option<f64>,
    /// Live mode on/off (edited by the mode switch).
    pub live_mode: &'a mut bool,
    /// Live input devices to choose from.
    pub midi_choices: &'a [String],
    /// Chosen live input device.
    pub midi_choice: &'a mut Option<String>,
    /// Live input status line.
    pub midi_status: &'a str,
    /// Whether a live input is open.
    pub midi_connected: bool,
    /// Set when the user asks to open a file; handled by the app after the frame.
    pub open_requested: bool,
    /// Set when the user asks to choose a SoundFont; handled by the app after the frame.
    pub soundfont_requested: bool,
}

/// Draws the overlay into the root `ui`.
pub fn draw_overlay(ui: &mut egui::Ui, state: &mut OverlayState<'_>) {
    let ctx = ui.ctx().clone();

    if state.show_fps {
        egui::Area::new(egui::Id::unique("fps")).anchor(Align2::RIGHT_TOP, [-12.0, 100.0]).interactable(false).show(
            &ctx,
            |ui| {
                ui.label(
                    RichText::new(format!("{:.0} fps  {:.2} ms", state.fps, state.frame_ms))
                        .monospace()
                        .color(Color32::from_gray(200)),
                );
                ui.label(
                    RichText::new(format!("{} notes", state.notes_drawn)).monospace().color(Color32::from_gray(170)),
                );
                if let Some(err) = state.sync_error_ms {
                    ui.label(RichText::new(format!("a/v {err:+.1} ms")).monospace().color(Color32::from_gray(170)));
                }
                ui.label(RichText::new(state.adapter).small().color(Color32::from_gray(130)));
            },
        );
    }

    notices(&ctx, state.notices);

    if *state.live_mode && !state.midi_connected {
        egui::Area::new(egui::Id::unique("live hint")).anchor(Align2::CENTER_CENTER, [0.0, -60.0]).show(&ctx, |ui| {
            ui.label(RichText::new("Plug in a MIDI keyboard").size(22.0).color(Color32::from_gray(170)));
            ui.label(RichText::new(state.midi_status).color(Color32::from_gray(130)));
        });
    } else if !*state.live_mode && state.song.is_none() {
        egui::Area::new(egui::Id::unique("empty hint")).anchor(Align2::CENTER_CENTER, [0.0, -60.0]).show(&ctx, |ui| {
            ui.label(RichText::new("Drop a .mid file here").size(22.0).color(Color32::from_gray(170)));
            if ui.button("Open MIDI…").clicked() {
                state.open_requested = true;
            }
        });
    }

    if !state.ui_visible {
        return;
    }

    egui::Panel::top("transport")
        .frame(egui::Frame::new().fill(Color32::from_black_alpha(170)).inner_margin(8.0))
        .show(ui, |ui| transport_bar(ui, state));

    let mut settings_open = *state.show_settings;
    egui::Window::new("Settings")
        .open(&mut settings_open)
        .default_pos([16.0, 110.0])
        .resizable(false)
        .show(&ctx, |ui| settings(ui, state));
    *state.show_settings = settings_open;

    if let Some(song) = state.song.as_deref_mut().filter(|_| !*state.live_mode) {
        egui::Window::new("Tracks")
            .open(state.show_tracks)
            .default_pos([16.0, 110.0])
            .resizable(false)
            .show(&ctx, |ui| track_list(ui, song, state.track_color));
    }
}

fn notices(ctx: &egui::Context, notices: &mut Vec<String>) {
    if notices.is_empty() {
        return;
    }
    egui::Window::new("Notice").anchor(Align2::CENTER_TOP, [0.0, 100.0]).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            let mut dismiss = None;
            for (i, msg) in notices.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(255, 170, 90), msg);
                    if ui.small_button("✕").clicked() {
                        dismiss = Some(i);
                    }
                });
            }
            if let Some(i) = dismiss {
                notices.remove(i);
            }
        },
    );
}

fn transport_bar(ui: &mut egui::Ui, state: &mut OverlayState<'_>) {
    ui.horizontal(|ui| {
        ui.selectable_value(state.live_mode, false, "▶ Playback");
        ui.selectable_value(state.live_mode, true, "🎹 Live");
        ui.separator();
        if *state.live_mode {
            live_controls(ui, state);
        } else {
            playback_controls(ui, state);
        }
        ui.add(egui::Slider::new(state.fall_seconds, FALL_SECONDS_RANGE).step_by(0.1).suffix(" s").text("visible"));
        ui.add(egui::Slider::new(state.volume, 0.0..=1.0).show_value(false).text("volume"));
        ui.separator();
        if !*state.live_mode {
            ui.toggle_value(state.show_tracks, "Tracks");
        }
        ui.toggle_value(state.show_settings, "⚙ Settings");
        if ui.button("🎹 SoundFont").on_hover_text(state.audio_status).clicked() {
            state.soundfont_requested = true;
        }
        ui.label(RichText::new(state.audio_status).small().color(Color32::from_gray(150)));

        if let Some(song) = state.song.as_deref().filter(|_| !*state.live_mode) {
            ui.separator();
            ui.label(RichText::new(&song.title).color(Color32::from_gray(190)));
        }
    });

    if !*state.live_mode && state.song.is_some() {
        seek_bar(ui, state.transport);
    }
}

fn live_controls(ui: &mut egui::Ui, state: &mut OverlayState<'_>) {
    let selected = state.midi_choice.clone().unwrap_or_else(|| "(first found)".to_owned());
    egui::ComboBox::from_id_salt("midi input").selected_text(selected).width(180.0).show_ui(ui, |ui| {
        ui.selectable_value(state.midi_choice, None, "(first found)");
        for name in state.midi_choices {
            ui.selectable_value(state.midi_choice, Some(name.clone()), name);
        }
    });
    ui.label(RichText::new(state.midi_status).small().color(Color32::from_gray(160))).on_hover_text(
        "Computer keyboard: Z…M / Q…P rows play notes, ←/→ change octave, Shift = loud, F1 hides the UI",
    );
    ui.separator();
}

fn playback_controls(ui: &mut egui::Ui, state: &mut OverlayState<'_>) {
    let transport = &mut *state.transport;
    if ui.button("📂 Open MIDI").clicked() {
        state.open_requested = true;
    }
    let label = if transport.is_playing() { "⏸ Pause" } else { "▶ Play" };
    if ui.add_enabled(state.song.is_some(), egui::Button::new(label)).clicked() {
        transport.toggle();
    }
    ui.separator();

    let mut speed = transport.speed();
    let speed_slider =
        egui::Slider::new(&mut speed, MIN_SPEED..=MAX_SPEED).step_by(0.05).fixed_decimals(2).suffix("×").text("speed");
    if ui.add(speed_slider).changed() {
        transport.set_speed(speed);
    }
    ui.separator();

    // Loop A–B: keys A / B set the points, L toggles.
    if ui.button("A").on_hover_text("Set loop start here  (A)").clicked() {
        transport.set_loop_a();
    }
    if ui.button("B").on_hover_text("Set loop end here and start looping  (B)").clicked() {
        transport.set_loop_b();
    }
    if let Some((a, b)) = transport.loop_points() {
        let mut on = transport.active_loop().is_some();
        if ui
            .toggle_value(&mut on, format!("⟲ {}–{}", format_time(a), format_time(b)))
            .on_hover_text("Loop on/off  (L)")
            .clicked()
        {
            transport.toggle_loop();
        }
        if ui.small_button("✕").on_hover_text("Clear loop").clicked() {
            transport.clear_loop();
        }
    }
}

fn seek_bar(ui: &mut egui::Ui, transport: &mut Transport) {
    ui.horizontal(|ui| {
        let pos = transport.position();
        ui.label(RichText::new(format_time(pos)).monospace());
        let time_w = 60.0;
        ui.spacing_mut().slider_width = (ui.available_width() - time_w).max(50.0);
        let mut seek = pos;
        let (start, end) = (transport.start(), transport.duration());
        let slider = egui::Slider::new(&mut seek, start..=end).show_value(false);
        let response = ui.add(slider);
        if response.changed() {
            transport.seek(seek);
        }
        // Shade the loop range on the bar.
        if let Some((a, b)) = transport.loop_points() {
            let r = response.rect;
            let span = (end - start).max(1e-6);
            let x = |t: f64| r.left() + ((t - start) / span) as f32 * r.width();
            let alpha = if transport.active_loop().is_some() { 70 } else { 30 };
            let band = egui::Rect::from_x_y_ranges(x(a)..=x(b), r.center().y - 5.0..=r.center().y + 5.0);
            ui.painter().rect_filled(band, 2.0, Color32::from_rgba_unmultiplied(255, 140, 60, alpha));
        }
        ui.label(RichText::new(format_time(transport.duration())).monospace());
    });
}

/// Buffer sizes offered in the audio settings.
const BUFFER_CHOICES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

fn settings(ui: &mut egui::Ui, state: &mut OverlayState<'_>) {
    egui::CollapsingHeader::new("Look").default_open(true).show(ui, |ui| look_settings(ui, state.visual));

    egui::CollapsingHeader::new("Audio").default_open(true).show(ui, |ui| {
        let current = state.audio_device.clone().unwrap_or_else(|| "System default".to_owned());
        egui::ComboBox::from_label("output").selected_text(current).width(260.0).show_ui(ui, |ui| {
            ui.selectable_value(state.audio_device, None, "System default");
            for name in state.audio_devices {
                ui.selectable_value(state.audio_device, Some(name.clone()), name);
            }
        });
        egui::ComboBox::from_label("buffer")
            .selected_text(format!("{} frames", state.buffer_frames))
            .show_ui(ui, |ui| {
                for n in BUFFER_CHOICES {
                    ui.selectable_value(state.buffer_frames, n, format!("{n} frames"));
                }
            })
            .response
            .on_hover_text("Smaller = lower latency (live mode) but more CPU; devices may enforce a minimum.");
        ui.label(RichText::new(state.audio_status).small().color(Color32::from_gray(150)));
        if ui.button("🎹 Choose SoundFont…").clicked() {
            state.soundfont_requested = true;
        }
    });

    egui::CollapsingHeader::new("Live").show(ui, |ui| {
        ui.add(
            egui::Slider::new(state.split_key, 21..=108).custom_formatter(|v, _| note_name(v as u8)).text("hand split"),
        )
        .on_hover_text("Keys below are colored as the left hand");
    });

    egui::CollapsingHeader::new("Display").show(ui, |ui| {
        ui.checkbox(&mut state.display.limit_fps, "limit frame rate")
            .on_hover_text("Saves power when the graphics driver ignores vsync. 0 = match the monitor's refresh rate.");
        ui.add_enabled(
            state.display.limit_fps,
            egui::Slider::new(&mut state.display.max_fps, 0..=360).text("max fps (0 = monitor)"),
        );
    });
}

fn look_settings(ui: &mut egui::Ui, visual: &mut VisualConfig) {
    egui::ComboBox::from_label("palette").selected_text(visual.palette.label()).show_ui(ui, |ui| {
        for kind in PaletteKind::ALL {
            ui.selectable_value(&mut visual.palette, kind, kind.label());
        }
    });
    ui.horizontal(|ui| {
        ui.label("color by");
        ui.selectable_value(&mut visual.color_mode, ColorMode::Track, "track");
        ui.selectable_value(&mut visual.color_mode, ColorMode::Hand, "hand");
    });
    ui.add(egui::Slider::new(&mut visual.bloom, 0.0..=2.0).text("bloom"));
    ui.add(egui::Slider::new(&mut visual.particles, 0.0..=2.0).text("particles"));
    ui.add(egui::Slider::new(&mut visual.corner_radius, 0.0..=12.0).suffix(" px").text("corner radius"));
    ui.checkbox(&mut visual.show_hit_line, "hit line");
    ui.checkbox(&mut visual.animated_background, "animated background");
}

/// Scientific pitch name, e.g. 60 → "C4".
fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[usize::from(pitch % 12)], i32::from(pitch / 12) - 1)
}

fn track_list(ui: &mut egui::Ui, song: &mut Song, track_color: &dyn Fn(u16) -> [f32; 3]) {
    for (i, track) in song.tracks.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let [r, g, b] = track_color(u16::try_from(i).unwrap_or(u16::MAX));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, Color32::from(egui::Rgba::from_rgb(r, g, b)));
            ui.checkbox(&mut track.visible, &track.name).on_hover_text("Show notes");
            let mute_label = if track.muted { "🔇" } else { "🔊" };
            if ui.selectable_label(track.muted, mute_label).on_hover_text("Mute").clicked() {
                track.muted = !track.muted;
            }
            ui.label(RichText::new(format!("{} notes", track.note_count)).color(Color32::from_gray(140)));
        });
    }
}

/// Formats seconds as `m:ss`, with a leading minus during the lead-in.
fn format_time(seconds: f64) -> String {
    let sign = if seconds < 0.0 { "-" } else { "" };
    let total = seconds.abs().floor() as u64;
    format!("{sign}{}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::{format_time, note_name};

    #[test]
    fn names_notes() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(21), "A0");
        assert_eq!(note_name(108), "C8");
    }

    #[test]
    fn formats_times() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(75.9), "1:15");
        assert_eq!(format_time(-1.5), "-0:01");
    }
}
