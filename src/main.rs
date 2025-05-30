//! main.rs – entry point for RhoLive assistant
//!
//! Invariants
//! ----------
//! • AutomaticActivityDetection is DISABLED ⇒ we *must* emit activityStart /
//!   activityEnd ourselves. audioStreamEnd is never sent.
//! • Never mix blobs and markers in the same RealtimeInput message.
//! • activityEnd **must** be the last message of a turn; no video frames after it.
//!
//! Turn life-cycle
//! ---------------
//!  audio  : ───────── start ……………… chunks ……………… end ───────┐
//!  video  : ─ frame ─ frame ─ frame ────────────────╯        │ OR Video-Only Turn
//!  markers: start───────────────┐  ┌──────────end             ├─ start ── end (video frames sent continuously)
//!                               ▼  ▼                          │
//!             [one Gemini server turn –⇢ generationComplete]──┘

pub mod audio;
pub mod audio_async;
pub mod audio_seg;
mod gemini;
mod gemini_client;
mod screen;
pub mod ui;
mod util;

use audio_async::AsyncAudioCapturer;
use audio_seg::{AudioSegmenter, SegConfig, SegmentedTurn, i16_slice_to_u8};
use gemini::{GeminiClientConfig, MediaResolution, ResponseModality, ApiResponse};
use gemini_client::GeminiClient;
use screen::ScreenCapturer;
use ui::{launch_ui, AudioSample, ConversationEntry};
use std::{error::Error, sync::{Arc, atomic::{AtomicBool, Ordering, AtomicU32}}, time::{Duration, Instant}};
use tokio::{
    sync::{mpsc, Mutex},
    time::sleep,
};
use tracing::{debug, error, info};

// Configuration for video-only turns
const VIDEO_TURN_CHECK_INTERVAL: Duration = Duration::from_secs(1); // How often to consider a video-only turn
const MIN_NEW_FRAMES_FOR_VIDEO_TURN: u32 = 1; // Min new unique frames sent to Gemini to trigger video turn

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    // ─────────────────── logging & panic backtrace ─────────────────────────
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    std::env::set_var("RUST_BACKTRACE", "full");

    // ─────────────────── Gemini Live API connection ────────────────────────
    let api_key = std::env::var("GEMINI_API_KEY")
        .expect("set GEMINI_API_KEY in the environment");
    let mut gemini = GeminiClient::from_api_key(
        &api_key,
        Some(GeminiClientConfig {
            model: "models/gemini-2.0-flash-live-001".into(),
            response_modality: ResponseModality::Text,
            system_instruction: Some(
                "\
you are a silent helper meant to assist the user in whatever task they choose. if you see a leetcode problem on the screen, solve it without waiting for them to say anything. if someone they are on call with asks you a question, answer it. you are effectively their second mind, they should not have to do any thinking, they should not have to ask you for anything. you are their brain, they should not have to think, respond to whatever is on screen or whatever someone says like the user would.

when there is no change or nothing to work, do, or comment on, respond only with '<nothing>' (without quotes).
                \
                ".into(),
            ),
            media_resolution: Some(MediaResolution::Medium),
            temperature: Some(0.7),
            ..Default::default()
        }),
    );
    gemini.connect_and_setup().await?;
    let mut gem_rx = gemini.subscribe();          // for logging / UI
    let gem = Arc::new(Mutex::new(gemini));      // shared writer guard

    // Launch UI and get state handle
    let ui_state = launch_ui();
    
    // Update connection status
    {
        let mut state = ui_state.lock().unwrap();
        state.connected = true;
        state.status_message = "Connected to Gemini Live API".to_string();
    }
    info!("✅ Connected to Gemini Live API");
    info!("🎯 Model: gemini-2.0-flash-live-001");

    // ─────────────────── audio capture & segmenter ─────────────────────────
    let mut mic = AsyncAudioCapturer::new("rholive", None)?;
    let device_name = mic.device_name();
    info!("🎙️  mic: {}", device_name);
    
    // Update UI with audio device info
    {
        let mut state = ui_state.lock().unwrap();
        state.audio_device = Some(device_name.to_string());
        state.status_message = format!("Using microphone: {}", device_name);
    }

    let mut segmenter = AudioSegmenter::new(
        SegConfig {
            open_voiced_frames: 4,      // 80ms to open (responsive)
            close_silence_ms: 600,      // 600ms silence to close (reasonable pauses)
            max_turn_ms: 8000,          // 8 seconds max (good for demo)
            min_clause_tokens: 10,       // 4 tokens for clause detection
            asr_poll_ms: 400,           // Poll every 400ms
            ring_capacity: 320_000,     // 20 seconds buffer
            asr_pool_size: 2,           // 2 worker threads
            asr_timeout_ms: 2000,       // 2 second timeout
        },
        Some(std::path::Path::new("./tiny.en-q8.gguf")),
    )?;
    
    info!("📊 Audio segmentation configured:");
    info!("   • Silence threshold: 600ms");
    info!("   • Max turn length: 8s");
    info!("   • ASR model: tiny.en-q8.gguf");
    
    // Update UI status
    {
        let mut state = ui_state.lock().unwrap();
        state.status_message = "Audio segmentation configured".to_string();
    }

    // channel carrying complete turns
    let (turn_tx, mut turn_rx) = mpsc::channel::<SegmentedTurn>(8);
    
    // channel for screen frames (continuous capture)
    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(16);

    // helper to send a single JPEG
    async fn send_frame(gem: &Arc<Mutex<GeminiClient>>, jpeg: &[u8], mime: &str) {
        let mut g = gem.lock().await;
        if let Err(e) = g.send_video(jpeg, mime).await {
            error!("send_video: {e:?}");
        }
    }
    
    // ─────────────────── continuous screen capture ─────────────────────────
    tokio::spawn({
        let frame_tx = frame_tx.clone();
        let ui_state_clone = ui_state.clone();
        async move {
            let mut screen = match ScreenCapturer::new() {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create screen capturer: {e}");
                    return;
                }
            };
            screen.set_capture_interval(Duration::from_millis(500)); // 2 FPS
            info!("📸 Continuous screen capture started at 2 FPS");
            
            // Update UI status
            if let Ok(mut state) = ui_state_clone.lock() {
                state.status_message = "Screen capture active (2 FPS)".to_string();
            }
            
            let mut last_hash = 0u64;
            let mut frames_captured = 0u32;
            let mut frames_skipped = 0u32;
            
            loop {
                match screen.capture_frame() {
                    Ok(mut frame) => {
                        frames_captured += 1;
                        let current_hash = frame.hash();
                        
                        // Skip duplicate frames
                        if current_hash != last_hash {
                            last_hash = current_hash;
                            match frame.to_jpeg() {
                                Ok(jpeg) => {
                                    if frame_tx.send(jpeg.to_vec()).await.is_err() {
                                        error!("Frame channel closed, stopping screen capture");
                                        break;
                                    }
                                }
                                Err(e) => error!("JPEG conversion failed: {e}"),
                            }
                        } else {
                            frames_skipped += 1;
                            if frames_skipped % 100 == 0 { // Log less frequently
                                debug!("Skipped {} duplicate frames", frames_skipped);
                            }
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if !err_str.contains("not reached") {
                            debug!("screen capture error: {err_str}");
                        }
                    }
                }
                // Screen capturer's internal interval usually dictates capture rate.
                // This sleep ensures the loop doesn't spin too fast if capture_frame is quick.
                sleep(Duration::from_millis(100)).await;
            }
            
            info!("Screen capture stopped. Captured: {}, Skipped: {}", frames_captured, frames_skipped);
        }
    });

    // Shared state to track if Gemini is currently processing a turn
    let is_gemini_processing = Arc::new(AtomicBool::new(false));

    // ─────────────────── orchestrator: one task per turn (audio-initiated) ─────
    tokio::spawn({
        let gem = gem.clone();
        let ui_state_clone = ui_state.clone();
        let is_gemini_processing = is_gemini_processing.clone(); // Clone Arc for the task
        async move {
            while let Some(turn) = turn_rx.recv().await {
                is_gemini_processing.store(true, Ordering::SeqCst); // Mark Gemini as busy

                info!("📤 Sending AUDIO turn to Gemini: {} samples ({:.2}s)", 
                      turn.audio.len(), 
                      turn.audio.len() as f32 / 16000.0);
                
                // Update UI with transcript and add to conversation history
                if let Some(text) = &turn.text {
                    let mut state = ui_state_clone.lock().unwrap();
                    state.current_transcript = text.clone();
                    // Add user transcript to history
                    state.conversation_history.push_back(ConversationEntry {
                        role: "User".to_string(),
                        text: text.clone(),
                        timestamp: Instant::now(),
                    });
                    // Keep history size reasonable
                    if state.conversation_history.len() > 50 {
                        state.conversation_history.pop_front();
                    }
                }
                
                // 1️⃣ activityStart
                {
                    let mut g = gem.lock().await;
                    g.send_audio_with_activity(&[], true, false, false).await.ok();
                }

                // 2️⃣ No need for per-turn frame pusher - we have continuous capture

                // 3️⃣ stream audio blobs (≤ 256 kB each)
                {
                    let mut g = gem.lock().await;
                    let pcm = i16_slice_to_u8(&turn.audio);
                    for chunk in pcm.chunks(256_000) {
                        g.send_audio_with_activity(chunk, false, false, false).await.ok();
                    }
                }

                // 4️⃣ activityEnd – last realtimeInput of the turn
                {
                    let mut g = gem.lock().await;
                    g.send_audio_with_activity(&[], false, true, false).await.ok();
                }
                // is_gemini_processing will be set to false when GenerationComplete is received
            }
        }
    });

    info!("🚀 RhoLive is ready! Start speaking or let it watch your screen...");
    
    {
        let mut state = ui_state.lock().unwrap();
        state.status_message = "Ready! Start speaking or let it watch...".to_string();
    }

    let mut last_status_update_time = Instant::now();
    let mut segments_processed = 0u32;
    let mut last_frame_send_time = Instant::now(); // Renamed for clarity
    let mut frames_sent_to_gemini = 0u32; // Renamed for clarity
    let mut last_audio_level_update_time = Instant::now(); // Renamed

    // State for video-only turns
    let mut last_video_turn_check_time = Instant::now();
    let mut frames_at_last_video_turn: u32 = 0;

    // ─────────────────── main audio/UI loop ─────────────────────────────
    loop {
        tokio::select! {
            // Handle Gemini responses
            Some(msg) = gem_rx.recv() => {
                match msg {
                    Ok(resp) => match resp {
                        ApiResponse::TextResponse { text, is_complete } => {
                            // Update UI with Gemini response
                            let mut state = ui_state.lock().unwrap();
                            
                            debug!("TextResponse: is_complete={}, text_len={}, text_preview={}", 
                                   is_complete, text.len(), 
                                   text.chars().take(50).collect::<String>());
                            
                            if is_complete {
                                // Final response - use accumulated text if available
                                let final_response = if state.current_ai_response.is_empty() {
                                    // No accumulated text, use the final chunk
                                    text.clone()
                                } else {
                                    // We have accumulated text, use it (it should already include this final chunk)
                                    // But append the final text just in case
                                    if !text.is_empty() && !state.current_ai_response.ends_with(&text) {
                                        state.current_ai_response.push_str(&text);
                                    }
                                    state.current_ai_response.clone()
                                };
                                
                                // Add complete response to history (ignore <nothing> responses)
                                if !final_response.is_empty() && final_response != "<nothing>" {
                                    state.conversation_history.push_back(ConversationEntry {
                                        role: "Gemini".to_string(),
                                        text: final_response.clone(),
                                        timestamp: Instant::now(),
                                    });
                                    // Keep history size reasonable
                                    if state.conversation_history.len() > 50 {
                                        state.conversation_history.pop_front();
                                    }
                                    info!("🤖 Gemini complete: {}", final_response);
                                }
                                
                                // Clear current response and transcript
                                state.current_ai_response.clear();
                                state.current_transcript.clear(); // Clear user transcript too
                                state.typewriter_position = 0;
                                state.typewriter_last_update = Instant::now();
                                is_gemini_processing.store(false, Ordering::SeqCst); // Gemini is done
                            } else {
                                // Streaming response - simply append the new text chunk
                                // Ignore <nothing> responses
                                if text != "<nothing>" {
                                    // If this is the first chunk, reset typewriter position
                                    if state.current_ai_response.is_empty() {
                                        state.typewriter_position = 0;
                                        state.typewriter_last_update = Instant::now();
                                        state.last_activity = Instant::now();
                                    }
                                    state.current_ai_response.push_str(&text);
                                    debug!("🤖 Gemini streaming: {} new chars (total: {} chars)", 
                                           text.len(), 
                                           state.current_ai_response.len());
                                }
                            }
                        }
                        ApiResponse::GenerationComplete => {
                            debug!("Generation complete");
                            // Ensure processing flag is cleared if no text response indicated completion
                            if is_gemini_processing.load(Ordering::SeqCst) {
                                let mut state = ui_state.lock().unwrap();
                                if !state.current_ai_response.is_empty() && state.current_ai_response != "<nothing>" {
                                    let response_text = state.current_ai_response.clone();
                                    state.conversation_history.push_back(ConversationEntry {
                                        role: "Gemini".to_string(),
                                        text: response_text.clone(),
                                        timestamp: Instant::now(),
                                    });
                                    if state.conversation_history.len() > 50 { state.conversation_history.pop_front(); }
                                    info!("🤖 Gemini complete (from GenComplete): {}", response_text);
                                }
                                state.current_ai_response.clear();
                                state.current_transcript.clear();
                                state.typewriter_position = 0;
                                state.typewriter_last_update = Instant::now();
                            }
                            is_gemini_processing.store(false, Ordering::SeqCst); // Gemini is done
                        }
                        other => debug!("Gemini: {other:?}"),
                    },
                    Err(e) => {
                        error!("Gemini error: {e:?}");
                        is_gemini_processing.store(false, Ordering::SeqCst); // Error, so not processing
                    },
                }
            }
            
            // Handle audio chunks
            Some(chunk) = mic.read_chunk() => {
                if last_audio_level_update_time.elapsed() >= Duration::from_millis(20) {
                    let level = calculate_audio_level(&chunk);
                    let mut state = ui_state.lock().unwrap();
                    state.audio_samples.push_back(AudioSample {
                        level,
                        timestamp: Instant::now(),
                    });
                    // Keep only last 200 samples (4 seconds at 50Hz)
                    if state.audio_samples.len() > 200 {
                        state.audio_samples.pop_front();
                    }
                    state.is_speaking = level > 0.01;
                    last_audio_level_update_time = Instant::now();
                }
                
                // Process the buffer through the segmenter
                if let Some(turn) = segmenter.push_chunk(&chunk) {
                    segments_processed += 1;
                    info!("🎯 Detected speech segment #{}: {} samples ({:.2}s)", 
                          segments_processed,
                          turn.audio.len(), 
                          turn.audio.len() as f32 / 16000.0);
                    
                    // Update UI
                    {
                        let mut state = ui_state.lock().unwrap();
                        state.segments_processed = segments_processed;
                        state.is_speaking = false; // Reset speaking indicator after segment detection
                        if let Some(text) = &turn.text {
                            state.current_transcript = text.clone();
                            info!("   Early transcript: {}", text);
                        }
                        state.status_message = format!("Segment #{} detected ({:.1}s)", 
                                                     segments_processed, 
                                                     turn.audio.len() as f32 / 16000.0);
                    }
                    info!("   Reason: {:?}", turn.close_reason);
                    
                    if turn_tx.send(turn).await.is_err() {
                        error!("Failed to send turn to orchestrator");
                        break; // Critical error
                    }
                }
            }
            
            // Handle screen frames - send them to Gemini
            Some(jpeg) = frame_rx.recv() => {
                if last_frame_send_time.elapsed() >= Duration::from_millis(500) { // Approx 2 FPS to Gemini
                    info!("📸 Sending screenshot #{} to Gemini (size: {} bytes)", 
                          frames_sent_to_gemini + 1, jpeg.len());
                    
                    send_frame(&gem, &jpeg, "image/jpeg").await;
                    frames_sent_to_gemini += 1;
                    last_frame_send_time = Instant::now();
                    
                    {
                        let mut state = ui_state.lock().unwrap();
                        state.frames_sent = frames_sent_to_gemini; // Update UI
                    }
                }
            }
            
            // Periodic checks (status updates and video-only turns)
            _ = sleep(Duration::from_millis(500)) => { // Check fairly often
                // Video-only turn logic
                if last_video_turn_check_time.elapsed() >= VIDEO_TURN_CHECK_INTERVAL {
                    last_video_turn_check_time = Instant::now(); // Reset check timer

                    if !is_gemini_processing.load(Ordering::SeqCst) { // Only if Gemini is idle
                        let current_total_frames_sent = frames_sent_to_gemini; // From our counter
                        let new_unique_frames_for_video_turn = current_total_frames_sent.saturating_sub(frames_at_last_video_turn);

                        if new_unique_frames_for_video_turn >= MIN_NEW_FRAMES_FOR_VIDEO_TURN {
                            info!("💡 Initiating VIDEO-ONLY turn ({} new unique frames).", new_unique_frames_for_video_turn);
                            is_gemini_processing.store(true, Ordering::SeqCst); // Mark Gemini as busy

                            { // Update UI
                                let mut state = ui_state.lock().unwrap();
                                state.status_message = "Analyzing screen content...".to_string();
                                state.current_transcript.clear(); // No user audio for this turn
                                state.conversation_history.push_back(ConversationEntry {
                                    role: "System".to_string(),
                                    text: "[Screen analysis initiated by system]".to_string(),
                                    timestamp: Instant::now(),
                                });
                                if state.conversation_history.len() > 50 {
                                    state.conversation_history.pop_front();
                                }
                            }

                            // Send activityStart for video-only turn
                            {
                                let mut g = gem.lock().await;
                                g.send_audio_with_activity(&[], true, false, false).await.ok();
                            }
                            // Video frames are sent by their own task. Gemini will use the most recent ones.
                            // Send activityEnd for video-only turn
                            {
                                let mut g = gem.lock().await;
                                g.send_audio_with_activity(&[], false, true, false).await.ok();
                            }
                            frames_at_last_video_turn = current_total_frames_sent;
                            // is_gemini_processing will be set to false when GenerationComplete is received
                        }
                    }
                }

                // Status update logic (runs less frequently than the 500ms sleep, due to its own timer)
                if last_status_update_time.elapsed() >= Duration::from_secs(5) {
                    if !is_gemini_processing.load(Ordering::SeqCst) { // Only update to "Listening" if idle
                        let mut state = ui_state.lock().unwrap();
                        state.status_message = "Listening...".to_string();
                    }
                    debug!("Status: segments_processed={}, frames_sent_to_gemini={}", segments_processed, frames_sent_to_gemini);
                    last_status_update_time = Instant::now();
                }
            }
            
            else => {
                // All channels closed or other select! termination
                info!("Main loop select! terminated. Exiting.");
                break;
            }
        }
    }
    
    info!("RhoLive assistant shutting down.");
    Ok(())
}

/// Calculate RMS audio level from PCM samples
fn calculate_audio_level(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    
    let sum_squares: f64 = samples.iter()
        .map(|&s| (s as f64).powi(2))
        .sum();
    
    let rms = (sum_squares / samples.len() as f64).sqrt();
    // Normalize to 0.0-1.0 range (i16 max is 32767)
    (rms / 32767.0) as f32
}