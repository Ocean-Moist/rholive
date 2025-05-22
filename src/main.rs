//! RhoLive - Real-time assistant with system audio and screen access
//!
//! This application provides a real-time assistant that can access system audio,
//! screen content, and microphone. It integrates with Google's Gemini Live API
//! to provide AI-powered responses to user interactions.

/// Audio capture module for system and microphone audio
pub mod audio;
/// Audio segmentation with VAD and Whisper
pub mod audio_seg;
/// Gemini API client module with types and messages
mod gemini;
/// Redesigned Gemini client with proper WebSocket handling
mod gemini_client;
/// Screen capture module (enabled with the "capture" feature)
mod screen;
/// UI module with glass-like effect (enabled with the "ui" feature)
mod ui;
/// Utility module for debugging
mod util;

use audio::AudioCapturer;
use gemini::{ApiResponse, GeminiClientConfig, MediaResolution, ResponseModality};
use screen::ScreenCapturer;
// Use the new GeminiClient implementation
use audio_seg::{send_turn_to_gemini, AudioSegmenter, SegConfig, SegmentedTurn};
use gemini_client::GeminiClient;
use std::collections::VecDeque;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use tracing::{error, info, warn};
// debug! macro for logging
use tracing::debug;
use ui::launch_ui;

// Custom error type for handling errors in the main function
#[derive(Debug)]
enum AppError {
    GeminiError(gemini::GeminiError),
    CaptureError(Box<dyn Error>),
    EnvError(std::env::VarError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::GeminiError(e) => write!(f, "Gemini error: {}", e),
            AppError::CaptureError(e) => write!(f, "Capture error: {}", e),
            AppError::EnvError(e) => write!(f, "Environment variable error: {}", e),
        }
    }
}

impl Error for AppError {}

impl From<gemini::GeminiError> for AppError {
    fn from(e: gemini::GeminiError) -> Self {
        AppError::GeminiError(e)
    }
}

impl From<Box<dyn Error + Send + Sync>> for AppError {
    fn from(e: Box<dyn Error + Send + Sync>) -> Self {
        AppError::CaptureError(e)
    }
}

impl From<Box<dyn Error>> for AppError {
    fn from(e: Box<dyn Error>) -> Self {
        AppError::CaptureError(e)
    }
}

// Make AppError Send and Sync
unsafe impl Send for AppError {}
unsafe impl Sync for AppError {}

impl From<std::env::VarError> for AppError {
    fn from(e: std::env::VarError) -> Self {
        AppError::EnvError(e)
    }
}

// Use a custom runtime with named threads for better debugging
fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    info!("Starting rholive assistant");

    // Build a custom runtime with named worker threads
    let rt = tokio::runtime::Builder::new_multi_thread()
        .thread_name_fn(|| {
            static ATOM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            format!(
                "tokio-w{}",
                ATOM.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            )
        })
        .enable_all()
        .build()?;

    // Start the async main function on the runtime
    rt.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Tracing is already initialized in main()

    // Get API key from environment variable
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(AppError::EnvError(e)) })?;

    // Configure and initialize Gemini client
    let config = GeminiClientConfig {
        model: "models/gemini-2.0-flash-live-001".to_string(),
        response_modality: ResponseModality::Text, // Use Text for simplicity, can be changed to Audio
        // System instructions in proper Content format
        system_instruction: Some("Act as a helpful assistant.".to_string()),
        media_resolution: Some(MediaResolution::Medium),
        temperature: Some(0.7),
        ..Default::default()
    };

    let mut gemini = GeminiClient::from_api_key(&api_key, Some(config));

    // Connect and set up the Gemini session
    info!("Connecting to Gemini API");
    gemini
        .connect_and_setup()
        .await
        .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(AppError::GeminiError(e)) })?;
    info!("Connected to Gemini");

    // Initialize audio capture with fallback between devices
    info!("Initializing audio capture with device fallback");
    // First list available audio devices for UI
    let audio_devices = match AudioCapturer::list_devices(audio::DeviceType::Any) {
        Ok(devices) => {
            info!("Found {} audio devices", devices.len());
            // Log each device for debugging
            for (i, device) in devices.iter().enumerate() {
                info!(
                    "  Device {}: {} ({})",
                    i + 1,
                    device.description,
                    if device.is_monitor {
                        "Monitor"
                    } else {
                        "Microphone"
                    }
                );
                info!("    Name: {}", device.name);
            }
            devices
        }
        Err(e) => {
            warn!("Failed to list audio devices: {}", e);
            Vec::new()
        }
    };

    // Create a simplified list for the UI
    let audio_device_list = audio_devices
        .iter()
        .map(|d| (d.name.clone(), d.description.clone()))
        .collect::<Vec<_>>();

    // Initialize with fallback - try non-monitor devices first
    info!("Using audio device fallback to find a working input");
    let mut audio = AudioCapturer::with_fallback("rholive")
        .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(AppError::CaptureError(e)) })?;

    // Get the active device name and log it
    let active_device = audio.device_name().map(|s| s.to_string());
    if let Some(device) = &active_device {
        info!("Successfully connected to audio device: {}", device);
    } else {
        info!("Using default audio device");
    }

    let mut screen = ScreenCapturer::new()
        .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(AppError::CaptureError(e)) })?;

    // Create channels for event handling
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

    // Create a channel to receive responses from the Gemini client
    let (response_tx, mut response_rx) = mpsc::channel(100);

    let ui_state = launch_ui();

    // Update UI with audio device info
    if let Ok(mut state) = ui_state.lock() {
        state.audio_device = active_device;
        state.audio_devices = audio_device_list;
    }

    // Create shared state before response handler
    // State for Gemini turns
    let awaiting_generation_arc = Arc::new(Mutex::new(false));
    let turn_backlog = Arc::new(Mutex::new(VecDeque::<SegmentedTurn>::new()));

    // Clone these for later use
    let awaiting_gen_clone = awaiting_generation_arc.clone();
    let turn_backlog_clone = turn_backlog.clone();

    // Wrap gemini in Arc<Mutex> for the writer operations only
    let gemini_clone = Arc::new(Mutex::new(gemini));

    // Start a response handler task
    let stop_tx_for_response = stop_tx.clone();

    let ui_state_clone = ui_state.clone();

    // Share state with the response handler
    let awaiting_generation_for_response = awaiting_generation_arc.clone();
    let _backlog_for_response = turn_backlog.clone(); // Added underscore to silence warning
    let gemini_for_response = gemini_clone.clone();

    tokio::spawn(async move {
        while let Some(response) = response_rx.recv().await {
            match response {
                Ok(response) => match response {
                    ApiResponse::InputTranscription(transcript) => {
                        info!(
                            "Transcription: {} ({})",
                            transcript.text,
                            if transcript.is_final {
                                "final"
                            } else {
                                "partial"
                            }
                        );
                    }
                    ApiResponse::TextResponse { text, is_complete } => {
                        info!(
                            "Response: {} ({})",
                            text,
                            if is_complete { "complete" } else { "partial" }
                        );

                        // Update UI with response if enabled
                        if let Ok(mut state) = ui_state_clone.lock() {
                            if is_complete {
                                // For complete responses, replace the text
                                state.ai_response = text;
                            } else {
                                // For partial responses, append to existing text
                                state.ai_response.push_str(&text);
                            }
                        }
                    }
                    ApiResponse::GenerationComplete => {
                        info!("Generation complete - processing backlog");

                        // Reset the awaiting_generation flag
                        let mut awaiting = awaiting_generation_for_response.lock().await;
                        *awaiting = false;

                        // Process the backlog of turns if not empty
                        let mut turns_to_process = VecDeque::new();

                        // Safely get the backlog and clear it
                        let mut backlog = turn_backlog.lock().await;
                        if !backlog.is_empty() {
                            info!("Processing {} queued turns from backlog", backlog.len());
                            // Move all turns from backlog to our local queue
                            std::mem::swap(&mut turns_to_process, &mut backlog);
                        }

                        if !turns_to_process.is_empty() {
                            // Process the first turn
                            if let Some(turn) = turns_to_process.pop_front() {
                                let mut gemini_guard = gemini_for_response.lock().await;

                                // Set awaiting generation flag
                                let mut awaiting = awaiting_generation_for_response.lock().await;
                                *awaiting = true;

                                // Send the turn directly
                                if let Err(e) = send_turn_to_gemini(&turn, &mut *gemini_guard).await
                                {
                                    error!("Failed to send backlog turn: {:?}", e);

                                    // Reset awaiting flag on error
                                    let mut awaiting =
                                        awaiting_generation_for_response.lock().await;
                                    *awaiting = false;
                                }

                                drop(gemini_guard);
                            }

                            // Put the rest back in the backlog for next time
                            if !turns_to_process.is_empty() {
                                let mut backlog = turn_backlog.lock().await;
                                // Add remaining turns back to the beginning of the backlog
                                let mut new_backlog = turns_to_process;
                                new_backlog.append(&mut backlog);
                                *backlog = new_backlog;
                            }
                        }
                    }
                    ApiResponse::AudioResponse { data, is_complete } => {
                        info!(
                            "Received audio response: {} bytes ({})",
                            data.len(),
                            if is_complete { "complete" } else { "partial" }
                        );
                        // In a full implementation, we would play this audio
                    }
                    ApiResponse::GoAway => {
                        info!("Server requested disconnection");
                        let _ = stop_tx_for_response.send(()).await;
                        break;
                    }
                    ApiResponse::SessionResumptionUpdate(token) => {
                        info!("Received session token for resumption: {}", token);
                        // Store this token for later reconnection
                    }
                    ApiResponse::SetupComplete => {
                        info!("Gemini setup completed successfully");
                    }
                    ApiResponse::ToolCall(tool_call) => {
                        info!("Received tool call: {:?}", tool_call);
                    }
                    ApiResponse::ToolCallCancellation(id) => {
                        info!("Tool call cancelled: {}", id);
                    }
                    ApiResponse::OutputTranscription(transcript) => {
                        info!(
                            "Output transcription: {} ({})",
                            transcript.text,
                            if transcript.is_final {
                                "final"
                            } else {
                                "partial"
                            }
                        );
                    }
                    ApiResponse::ConnectionClosed => {
                        info!("WebSocket connection closed, cleaning up resources");
                        let _ = stop_tx_for_response.send(()).await;
                        break;
                    }
                },
                Err(e) => {
                    error!("Error from Gemini: {:?}", e);
                    let _ = stop_tx_for_response.send(()).await;
                    break;
                }
            }
        }
    });

    // Process responses from the Gemini client and forward them to the UI
    {
        // Get a new subscription for this task
        let mut gemini_guard = gemini_clone.lock().await;
        let mut rx = gemini_guard.subscribe();
        drop(gemini_guard);

        tokio::spawn(async move {
            // Process responses directly from the channel without holding a lock
            while let Some(response) = rx.recv().await {
                if response_tx.send(response).await.is_err() {
                    error!("Failed to forward response to UI channel");
                    break;
                }
            }
            error!("Response channel closed");
        });
    }

    // Handle audio/video sending directly in the main loop
    // No need for a command channel with the new client design

    // Main capture and processing loop
    info!("Starting audio and screen capture");

    // Send an initial text message to Gemini
    let mut gemini_guard = gemini_clone.lock().await;
    if let Err(e) = gemini_guard.send_text("Hello, I'm now streaming audio and screen content to you. I'm an AI assistant that can see and hear what's happening on your computer. How can I help you today?").await {
        error!("Failed to send initial text message: {:?}", e);
    }
    drop(gemini_guard); // Release the lock

    // Audio capture setup
    let _buffer = [0u8; 3200]; // ~100ms of 16 kHz mono S16LE (unused)
    let mut last_successful_frame_time = std::time::Instant::now();
    let _consecutive_audio_errors = 0; // Unused but kept for reference

    // Configure screen captures to maintain 2 fps
    screen.set_capture_interval(Duration::from_millis(500)); // 500ms between frames (2 fps)

    // Setup audio segmentation
    info!("Initializing audio segmentation");

    // Create a whisper model path relative to the current directory
    let whisper_model_path = Path::new("./tiny.en-q8.gguf").to_path_buf();

    // Check if model exists
    let whisper_path = if whisper_model_path.exists() {
        info!("Using Whisper model at {}", whisper_model_path.display());
        Some(whisper_model_path.as_path())
    } else {
        warn!(
            "Whisper model not found at {}, falling back to VAD-only mode",
            whisper_model_path.display()
        );
        None
    };

    // Create segmenter configuration with the clean-sheet design parameters
    let seg_config = SegConfig {
        open_voiced_frames: 6, // 120ms of speech to open (more responsive)
        close_silence_ms: 300, // 300ms silence to close (faster turn completion)
        max_turn_ms: 5000,     // 5 seconds max turn (shorter turns)
        min_clause_tokens: 8,  // 8 tokens is roughly a short phrase
        asr_poll_ms: 300,  // Run Whisper every 300ms (more frequent analysis)
        ring_capacity: 320_000, // 20 seconds buffer
        asr_pool_size: 2,      // 2 worker threads
        asr_timeout_ms: 2000,  // 2 second timeout
    };

    // Create the segmenter
    let segmenter = match AudioSegmenter::new(seg_config, whisper_path) {
        Ok(seg) => Arc::new(Mutex::new(seg)),
        Err(e) => {
            error!("Failed to initialize audio segmentation: {}", e);
            return Err(Box::new(AppError::CaptureError(
                format!("Failed to initialize audio segmentation: {}", e).into(),
            )) as Box<dyn Error + Send + Sync>);
        }
    };

    // Create channels for turn processing
    let (turn_tx, mut turn_rx) = mpsc::channel::<SegmentedTurn>(8);

    // Audio processing will happen in the main loop instead of a spawned task
    let audio_ui_state = ui_state.clone();
    let turn_sender = turn_tx.clone();

    // Buffer for audio samples
    let _audio_buffer = [0i16; 1600]; // 100ms of 16kHz mono i16 (unused)
    let mut consecutive_audio_errors = 0;

    // Spawn a task to process turns and send to Gemini
    let gemini_for_turns = gemini_clone.clone();
    let awaiting_gen = awaiting_gen_clone.clone();
    let backlog = turn_backlog_clone.clone();

    tokio::spawn(async move {
        info!("Turn processing task started");

        while let Some(turn) = turn_rx.recv().await {
            // Check if we're waiting for generation
            let is_awaiting = {
                let guard = awaiting_gen.lock().await;
                *guard
            };

            if is_awaiting {
                // Add to backlog if we're waiting
                info!("Adding turn to backlog (waiting for generation to complete)");
                let mut backlog_guard = backlog.lock().await;
                backlog_guard.push_back(turn);

                // Cap backlog size to prevent memory issues
                if backlog_guard.len() > 10 {
                    warn!("Backlog too large, dropping oldest turn");
                    backlog_guard.pop_front();
                }
            } else {
                // Process turn directly
                let mut gemini_guard = gemini_for_turns.lock().await;

                // Set awaiting generation flag
                {
                    let mut guard = awaiting_gen.lock().await;
                    *guard = true;
                }

                // Send the turn directly
                if let Err(e) = send_turn_to_gemini(&turn, &mut *gemini_guard).await {
                    error!("Failed to send turn to Gemini: {:?}", e);

                    // Reset awaiting flag on error
                    let mut guard = awaiting_gen.lock().await;
                    *guard = false;
                }

                drop(gemini_guard);
            }
        }
    });

    info!("Starting main capture loop");

    loop {
        // Check for stop signal
        if stop_rx.try_recv().is_ok() {
            info!("Stopping capture loop due to stop signal");
            break;
        }

        // AUDIO PROCESSING
        // Check if audio is muted in UI
        let is_audio_muted = if let Ok(state) = audio_ui_state.lock() {
            state.is_muted
        } else {
            false
        };

        if !is_audio_muted {
            // Create a fresh buffer for each iteration of the loop
            let mut local_buffer = [0i16; 1600];
            let mut buffer_u8 = audio_seg::i16_to_u8_mut(&mut local_buffer);

            // Read audio directly (without spawn_blocking for now to avoid ownership issues)
            tdbg!("▶ audio.read() — entering (will block)");
            let read_start = std::time::Instant::now();
            match audio.read(&mut buffer_u8) {
                Ok(_) => {
                    tdbg!("⏹ audio.read() — returned in {:?}", read_start.elapsed());
                    // Reset error counter on success
                    consecutive_audio_errors = 0;

                    // Process buffer through segmenter - get a mutable reference to avoid Send issues
                    let mut segmenter_guard = segmenter.lock().await;
                    if let Some(turn) = segmenter_guard.push_chunk(&local_buffer) {
                        debug!(
                            "Segmenter produced a turn: {} samples, reason: {:?}",
                            turn.audio.len(),
                            turn.close_reason
                        );

                        // If we have partial text, log it
                        if let Some(text) = &turn.text {
                            info!("Segmenter transcription: {}", text);
                        }

                        // Send the turn for processing
                        if let Err(e) = turn_sender.send(turn).await {
                            error!("Failed to send turn: {}", e);
                        }
                    }
                    drop(segmenter_guard);
                }
                Err(e) => {
                    consecutive_audio_errors += 1;
                    error!("Audio read error: {:?}", e);

                    // If we have too many consecutive errors, try another device
                    if consecutive_audio_errors > 5 {
                        error!("Too many consecutive audio errors, trying different audio device");

                        // Try to get another device
                        match AudioCapturer::with_fallback("rholive") {
                            Ok(new_audio) => {
                                // Get the new device name
                                let new_device = new_audio.device_name().map(|s| s.to_string());
                                info!("Switched to new audio device: {:?}", new_device);

                                // Replace the audio capturer
                                audio = new_audio;

                                // Update UI
                                if let Ok(mut state) = audio_ui_state.lock() {
                                    state.audio_device = new_device;
                                }

                                consecutive_audio_errors = 0;
                            }
                            Err(e) => {
                                error!("Failed to initialize new audio device: {}", e);
                                // Take a longer break before trying again
                                sleep(Duration::from_secs(5)).await;
                                consecutive_audio_errors = 3; // Reset but not fully to try again soon
                            }
                        }
                    } else {
                        // For fewer errors, just take a short break
                        sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }

        // VIDEO CAPTURE AND PROCESSING - maintain steady 2 fps

        // Try to capture a frame on each cycle to maintain 2 fps
        // The capture_interval in the ScreenCapturer will limit actual captures
        // Check if we should try to capture a frame
        let now = std::time::Instant::now();
        let time_since_last_frame = now.duration_since(last_successful_frame_time);

        // If it's been too long since our last successful frame, force a capture
        tdbg!("▶ screen.capture_frame() — entering");
        let scr_start = std::time::Instant::now();
        let frame_result = if time_since_last_frame > Duration::from_secs(6) {
            debug!("Too long since last frame, forcing capture");
            screen.force_capture_frame()
        } else {
            screen.capture_frame()
        };
        tdbg!("⏹ screen.capture_frame() — {:?}", scr_start.elapsed());

        match frame_result {
            Ok(mut frame) => {
                let width = frame.width();
                let height = frame.height();
                let mime_type = frame.mime_type().to_string();

                // Convert frame to JPEG and send to Gemini
                match frame.to_jpeg() {
                    Ok(jpeg_data) => {
                        debug!("Captured screen frame: {}x{} ready to send", width, height);

                        let mut gemini_guard = gemini_clone.lock().await;
                        if let Err(e) = gemini_guard.send_video(&jpeg_data, &mime_type).await {
                            error!("Failed to send video frame: {:?}", e);
                        } else {
                            // Update last successful frame time
                            last_successful_frame_time = now;
                        }
                        drop(gemini_guard);
                    }
                    Err(e) => {
                        error!("Failed to convert frame to JPEG: {:?}", e);
                    }
                }
            }
            Err(e) => {
                // Only log certain errors at debug level since they're expected
                if e.to_string().contains("timeout") {
                    debug!("Frame capture timeout (normal): {}", e);
                } else if e.to_string().contains("Duplicate frame") {
                    debug!("Skipping duplicate frame (optimization)");
                } else {
                    error!("Failed to capture screen frame: {:?}", e);
                }
            }
        };

        // Reduce main loop delay to maintain steady frame rate
        // Each loop iteration should take about 100ms for proper timing with 500ms capture interval
        let delay_ms = if consecutive_audio_errors > 0 {
            // Longer delay if we're having audio issues
            200
        } else {
            // Normal delay - frequent enough to maintain 2 fps with the capture_interval
            100
        };

        // Short delay to avoid overwhelming the API
        sleep(Duration::from_millis(delay_ms)).await;
    }

    // Send end-of-audio signal before exiting
    info!("Cleaning up resources and closing connections");

    // Send a final message to Gemini
    let mut gemini_guard = gemini_clone.lock().await;

    // First send end-of-audio signal
    if let Err(e) = gemini_guard
        .send_audio_with_activity(&[], false, true, true)
        .await
    {
        error!("Failed to send end-of-audio signal: {:?}", e);
    }

    // Then send a proper goodbye message
    if let Err(e) = gemini_guard
        .send_text("The application is shutting down now. Thanks for using rholive assistant!")
        .await
    {
        error!("Failed to send goodbye message: {:?}", e);
    }

    // Small delay to ensure the final messages are sent
    drop(gemini_guard);
    sleep(Duration::from_millis(500)).await;

    // Update UI to indicate shutdown
    if let Ok(mut state) = ui_state.lock() {
        state.ai_response = "Application shutting down. Please close this window.".to_string();
    }

    // Final sleep to let messages propagate
    sleep(Duration::from_millis(1000)).await;

    info!("rholive assistant stopped successfully");
    Ok(())
}
