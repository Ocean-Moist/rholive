use egui::{Color32, Context, FontId, RichText, Stroke, Vec2};
use egui_window_glfw_passthrough::{glfw, GlfwBackend, GlfwConfig};
use egui_window_glfw_passthrough::glfw::Context as GlfwContext;
use std::sync::{Arc, Mutex};
use glow;
use glow::HasContext;
use egui_glow; // Add egui_glow for the Painter

pub struct UiState {
    /// Whether the audio is currently muted
    pub is_muted: bool,
    /// Current AI response text
    pub ai_response: String,
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
            })),
        }
    }

    /// Get a clone of the state for use in the main application
    pub fn get_state_handle(&self) -> Arc<Mutex<UiState>> {
        self.state.clone()
    }

    /// Update the AI response text
    pub fn update_response(&self, response: String) {
        if let Ok(mut state) = self.state.lock() {
            state.ai_response = response;
        }
    }

    /// Run the UI application
    pub fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let config = GlfwConfig {
            window_title: "RhoLive Assistant".to_string(),
            size: [600, 800], // Larger window
            transparent_window: Some(true), // Enable transparency
            opengl_window: Some(true),
            glfw_callback: Box::new(|glfw: &mut glfw::Glfw| {
                glfw.window_hint(glfw::WindowHint::Decorated(false));
                glfw.window_hint(glfw::WindowHint::Floating(true));
                glfw.window_hint(glfw::WindowHint::Resizable(false));
                glfw.window_hint(glfw::WindowHint::TransparentFramebuffer(true));
                glfw.window_hint(glfw::WindowHint::AlphaBits(Some(8)));
            }),
            window_callback: Box::new(|window| {
                window.set_opacity(0.9); // Slight transparency for entire window
            }),
        };

        let mut backend = GlfwBackend::new(config);
        let mut ctx = Context::default();
        
        // Set up dark theme with glass effect
        configure_style(&mut ctx);

        let state = self.state;
        
        // Create a GL context for rendering
        let gl = unsafe {
            let gl = glow::Context::from_loader_function(|s| {
                backend.window.get_proc_address(s) as *const _
            });
            gl
        };
        
        // Main event loop
        while !backend.window.should_close() {
            // Poll events
            backend.glfw.poll_events();
            
            // Get input state
            let raw_input = backend.take_raw_input();
            
            // UI frame
            let output = ctx.run(raw_input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(Color32::from_rgba_premultiplied(20, 20, 30, 180)))
                    .show(ctx, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(10.0);
                            ui.heading(RichText::new("RhoLive Assistant").color(Color32::from_rgb(220, 220, 255)).size(24.0));
                            ui.add_space(5.0);

                            // Glass divider
                            ui.add(egui::Separator::default().spacing(5.0));
                            ui.add_space(10.0);
                            
                            // AI Response area (scrollable)
                            let response_text = if let Ok(state) = state.lock() {
                                state.ai_response.clone()
                            } else {
                                "Error accessing state".to_string()
                            };

                            let scroll_height = ui.available_height() - 60.0; // Reserve space for button
                            
                            egui::ScrollArea::vertical()
                                .max_height(scroll_height)
                                .show(ui, |ui| {
                                    ui.add_space(5.0);
                                    let _text_area = egui::Frame::none()
                                        .fill(Color32::from_rgba_premultiplied(40, 40, 60, 220))
                                        .rounding(8.0)
                                        .stroke(Stroke::new(1.0, Color32::from_rgb(100, 100, 180)))
                                        .shadow(egui::epaint::Shadow {
                                            offset: Vec2::new(0.0, 2.0),
                                            blur: 5.0,
                                            color: Color32::from_black_alpha(100),
                                        })
                                        .inner_margin(10.0)
                                        .show(ui, |ui| {
                                            ui.add_sized(
                                                Vec2::new(ui.available_width(), ui.available_height()),
                                                egui::TextEdit::multiline(&mut response_text.as_str())
                                                    .desired_width(f32::INFINITY)
                                                    .font(FontId::proportional(16.0))
                                                    .text_color(Color32::from_rgb(220, 220, 255))
                                                    .frame(false)
                                            );
                                        });
                                });
                            
                            ui.add_space(10.0);
                            
                            // Bottom buttons area
                            let button_height = 40.0;
                            ui.horizontal(|ui| {
                                let mut is_muted = if let Ok(state) = state.lock() {
                                    state.is_muted
                                } else {
                                    false
                                };

                                let mute_button = ui.add_sized(
                                    Vec2::new(ui.available_width(), button_height),
                                    egui::Button::new(
                                        RichText::new(
                                            if is_muted { "🔇 Unmute Audio" } else { "🔊 Mute Audio" }
                                        ).size(18.0)
                                    )
                                    .fill(if is_muted {
                                        Color32::from_rgba_premultiplied(80, 40, 40, 230)
                                    } else {
                                        Color32::from_rgba_premultiplied(40, 60, 80, 230)
                                    })
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(100, 100, 150)))
                                    .rounding(8.0)
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
            
            // Handle egui output (clipboard, cursor, etc)
            if output.textures_delta.is_empty() && output.shapes.is_empty() {
                // Nothing to render, skip this frame
                continue;
            }
            
            // Paint the UI
            let clipped_primitives = ctx.tessellate(output.shapes, 1.0);
            
            // Clear the framebuffer with a transparent background
            unsafe {
                // Set completely transparent background
                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                
                // Enable blending for transparency
                gl.enable(glow::BLEND);
                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            }
            
            // Create a painter to render the UI with proper transparency
            let painter = egui_glow::Painter::new(
                unsafe { Arc::new(glow::Context::from_loader_function(|s| {
                    backend.window.get_proc_address(s) as *const _
                })) },
                "", // shader prefix
                None // default shader version
            ).expect("Failed to create egui_glow Painter");
            
            // Get the physical size of the framebuffer
            let [fb_width, fb_height] = [backend.window.get_framebuffer_width() as u32, 
                                         backend.window.get_framebuffer_height() as u32];
                                         
            // Calculate the scale factor
            let scale_factor = backend.window.get_content_scale().0;
            
            // Paint the primitives using our painter
            painter.paint_and_update_textures(
                [fb_width, fb_height],
                scale_factor,
                &clipped_primitives,
                &output.textures_delta,
            ).expect("Failed to paint UI");
            
            // Swap buffers to present the frame
            backend.window.swap_buffers();
        }
        
        // Clean up GL resources
        unsafe {
            gl.finish();
        }
        
        Ok(())
    }
}

/// Configure egui visual style to have a glass-like dark theme
fn configure_style(ctx: &mut Context) {
    let mut style = (*ctx.style()).clone();
    
    // Set dark theme with glass-like effects
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = Color32::from_rgba_premultiplied(20, 20, 30, 180);
    style.visuals.window_fill = Color32::from_rgba_premultiplied(20, 20, 30, 220);
    
    // Update shadow properties
    style.visuals.window_shadow.color = Color32::from_black_alpha(80);
    
    // Text colors
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 220, 255));
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(180, 180, 200));
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::from_rgb(240, 240, 255));
    style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, Color32::from_rgb(255, 255, 255));
    
    // Button styling
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgba_premultiplied(60, 60, 80, 200);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(100, 100, 150));
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgba_premultiplied(70, 70, 100, 220);
    style.visuals.widgets.active.bg_fill = Color32::from_rgba_premultiplied(80, 80, 120, 250);
    
    let mut widgets = style.visuals.widgets.clone();
    widgets.noninteractive.rounding = egui::Rounding::from(8.0);
    widgets.inactive.rounding = egui::Rounding::from(8.0);
    widgets.hovered.rounding = egui::Rounding::from(8.0);
    widgets.active.rounding = egui::Rounding::from(8.0);
    style.visuals.widgets = widgets;
    
    ctx.set_style(style);
}

/// Integration function to run the UI in a separate thread
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