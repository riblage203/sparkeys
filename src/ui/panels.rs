//! The overlay panels drawn each frame.

/// Everything the overlay needs to display this frame.
pub struct OverlayState<'a> {
    /// Whether the main panels are visible (H).
    pub ui_visible: bool,
    /// Whether the FPS counter is visible (F3).
    pub show_fps: bool,
    /// Smoothed frames per second.
    pub fps: f32,
    /// Smoothed frame time in milliseconds.
    pub frame_ms: f32,
    /// GPU adapter description.
    pub adapter: &'a str,
    /// Messages for the user (invalid config, missing devices, ...). Dismissed entries are removed.
    pub notices: &'a mut Vec<String>,
}

/// Draws the overlay into the root `ui`.
pub fn draw_overlay(ui: &mut egui::Ui, state: &mut OverlayState<'_>) {
    let ctx = ui.ctx().clone();

    if state.show_fps {
        egui::Area::new(egui::Id::unique("fps"))
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
            .interactable(false)
            .show(&ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("{:.0} fps  {:.2} ms", state.fps, state.frame_ms))
                        .monospace()
                        .color(egui::Color32::from_gray(200)),
                );
            });
    }

    if !state.notices.is_empty() {
        egui::Window::new("Notice")
            .anchor(egui::Align2::CENTER_TOP, [0.0, 12.0])
            .collapsible(false)
            .resizable(false)
            .show(&ctx, |ui| {
                let mut dismiss = None;
                for (i, msg) in state.notices.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(255, 170, 90), msg);
                        if ui.small_button("✕").clicked() {
                            dismiss = Some(i);
                        }
                    });
                }
                if let Some(i) = dismiss {
                    state.notices.remove(i);
                }
            });
    }

    if !state.ui_visible {
        return;
    }

    egui::Window::new("sparkeys").default_pos([16.0, 16.0]).resizable(false).show(&ctx, |ui| {
        ui.label(format!("GPU: {}", state.adapter));
        ui.separator();
        ui.label("H  hide / show UI");
        ui.label("F3  FPS counter");
        ui.label("F11  fullscreen");
    });
}
