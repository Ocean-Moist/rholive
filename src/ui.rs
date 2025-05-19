use egui::{Color32, Context, FontId, RichText, Stroke, Vec2};
use egui_glow::Painter;
use egui_window_glfw_passthrough::glfw::Context as GlfwContext;
use egui_window_glfw_passthrough::{glfw, GlfwBackend, GlfwConfig};
use glow;
use std::sync::{Arc, Mutex};

pub struct UiState {
    /// Whether the audio is currently muted
    pub is_muted: bool,
    /// Current AI response text
    pub ai_response: String,
    /// Active audio device
    pub audio_device: Option<String>,
    /// List of available audio devices
    pub audio_devices: Vec<(String, String)>, // (name, description)
}

pub struct UiApp {
    /// Shared state between UI and main application
    state: Arc<Mutex<UiState>>,
}

impl UiApp {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(UiState {
                is_muted: false,
                ai_response: String::new(),
                audio_device: None,
                audio_devices: Vec::new(),
            })),
        }
    }

    /// Get a clone of the state for use in the main application
    pub fn get_state_handle(&self) -> Arc<Mutex<UiState>> {
        self.state.clone()
    }

    /// Run the UI application
    pub fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // Create a GLFW config that uses transparency
        let config = GlfwConfig {
            window_title: "RhoLive Assistant".to_string(),
            size: [600, 800], // Larger window
            transparent_window: Some(true),
            opengl_window: Some(true),
            glfw_callback: Box::new(|glfw: &mut glfw::Glfw| {
                glfw.window_hint(glfw::WindowHint::Decorated(false));
                glfw.window_hint(glfw::WindowHint::Floating(true));
                glfw.window_hint(glfw::WindowHint::Resizable(false));
                glfw.window_hint(glfw::WindowHint::TransparentFramebuffer(true));
                glfw.window_hint(glfw::WindowHint::AlphaBits(Some(8)));
            }),
            window_callback: Box::new(|window| {
                window.set_opacity(0.9); // Set window opacity
            }),
        };

        // Create the backend with our config
        let mut backend = GlfwBackend::new(config);

        // Set up glow renderer for egui
        let gl = unsafe {
            let gl = egui_glow::glow::Context::from_loader_function(|s| {
                backend.window.get_proc_address(s) as *const _
            });
            Arc::new(gl)
        };

        // Create painter for egui
        let mut painter = Painter::new(gl, "", None, false).expect("Failed to create painter");

        // Create egui context
        let mut ctx = Context::default();

        // Set up dark theme with glass-like effect
        configure_style(&mut ctx);

        // Get a clone of the shared state
        let state = self.state;

        // Main event loop
        while !backend.window.should_close() {
            // Poll events and get input
            backend.glfw.poll_events();
            let raw_input = backend.take_raw_input();

            // Process for ESC key to close window
            if raw_input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            }) {
                backend.window.set_should_close(true);
            }

            // Begin the UI frame
            let output = ctx.run(raw_input, |ctx| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::none().fill(Color32::from_rgba_premultiplied(20, 20, 30, 200)),
                    )
                    .show(ctx, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(10.0);
                            ui.heading(
                                RichText::new("RhoLive Assistant")
                                    .color(Color32::from_rgb(220, 220, 255))
                                    .size(24.0),
                            );
                            ui.add_space(5.0);

                            // Divider
                            ui.add(egui::Separator::default().spacing(5.0));
                            ui.add_space(10.0);

                            // AI Response area (scrollable)
                            let response_text = if let Ok(state) = state.lock() {
                                state.ai_response.clone()
                            } else {
                                "Error accessing state".to_string()
                            };

                            let scroll_height = ui.available_height() - 60.0; // Reserve space for button

                            // Create a ScrollArea with scroll-to-bottom behavior
                            egui::ScrollArea::vertical()
                                .max_height(scroll_height)
                                .auto_shrink([false; 2])
                                .stick_to_bottom(true) // Always scroll to bottom with new content
                                .show(ui, |ui| {
                                    ui.add_space(5.0);
                                    let _text_area = egui::Frame::none()
                                        .fill(Color32::from_rgba_premultiplied(40, 40, 60, 220))
                                        .rounding(8.0)
                                        .stroke(Stroke::new(1.0, Color32::from_rgb(100, 100, 180)))
                                        .inner_margin(10.0)
                                        .show(ui, |ui| {
                                            // Use a Label instead of TextEdit for better scrolling behavior
                                            ui.add_sized(
                                                Vec2::new(
                                                    ui.available_width(),
                                                    ui.available_height(),
                                                ),
                                                egui::Label::new(
                                                    RichText::new(&response_text)
                                                        .font(FontId::proportional(16.0))
                                                        .color(Color32::from_rgb(220, 220, 255))
                                                        .text_style(egui::TextStyle::Body),
                                                )
                                                .wrap(),
                                            );
                                        });
                                });

                            ui.add_space(10.0);

                            // Bottom buttons area
                            let button_height = 40.0;
                            // Show active device if available
                            let audio_device_name = if let Ok(state) = state.lock() {
                                state.audio_device.clone()
                            } else {
                                None
                            };

                            if let Some(device_name) = audio_device_name {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("🎤 Active: {}", device_name))
                                            .size(14.0)
                                            .color(Color32::from_rgb(180, 220, 255)),
                                    );
                                });
                                ui.add_space(5.0);
                            }

                            ui.horizontal(|ui| {
                                let mut is_muted = if let Ok(state) = state.lock() {
                                    state.is_muted
                                } else {
                                    false
                                };

                                let mute_button = ui.add_sized(
                                    Vec2::new(ui.available_width(), button_height),
                                    egui::Button::new(
                                        RichText::new(if is_muted {
                                            "🔇 Unmute Audio"
                                        } else {
                                            "🔊 Mute Audio"
                                        })
                                        .size(18.0),
                                    )
                                    .fill(if is_muted {
                                        Color32::from_rgba_premultiplied(80, 40, 40, 230)
                                    } else {
                                        Color32::from_rgba_premultiplied(40, 60, 80, 230)
                                    })
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(100, 100, 150)))
                                    .rounding(8.0),
                                );

                                if mute_button.clicked() {
                                    is_muted = !is_muted;
                                    if let Ok(mut state) = state.lock() {
                                        state.is_muted = is_muted;
                                    }
                                }
                            });
                        });
                    });
            });

            // Paint the UI using egui_glow painter
            let clipped_primitives = ctx.tessellate(output.shapes, output.pixels_per_point);

            // Get the physical size
            let (fb_width, fb_height) = backend.window.get_framebuffer_size();
            painter.paint_and_update_textures(
                [fb_width as u32, fb_height as u32],
                output.pixels_per_point,
                &clipped_primitives,
                &output.textures_delta,
            );

            // Swap buffers to present the frame
            backend.window.swap_buffers();

            // Sleep to reduce CPU usage if no need for immediate repaint
            let needs_repaint = ctx.has_requested_repaint();
            if !needs_repaint {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        Ok(())
    }
}

/// Configure egui visual style for a glass-like dark theme
fn configure_style(ctx: &mut Context) {
    let mut style = (*ctx.style()).clone();

    // Set dark theme
    style.visuals.dark_mode = true;

    // Semi-transparent backgrounds
    style.visuals.panel_fill = Color32::from_rgba_premultiplied(20, 20, 30, 120);
    style.visuals.window_fill = Color32::from_rgba_premultiplied(20, 20, 30, 180);
    style.visuals.extreme_bg_color = Color32::from_rgba_premultiplied(0, 0, 0, 0);

    // Text colors
    style.visuals.widgets.noninteractive.fg_stroke =
        Stroke::new(1.0, Color32::from_rgb(240, 240, 255));
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 220, 240));
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::from_rgb(255, 255, 255));
    style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, Color32::from_rgb(255, 255, 255));

    // Button styling
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgba_premultiplied(60, 60, 100, 180);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(120, 120, 200));
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgba_premultiplied(80, 80, 120, 200);
    style.visuals.widgets.active.bg_fill = Color32::from_rgba_premultiplied(100, 100, 160, 220);

    // Round all corners
    let mut widgets = style.visuals.widgets.clone();
    widgets.noninteractive.rounding = egui::Rounding::from(8.0);
    widgets.inactive.rounding = egui::Rounding::from(8.0);
    widgets.hovered.rounding = egui::Rounding::from(8.0);
    widgets.active.rounding = egui::Rounding::from(8.0);
    style.visuals.widgets = widgets;

    // Set spacings
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(16.0);

    ctx.set_style(style);
}

/// Launch the UI in a separate thread
pub fn launch_ui() -> Arc<Mutex<UiState>> {
    let app = UiApp::new();
    let state_handle = app.get_state_handle();

    // Launch UI in a separate thread
    std::thread::spawn(move || {
        if let Err(error) = app.run() {
            eprintln!("UI error: {}", error);
        }
    });

    state_handle
}
