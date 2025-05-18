//! RhoLive - Real-time assistant with system audio and screen access
//!
//! This application provides a real-time assistant that can access system audio,
//! screen content, and microphone. It integrates with Google's Gemini Live API
//! to provide AI-powered responses to user interactions.

#![forbid(unsafe_code)]

/// Audio capture module for system and microphone audio
mod audio;
/// Screen capture module (enabled with the "capture" feature)
#[cfg(feature = "capture")]
mod screen;
/// Gemini API client module
mod gemini;

use audio::AudioCapturer;
#[cfg(feature = "capture")]
use screen::ScreenCapturer;
use gemini::{GeminiClientConfig, GeminiClient, ResponseModality, MediaResolution, ApiResponse};
use std::time::Duration;
use std::error::Error;
use std::sync::Arc;
use tracing::{info, error, debug};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;

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
        system_instruction: Some("You are a helpful real-time assistant with access to screen and audio content. Respond concisely and helpfully.".to_string()),
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
    
    // Initialize screen capture if enabled
    #[cfg(feature = "capture")]
    let mut screen = ScreenCapturer::new().map_err(|e| -> Box<dyn Error> {
        Box::new(AppError::CaptureError(e))
    })?;
    
    // Create channels for event handling
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    
    // Create channels for Gemini client communication
    let (command_tx, mut command_rx) = mpsc::channel(100);
    let (response_tx, mut response_rx) = mpsc::channel(100);
    
    // Start a response handler task
    let stop_tx_for_response = stop_tx.clone();
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
                    ApiResponse::SessionResumptionUpdate(_token) => {
                        debug!("Received session token for resumption");
                        // Store this token for later reconnection
                    },
                    _ => {}
                },
                Err(e) => {
                    error!("Error from Gemini: {:?}", e);
                    let _ = stop_tx_for_response.send(()).await;
                    break;
                }
            }
        }
    });
    
    // Commands that can be sent to the Gemini client
    enum GeminiCommand {
        SendText(String),
        SendAudio {
            data: Vec<u8>,
            is_start: bool,
            is_end: bool,
        },
        SendVideo {
            data: Vec<u8>,
            mime_type: String,
        },
    }
    
    // Start the Gemini client in its own task
    tokio::spawn(async move {
        // Use Arc<Mutex<>> to share Gemini client between tasks
        let gemini = Arc::new(Mutex::new(gemini));
        
        // Forward responses from Gemini client to the response channel
        let response_tx_clone = response_tx.clone();
        let gemini_clone = gemini.clone();
        tokio::spawn(async move {
            loop {
                let response = {
                    let mut gemini_guard = gemini_clone.lock().await;
                    match gemini_guard.next_response().await {
                        Some(response) => response,
                        None => break,
                    }
                };
                
                if response_tx_clone.send(response).await.is_err() {
                    break;
                }
            }
        });
        
        // Process commands sent to the Gemini client
        while let Some(command) = command_rx.recv().await {
            let result = {
                let mut gemini_guard = gemini.lock().await;
                match command {
                    GeminiCommand::SendText(text) => {
                        gemini_guard.send_text(&text).await
                    },
                    GeminiCommand::SendAudio { data, is_start, is_end } => {
                        gemini_guard.send_audio(&data, is_start, is_end).await
                    },
                    GeminiCommand::SendVideo { data, mime_type } => {
                        gemini_guard.send_video(&data, &mime_type).await
                    },
                }
            };
            
            if let Err(e) = result {
                error!("Error executing Gemini command: {:?}", e);
            }
        }
    });
    
    // Main capture and processing loop
    info!("Starting audio and screen capture");
    
    // Example: Send a text message to Gemini
    let _ = command_tx.send(GeminiCommand::SendText(
        "Hello, I'm now streaming audio and screen content to you.".to_string()
    )).await;
    
    // Audio capture loop
    let mut buffer = [0u8; 3200]; // ~100ms of 16 kHz mono S16LE
    let mut is_first_chunk = true;
    
    loop {
        // Check for stop signal
        if stop_rx.try_recv().is_ok() {
            info!("Stopping capture loop");
            break;
        }
        
        // Capture and send audio
        if let Err(e) = audio.read(&mut buffer) {
            error!("Audio read error: {:?}", e);
            continue;
        }
        
        // Send audio to Gemini - mark first chunk as start of activity
        let _ = command_tx.send(GeminiCommand::SendAudio {
            data: buffer.to_vec(),
            is_start: is_first_chunk,
            is_end: false,
        }).await;
        is_first_chunk = false;
        
        // Capture and send screen frame (if enabled)
        #[cfg(feature = "capture")]
        if let Ok(mut frame) = screen.capture_frame() {  
            let width = frame.width();
            let height = frame.height();
            
            // Convert frame to JPEG and send to Gemini
            match frame.to_jpeg() {
                Ok(jpeg_data) => {
                    debug!("Sending screen frame: {}x{} ({} bytes)", 
                          width, height, jpeg_data.len());
                    let _ = command_tx.send(GeminiCommand::SendVideo {
                        data: jpeg_data.to_vec(),
                        mime_type: frame.mime_type().to_string(),
                    }).await;
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
    let _ = command_tx.send(GeminiCommand::SendAudio {
        data: vec![],
        is_start: false,
        is_end: true,
    }).await;
    
    info!("rholive assistant stopped");
    Ok(())
}
