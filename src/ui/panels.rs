//! The overlay panels drawn each frame.

use egui::{Align2, Color32, RichText};

use crate::config::VisualConfig;
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
    /// Whether the visual settings window is open.
    pub show_visual: &'a mut bool,
    /// Visual settings, edited live.
    pub visual: &'a mut VisualConfig,
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

    if state.song.is_none() {
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

    egui::Window::new("Visual")
        .open(state.show_visual)
        .default_pos([16.0, 110.0])
        .resizable(false)
        .show(&ctx, |ui| visual_settings(ui, state.visual));

    if let Some(song) = state.song.as_deref_mut() {
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
    let transport = &mut *state.transport;
    ui.horizontal(|ui| {
        if ui.button("📂 Open MIDI").clicked() {
            state.open_requested = true;
        }
        let has_song = state.song.is_some();
        let label = if transport.is_playing() { "⏸ Pause" } else { "▶ Play" };
        if ui.add_enabled(has_song, egui::Button::new(label)).clicked() {
            transport.toggle();
        }
        ui.separator();

        let mut speed = transport.speed();
        let speed_slider = egui::Slider::new(&mut speed, MIN_SPEED..=MAX_SPEED)
            .step_by(0.05)
            .fixed_decimals(2)
            .suffix("×")
            .text("speed");
        if ui.add(speed_slider).changed() {
            transport.set_speed(speed);
        }
        ui.add(egui::Slider::new(state.fall_seconds, FALL_SECONDS_RANGE).step_by(0.1).suffix(" s").text("visible"));
        ui.add(egui::Slider::new(state.volume, 0.0..=1.0).show_value(false).text("volume"));
        ui.separator();
        ui.toggle_value(state.show_tracks, "Tracks");
        ui.toggle_value(state.show_visual, "Visual");
        if ui.button("🎹 SoundFont").on_hover_text(state.audio_status).clicked() {
            state.soundfont_requested = true;
        }
        ui.label(RichText::new(state.audio_status).small().color(Color32::from_gray(150)));

        if let Some(song) = state.song.as_deref() {
            ui.separator();
            ui.label(RichText::new(&song.title).color(Color32::from_gray(190)));
        }
    });

    if state.song.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        let pos = transport.position();
        ui.label(RichText::new(format_time(pos)).monospace());
        let time_w = 60.0;
        ui.spacing_mut().slider_width = (ui.available_width() - time_w).max(50.0);
        let mut seek = pos;
        let slider = egui::Slider::new(&mut seek, transport.start()..=transport.duration()).show_value(false);
        if ui.add(slider).changed() {
            transport.seek(seek);
        }
        ui.label(RichText::new(format_time(transport.duration())).monospace());
    });
}

fn visual_settings(ui: &mut egui::Ui, visual: &mut VisualConfig) {
    ui.add(egui::Slider::new(&mut visual.bloom, 0.0..=2.0).text("bloom"));
    ui.add(egui::Slider::new(&mut visual.particles, 0.0..=2.0).text("particles"));
    ui.add(egui::Slider::new(&mut visual.corner_radius, 0.0..=12.0).suffix(" px").text("corner radius"));
    ui.checkbox(&mut visual.show_hit_line, "hit line");
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
    use super::format_time;

    #[test]
    fn formats_times() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(75.9), "1:15");
        assert_eq!(format_time(-1.5), "-0:01");
    }
}
