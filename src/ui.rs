use egui::{Color32, Context, FontId, RichText, Stroke, Vec2, Pos2, FontFamily, FontDefinitions};
use egui_glow::Painter;
use egui_window_glfw_passthrough::glfw::Context as GlfwContext;
use egui_window_glfw_passthrough::{glfw, GlfwBackend, GlfwConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Audio visualization sample
#[derive(Clone, Debug)]
pub struct AudioSample {
    pub level: f32,
    pub timestamp: Instant,
}

/// Conversation entry
#[derive(Clone, Debug)]
pub struct ConversationEntry {
    pub role: String, // "User" or "Gemini"
    pub text: String,
    pub timestamp: Instant,
}

pub struct UiState {
    /// Whether the audio is currently muted
    pub is_muted: bool,
    /// Current AI response being built
    pub current_ai_response: String,
    /// Conversation history
    pub conversation_history: VecDeque<ConversationEntry>,
    /// Active audio device
    pub audio_device: Option<String>,
    /// List of available audio devices
    pub audio_devices: Vec<(String, String)>, // (name, description)
    /// Current transcript being spoken
    pub current_transcript: String,
    /// Whether user is currently speaking
    pub is_speaking: bool,
    /// Number of segments processed
    pub segments_processed: u32,
    /// Number of frames sent to Gemini
    pub frames_sent: u32,
    /// Audio level samples for visualization
    pub audio_samples: VecDeque<AudioSample>,
    /// Connection status
    pub connected: bool,
    /// Show debug info
    pub show_debug: bool,
    /// Last status message
    pub status_message: String,
    /// UI collapsed state
    pub is_collapsed: bool,
    /// Last activity time for auto-collapse
    pub last_activity: Instant,
    /// Typewriter animation state
    pub typewriter_position: usize,
    pub typewriter_last_update: Instant,
}

pub struct UiApp {
    /// Shared state between UI and main application
    state: Arc<Mutex<UiState>>,
    /// Start time for runtime calculation
    start_time: Instant,
    /// Frame counter for FPS
    frame_count: u64,
    last_fps_update: Instant,
    fps: f32,
}

impl UiApp {
    pub fn new() -> Self {
        let mut ui_state = UiState {
            is_muted: false,
            current_ai_response: String::new(),
            conversation_history: VecDeque::with_capacity(100),
            audio_device: None,
            audio_devices: Vec::new(),
            current_transcript: String::new(),
            is_speaking: false,
            segments_processed: 0,
            frames_sent: 0,
            audio_samples: VecDeque::with_capacity(200),
            connected: false,
            show_debug: false,
            status_message: String::from("Ready to assist..."),
            is_collapsed: true, // Start collapsed
            last_activity: Instant::now(),
            typewriter_position: 0,
            typewriter_last_update: Instant::now(),
        };
        
        // Initialize with some flat audio samples
        let now = Instant::now();
        for i in 0..100 {
            ui_state.audio_samples.push_back(AudioSample {
                level: 0.0,
                timestamp: now - Duration::from_millis(i * 20),
            });
        }
        
        Self {
            state: Arc::new(Mutex::new(ui_state)),
            start_time: Instant::now(),
            frame_count: 0,
            last_fps_update: Instant::now(),
            fps: 0.0,
        }
    }

    /// Get a clone of the state for use in the main application
    pub fn get_state_handle(&self) -> Arc<Mutex<UiState>> {
        self.state.clone()
    }

    /// Run the UI application
    pub fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // Fixed dimensions for horizontal bar
        let window_width = 1400; // Wider
        let window_height = 100; // Taller initial height for visibility
        
        // Create a GLFW config that uses transparency
        let config = GlfwConfig {
            window_title: "RhoLive".to_string(),
            size: [window_width as u32, window_height as u32],
            transparent_window: Some(true),
            opengl_window: Some(true),
            glfw_callback: Box::new(|glfw: &mut glfw::Glfw| {
                glfw.window_hint(glfw::WindowHint::Decorated(false));
                glfw.window_hint(glfw::WindowHint::Floating(true));
                glfw.window_hint(glfw::WindowHint::Resizable(false));
                glfw.window_hint(glfw::WindowHint::TransparentFramebuffer(true));
                glfw.window_hint(glfw::WindowHint::AlphaBits(Some(8)));
                glfw.window_hint(glfw::WindowHint::DepthBits(Some(0)));
                glfw.window_hint(glfw::WindowHint::StencilBits(Some(0)));
                // Make sure window can receive focus
                glfw.window_hint(glfw::WindowHint::Focused(true));
                glfw.window_hint(glfw::WindowHint::FocusOnShow(true));
            }),
            window_callback: Box::new(move |window| {
                // Position window at bottom center
                // Default to 1920x1080 if we can't get monitor size
                let window_x = (1920 - window_width) / 2;
                let window_y = 1080 - window_height - 40; // 40px from bottom
                window.set_pos(window_x, window_y);
            }),
        };

        // Create the backend with our config
        let mut backend = GlfwBackend::new(config);
        backend.set_passthrough(false);
        // Enable event polling - CRUCIAL for receiving any events!
        backend.window.set_all_polling(true);
        
        // Make sure window can receive events
        backend.window.show();
        backend.window.focus();
        backend.window.set_mouse_passthrough(false);

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

        // Load custom fonts
        configure_fonts(&mut ctx);
        
        // Set up minimal glass-like theme
        configure_style(&mut ctx);
        
        // Increase scroll speed for better user experience
        ctx.options_mut(|o| {
            o.line_scroll_speed = 1200.0; // 3x faster than default (40.0)
        });

        // Get a clone of the shared state
        let state = self.state;
        let _start_time = self.start_time;
        let mut frame_count = self.frame_count;
        let mut last_fps_update = self.last_fps_update;
        let mut fps = self.fps;
        let mut current_height = window_height as f32;
        let target_collapsed_height = 60.0;
        let target_expanded_height = 280.0;

        // Main event loop
        while !backend.window.should_close() {
            // Update FPS counter
            frame_count += 1;
            if last_fps_update.elapsed() >= Duration::from_secs(1) {
                fps = frame_count as f32 / last_fps_update.elapsed().as_secs_f32();
                frame_count = 0;
                last_fps_update = Instant::now();
            }

            // Poll events and get input
            backend.glfw.poll_events();
            backend.tick();
            let raw_input = backend.take_raw_input();

            // Process keyboard shortcuts
            let mut toggle_collapse = false;
            let mut toggle_mute = false;
            
            // First check if window has focus
            if !backend.window.is_focused() {
                // Try to capture focus on mouse click
                if raw_input.events.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. })) {
                    backend.window.focus();
                }
            }
            
            for event in &raw_input.events {
                // Debug print key events
                if let egui::Event::Key { key, pressed, modifiers, .. } = event {
                    if *pressed {
                        eprintln!("Key pressed: {:?}, Shift: {}, Ctrl: {}, Cmd: {}", 
                                 key, modifiers.shift, modifiers.ctrl, modifiers.command);
                    }
                }
                
                match event {
                    egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                        backend.window.set_should_close(true);
                    }
                    egui::Event::Key { key: egui::Key::Space, pressed: true, modifiers, .. } => {
                        if modifiers.shift && modifiers.ctrl {
                            eprintln!("Toggle collapse triggered!");
                            toggle_collapse = true;
                        }
                    }
                    egui::Event::Key { key: egui::Key::M, pressed: true, modifiers, .. } => {
                        if modifiers.shift && modifiers.ctrl {
                            eprintln!("Toggle mute triggered!");
                            toggle_mute = true;
                        }
                    }
                    _ => {}
                }
            }

            // Handle state changes
            if toggle_collapse || toggle_mute {
                let mut state_guard = state.lock().unwrap();
                if toggle_collapse {
                    state_guard.is_collapsed = !state_guard.is_collapsed;
                    state_guard.last_activity = Instant::now();
                }
                if toggle_mute {
                    state_guard.is_muted = !state_guard.is_muted;
                }
            }

            // Check for auto-collapse (30 seconds of inactivity)
            {
                let mut state_guard = state.lock().unwrap();
                if !state_guard.is_collapsed && state_guard.last_activity.elapsed() > Duration::from_secs(30) {
                    state_guard.is_collapsed = true;
                }
            }

            // Animate height changes
            let is_collapsed = state.lock().unwrap().is_collapsed;
            let target_height = if is_collapsed { target_collapsed_height } else { target_expanded_height };
            current_height += (target_height - current_height) * 0.15; // Smooth animation
            
            // Update window size if needed
            if (current_height - target_height).abs() > 0.5 {
                backend.window.set_size(window_width, current_height as i32);
                // Re-position to keep bottom-anchored and horizontally centered
                let window_x = (1920 - window_width) / 2; // Keep centered horizontally
                let window_y = 1080 - (current_height as i32) - 40;
                backend.window.set_pos(window_x, window_y);
            }

            // Clear the framebuffer with transparency
            unsafe {
                use egui_glow::glow::HasContext;
                // Enable blending for transparency
                painter.gl().enable(egui_glow::glow::BLEND);
                painter.gl().blend_func(egui_glow::glow::SRC_ALPHA, egui_glow::glow::ONE_MINUS_SRC_ALPHA);
                // Clear to fully transparent
                painter.gl().clear_color(0.0, 0.0, 0.0, 0.0);
                painter.gl().clear(egui_glow::glow::COLOR_BUFFER_BIT);
            }

            // Begin the UI frame
            let output = ctx.run(raw_input, |ctx| {
                // Request continuous repaint for animations
                ctx.request_repaint();
                
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::none()
                            .fill(Color32::from_rgba_premultiplied(10, 10, 15, 120)) // Much more transparent
                            .inner_margin(egui::Margin::symmetric(30.0, 8.0))
                            .rounding(8.0),
                    )
                    .show(ctx, |ui| {
                        let mut state_guard = state.lock().unwrap();
                        
                        if state_guard.is_collapsed {
                            // Collapsed view - minimal height
                            // Make the entire area clickable
                            let response = ui.allocate_response(
                                ui.available_size(),
                                egui::Sense::click()
                            );
                            
                            if response.clicked() {
                                state_guard.is_collapsed = false;
                                eprintln!("Clicked to expand!");
                            }
                            
                            // Draw content on top
                            ui.allocate_ui_at_rect(response.rect, |ui| {
                                ui.horizontal(|ui| {
                                    // Status indicator
                                    let (icon, color) = if state_guard.connected {
                                        ("●", Color32::from_rgb(100, 255, 150))
                                    } else {
                                        ("○", Color32::from_rgb(100, 100, 100))
                                    };
                                    ui.label(RichText::new(icon).color(color).size(12.0));
                                    
                                    ui.add_space(10.0);
                                    
                                    // Activity indicator or status
                                    if state_guard.is_speaking {
                                        ui.label(RichText::new("Listening...").size(14.0).color(Color32::from_gray(200)));
                                    } else if !state_guard.current_ai_response.is_empty() {
                                        ui.label(RichText::new("Responding...").size(14.0).color(Color32::from_rgb(150, 220, 255)));
                                    } else {
                                        ui.label(RichText::new(&state_guard.status_message).size(14.0).color(Color32::from_gray(180)));
                                    }
                                    
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(RichText::new("Click to expand").size(11.0).color(Color32::from_gray(120)));
                                    });
                                });
                            });
                        } else {
                            // Expanded view - full horizontal layout
                            ui.vertical(|ui| {
                                // Top bar with status and controls
                                ui.horizontal(|ui| {
                                    // Status dot
                                    let (icon, color) = if state_guard.connected {
                                        ("●", Color32::from_rgb(100, 255, 150))
                                    } else {
                                        ("○", Color32::from_rgb(100, 100, 100))
                                    };
                                    ui.label(RichText::new(icon).color(color).size(14.0));
                                    
                                    ui.add_space(15.0);
                                    
                                    // Current activity or transcript
                                    if state_guard.is_speaking {
                                        let time = ui.ctx().input(|i| i.time) as f32;
                                        let pulse = (time * 3.0).sin() * 0.5 + 0.5;
                                        let color = Color32::from_rgb(
                                            (100.0 + 50.0 * pulse) as u8,
                                            255,
                                            (150.0 + 50.0 * pulse) as u8
                                        );
                                        ui.label(RichText::new("● Listening...").color(color).size(16.0));
                                    } else if !state_guard.current_transcript.is_empty() {
                                        ui.label(RichText::new(&state_guard.current_transcript).size(16.0).color(Color32::from_gray(220)));
                                    } else {
                                        ui.label(RichText::new(&state_guard.status_message).size(16.0).color(Color32::from_gray(180)));
                                    }
                                    
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        // Mute button
                                        let mute_text = if state_guard.is_muted { "🔇" } else { "🔊" };
                                        if ui.button(RichText::new(mute_text).size(18.0)).clicked() {
                                            state_guard.is_muted = !state_guard.is_muted;
                                        }
                                        
                                        ui.add_space(10.0);
                                        
                                        // Collapse button
                                        if ui.button(RichText::new("—").size(16.0)).clicked() {
                                            state_guard.is_collapsed = true;
                                        }
                                    });
                                });
                                
                                ui.add_space(10.0);
                                
                                // Thin separator line
                                ui.add(egui::Separator::default().spacing(2.0));
                                
                                ui.add_space(10.0);
                                
                                // Main content area with scrolling
                                egui::ScrollArea::vertical()
                                    .max_height(ui.available_height() - 60.0) // Leave room for bottom controls
                                    .auto_shrink([false; 2])
                                    .stick_to_bottom(true) // Auto-scroll to bottom
                                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                                    .animated(true)
                                    .show(ui, |ui| {
                                        if !state_guard.current_ai_response.is_empty() {
                                            // Update typewriter animation
                                            if state_guard.typewriter_last_update.elapsed() > Duration::from_millis(20) {
                                                state_guard.typewriter_position = state_guard.typewriter_position
                                                    .saturating_add(2)
                                                    .min(state_guard.current_ai_response.len());
                                                state_guard.typewriter_last_update = Instant::now();
                                                state_guard.last_activity = Instant::now();
                                            }
                                            
                                            let visible_text = &state_guard.current_ai_response[..state_guard.typewriter_position];
                                            
                                            // Parse and render text with code blocks
                                            render_text_with_code_blocks(ui, visible_text);
                                            
                                            // Show cursor if still typing
                                            if state_guard.typewriter_position < state_guard.current_ai_response.len() {
                                                let time = ui.ctx().input(|i| i.time) as f32;
                                                let cursor_alpha = ((time * 2.0).sin() + 1.0) * 0.5;
                                                ui.label(
                                                    RichText::new("│")
                                                        .size(18.0)
                                                        .color(Color32::from_rgba_premultiplied(
                                                            240, 240, 255, 
                                                            (255.0 * cursor_alpha) as u8
                                                        ))
                                                );
                                            }
                                        } else if state_guard.conversation_history.is_empty() {
                                            // Show placeholder when no content
                                            ui.vertical_centered(|ui| {
                                                ui.add_space(40.0);
                                                ui.label(
                                                    RichText::new("<nothing>")
                                                        .size(18.0)
                                                        .color(Color32::from_gray(100))
                                                        .italics()
                                                );
                                            });
                                        } else {
                                            // Show conversation history
                                            for entry in &state_guard.conversation_history {
                                                ui.group(|ui| {
                                                    ui.horizontal(|ui| {
                                                        if entry.role == "User" {
                                                            ui.label(RichText::new("👤").size(14.0));
                                                        } else {
                                                            ui.label(RichText::new("🤖").size(14.0));
                                                        }
                                                        ui.add_space(8.0);
                                                    });
                                                    render_text_with_code_blocks(ui, &entry.text);
                                                });
                                                ui.add_space(8.0);
                                            }
                                        }
                                    });
                                
                                // Bottom section with audio viz
                                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                                    ui.horizontal(|ui| {
                                        // Minimal audio visualization
                                        ui.allocate_ui(Vec2::new(300.0, 30.0), |ui| {
                                            draw_horizontal_audio_viz(ui, &state_guard.audio_samples, state_guard.is_speaking);
                                        });
                                        
                                        ui.add_space(20.0);
                                        
                                        // Stats (minimal)
                                        if state_guard.show_debug {
                                            ui.label(
                                                RichText::new(format!("Segments: {} | Frames: {} | FPS: {:.0}", 
                                                    state_guard.segments_processed, 
                                                    state_guard.frames_sent,
                                                    fps))
                                                    .size(11.0)
                                                    .color(Color32::from_gray(120))
                                            );
                                        }
                                    });
                                });
                            });
                        }
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

            // Sleep to reduce CPU usage
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        Ok(())
    }
}

/// Render text with code blocks formatted properly
fn render_text_with_code_blocks(ui: &mut egui::Ui, text: &str) {
    let parts: Vec<&str> = text.split("```").collect();
    
    for (i, part) in parts.iter().enumerate() {
        if i % 2 == 0 {
            // Regular text
            if !part.is_empty() {
                ui.label(
                    RichText::new(*part)
                        .size(16.0)
                        .color(Color32::from_rgb(240, 240, 255))
                );
            }
        } else {
            // Code block
            let lines: Vec<&str> = part.lines().collect();
            let lang = lines.first().unwrap_or(&"");
            let code = if lines.len() > 1 {
                lines[1..].join("\n")
            } else {
                part.to_string()
            };
            
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.visuals_mut().extreme_bg_color = Color32::from_rgba_premultiplied(30, 30, 40, 180);
                ui.visuals_mut().override_text_color = Some(Color32::from_rgb(220, 220, 240));
                
                // Language label if present
                if !lang.is_empty() {
                    ui.label(
                        RichText::new(*lang)
                            .size(12.0)
                            .color(Color32::from_rgb(150, 150, 170))
                            .italics()
                    );
                }
                
                // Code content with monospace font
                ui.label(
                    RichText::new(&code)
                        .size(14.0)
                        .font(FontId::new(14.0, FontFamily::Monospace))
                        .color(Color32::from_rgb(220, 220, 240))
                );
            });
        }
    }
}

/// Draw horizontal audio visualization
fn draw_horizontal_audio_viz(ui: &mut egui::Ui, samples: &VecDeque<AudioSample>, is_speaking: bool) {
    let rect = ui.available_rect_before_wrap();
    let painter = ui.painter_at(rect);
    
    // Very subtle background
    painter.rect_filled(
        rect,
        egui::Rounding::same(4.0),
        Color32::from_rgba_premultiplied(30, 30, 40, 50),
    );
    
    if samples.is_empty() {
        return;
    }
    
    // Draw minimal waveform
    let width = rect.width();
    let height = rect.height();
    let center_y = rect.center().y;
    
    let max_samples = 60;
    let samples_to_show: Vec<_> = samples.iter()
        .rev()
        .take(max_samples)
        .rev()
        .collect();
    
    if samples_to_show.len() > 1 {
        let x_step = width / (samples_to_show.len() - 1) as f32;
        
        let color = if is_speaking {
            Color32::from_rgba_premultiplied(100, 255, 150, 150)
        } else {
            Color32::from_rgba_premultiplied(100, 150, 255, 80)
        };
        
        for (i, sample) in samples_to_show.iter().enumerate() {
            let x = rect.left() + i as f32 * x_step;
            let amplitude = sample.level.min(1.0) * height * 0.3;
            
            // Draw vertical line from center
            painter.line_segment(
                [
                    Pos2::new(x, center_y - amplitude),
                    Pos2::new(x, center_y + amplitude)
                ],
                Stroke::new(1.5, color),
            );
        }
    }
    
    ui.allocate_rect(rect, egui::Sense::hover());
}

/// Configure custom fonts
fn configure_fonts(ctx: &mut Context) {
    let mut fonts = FontDefinitions::default();
    
    // Try to load Inter font from assets
    match std::fs::read("assets/Inter-Regular.ttf") {
        Ok(font_data) => {
            fonts.font_data.insert(
                "Inter".to_string(),
                egui::FontData::from_owned(font_data),
            );
            
            // Use Inter as the primary font
            fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Inter".to_string());
            fonts.families.entry(FontFamily::Monospace).or_default().push("Inter".to_string());
            
            ctx.set_fonts(fonts);
        }
        Err(e) => {
            eprintln!("Failed to load Inter font: {}. Using system defaults.", e);
            // Don't set custom fonts, use defaults
        }
    }
}

/// Configure egui visual style for minimal glass theme
fn configure_style(ctx: &mut Context) {
    let mut style = (*ctx.style()).clone();

    // Set dark theme
    style.visuals.dark_mode = true;

    // Ultra-transparent backgrounds
    style.visuals.panel_fill = Color32::TRANSPARENT;
    style.visuals.window_fill = Color32::TRANSPARENT;
    style.visuals.extreme_bg_color = Color32::TRANSPARENT;
    style.visuals.faint_bg_color = Color32::TRANSPARENT;

    // Text colors - high contrast
    style.visuals.widgets.noninteractive.fg_stroke =
        Stroke::new(1.0, Color32::from_rgb(240, 240, 255));
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 220, 240));
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::from_rgb(255, 255, 255));
    style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, Color32::from_rgb(255, 255, 255));

    // Minimal button styling
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgba_premultiplied(255, 255, 255, 10);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(0.5, Color32::from_rgba_premultiplied(255, 255, 255, 30));
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgba_premultiplied(255, 255, 255, 20);
    style.visuals.widgets.active.bg_fill = Color32::from_rgba_premultiplied(255, 255, 255, 30);

    // Subtle rounding
    let mut widgets = style.visuals.widgets.clone();
    widgets.noninteractive.rounding = egui::Rounding::from(4.0);
    widgets.inactive.rounding = egui::Rounding::from(4.0);
    widgets.hovered.rounding = egui::Rounding::from(4.0);
    widgets.active.rounding = egui::Rounding::from(4.0);
    style.visuals.widgets = widgets;

    // Minimal spacing
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(0.0);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    
    // Increase scroll sensitivity
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.handle_min_length = 20.0;
    style.spacing.scroll.bar_inner_margin = 2.0;
    style.spacing.scroll.bar_outer_margin = 2.0;
    style.spacing.scroll.floating = true;

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

    // Give UI time to initialize
    std::thread::sleep(Duration::from_millis(100));

    state_handle
}