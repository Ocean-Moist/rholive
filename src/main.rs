//! RhoLive - Real-time assistant with system audio and screen access
//!
//! This application provides a real-time assistant that can access system audio,
//! screen content, and microphone. It integrates with Google's Gemini Live API
//! to provide AI-powered responses to user interactions.

/// Audio capture module for system and microphone audio
mod audio;
/// Screen capture module (enabled with the "capture" feature)
mod screen;
/// Gemini API client module with types and messages
mod gemini;
/// Redesigned Gemini client with proper WebSocket handling
mod gemini_client;
/// UI module with glass-like effect (enabled with the "ui" feature)
mod ui;

use audio::AudioCapturer;
use screen::ScreenCapturer;
use gemini::{GeminiClientConfig, ResponseModality, MediaResolution, ApiResponse};
// Use the new GeminiClient implementation
use gemini_client::GeminiClient;
use std::time::Duration;
use std::error::Error;
use std::sync::Arc;
use tracing::{info, error};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
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

impl From<Box<dyn Error>> for AppError {
    fn from(e: Box<dyn Error>) -> Self {
        AppError::CaptureError(e)
    }
}

impl From<std::env::VarError> for AppError {
    fn from(e: std::env::VarError) -> Self {
        AppError::EnvError(e)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    info!("Starting rholive assistant");

    // Get API key from environment variable
    let api_key = std::env::var("GEMINI_API_KEY").map_err(|e| -> Box<dyn Error> { 
        Box::new(AppError::EnvError(e)) 
    })?;
    
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
    gemini.connect_and_setup().await.map_err(|e| -> Box<dyn Error> {
        Box::new(AppError::GeminiError(e))
    })?;
    info!("Connected to Gemini");

    // Initialize audio capture
    let mut audio = AudioCapturer::new("rholive").map_err(|e| -> Box<dyn Error> {
        Box::new(AppError::CaptureError(e))
    })?;
    
    let mut screen = ScreenCapturer::new().map_err(|e| -> Box<dyn Error> {
        Box::new(AppError::CaptureError(e))
    })?;
    
    // Create channels for event handling
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    
    // Create a channel to receive responses from the Gemini client
    let (response_tx, mut response_rx) = mpsc::channel(100);
    
    let ui_state = launch_ui();
    
    // Start a response handler task
    let stop_tx_for_response = stop_tx.clone();
    
    let ui_state_clone = ui_state.clone();
    
    tokio::spawn(async move {
        while let Some(response) = response_rx.recv().await {
            match response {
                Ok(response) => match response {
                    ApiResponse::InputTranscription(transcript) => {
                        info!("Transcription: {} ({})", 
                              transcript.text, 
                              if transcript.is_final { "final" } else { "partial" });
                    },
                    ApiResponse::TextResponse { text, is_complete } => {
                        info!("Response: {} ({})", 
                              text, 
                              if is_complete { "complete" } else { "partial" });
                        
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
                    },
                    ApiResponse::AudioResponse { data, is_complete } => {
                        info!("Received audio response: {} bytes ({})", 
                             data.len(), 
                             if is_complete { "complete" } else { "partial" });
                        // In a full implementation, we would play this audio
                    },
                    ApiResponse::GoAway => {
                        info!("Server requested disconnection");
                        let _ = stop_tx_for_response.send(()).await;
                        break;
                    },
                    ApiResponse::SessionResumptionUpdate(token) => {
                        info!("Received session token for resumption: {}", token);
                        // Store this token for later reconnection
                    },
                    ApiResponse::SetupComplete => {
                        info!("Gemini setup completed successfully");
                    },
                    ApiResponse::ToolCall(tool_call) => {
                        info!("Received tool call: {:?}", tool_call);
                    },
                    ApiResponse::ToolCallCancellation(id) => {
                        info!("Tool call cancelled: {}", id);
                    },
                    ApiResponse::OutputTranscription(transcript) => {
                        info!("Output transcription: {} ({})",
                             transcript.text,
                             if transcript.is_final { "final" } else { "partial" });
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
    
    // Start the forwarding loop from the Gemini client to the UI
    let gemini_clone = Arc::new(Mutex::new(gemini));
    
    // Process responses from the Gemini client and forward them to the UI
    let task_gemini = gemini_clone.clone();
    tokio::spawn(async move {
        let mut gemini = task_gemini.lock().await;
        
        // Forward all responses to the application response channel
        loop {
            match gemini.next_response().await {
                Some(response) => {
                    if response_tx.send(response).await.is_err() {
                        error!("Failed to forward response to UI channel");
                        break;
                    }
                },
                None => {
                    error!("Response channel closed");
                    break;
                }
            }
        }
    });
    
    // Handle audio/video sending directly in the main loop
    // No need for a command channel with the new client design
    
    // Main capture and processing loop
    info!("Starting audio and screen capture");
    
    // Send an initial text message to Gemini
    let mut gemini_guard = gemini_clone.lock().await;
    if let Err(e) = gemini_guard.send_text("Hello, I'm now streaming audio and screen content to you.").await {
        error!("Failed to send initial text message: {:?}", e);
    }
    drop(gemini_guard); // Release the lock
    
    // Audio capture loop
    let mut buffer = [0u8; 3200]; // ~100ms of 16 kHz mono S16LE
    let mut is_first_chunk = true;
    
    loop {
        // Check for stop signal
        if stop_rx.try_recv().is_ok() {
            info!("Stopping capture loop");
            break;
        }
        
        // Capture audio
        if let Err(e) = audio.read(&mut buffer) {
            error!("Audio read error: {:?}", e);
            continue;
        }
        
        // Check if audio is muted in UI
        let is_audio_muted = if let Ok(state) = ui_state.lock() {
            state.is_muted
        } else {
            false
        };
        
        // Only send audio if not muted
        if !is_audio_muted {
            let mut gemini_guard = gemini_clone.lock().await;
            if let Err(e) = gemini_guard.send_audio(&buffer, is_first_chunk, false).await {
                error!("Failed to send audio: {:?}", e);
            }
            drop(gemini_guard);
        }
        is_first_chunk = false;
        
        // Capture and send screen frame
        if let Ok(mut frame) = screen.capture_frame() {
            let width = frame.width();
            let height = frame.height();
            let mime_type = frame.mime_type().to_string();
            
            // Convert frame to JPEG and send to Gemini
            match frame.to_jpeg() {
                Ok(jpeg_data) => {
                    debug!("Sending screen frame: {}x{} ({} bytes)", 
                          width, height, jpeg_data.len());
                    
                    let mut gemini_guard = gemini_clone.lock().await;
                    if let Err(e) = gemini_guard.send_video(&jpeg_data, &mime_type).await {
                        error!("Failed to send video frame: {:?}", e);
                    }
                    drop(gemini_guard);
                },
                Err(e) => {
                    error!("Failed to convert frame to JPEG: {:?}", e);
                }
            }
        }
        
        // Short delay to avoid overwhelming the API
        sleep(Duration::from_millis(100)).await;
    }
    
    // Send end-of-audio signal before exiting
    let mut gemini_guard = gemini_clone.lock().await;
    if let Err(e) = gemini_guard.send_audio(&[], false, true).await {
        error!("Failed to send end-of-audio signal: {:?}", e);
    }
    
    info!("rholive assistant stopped");
    Ok(())
}
