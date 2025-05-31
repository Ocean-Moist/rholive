//! main.rs – Event-driven entry point for RhoLive assistant
//!
//! This implements a clean-sheet event-driven architecture that:
//! - Ingests audio continuously, breaks it into clause-size chunks with local VAD/Whisper
//! - Ingests video continuously at ~2 fps, drops duplicates
//! - Decides turn-by-turn whether to speak or stay silent (`<nothing>`)
//! - Drives the Gemini Live WS API correctly with activityStart/activityEnd markers
//!
//! The key improvement is that all turn management is centralized in a single FSM
//! that owns the authoritative "busy" state, eliminating race conditions.

mod events;
mod broker;
mod audio_capture;
mod video_capture;
mod gemini_ws;
mod gemini_ws_json;
mod ws_writer;
pub mod audio_async;
pub mod audio_seg;
mod gemini;
mod gemini_client;
mod screen;
pub mod ui;
mod util;

use crate::events::{InEvent, TurnInput, WsOut, WsIn, Outgoing};
use crate::broker::{Broker, Event};
use audio_seg::{AudioSegmenter, SegConfig, i16_slice_to_u8};
use ui::{launch_ui, AudioSample, ConversationEntry};

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // Initialize logging with filters
    use tracing_subscriber::{EnvFilter, prelude::*};

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(
                    EnvFilter::new("info")
                        .add_directive("egui_window_glfw_passthrough=warn".parse().unwrap())
                )
        )
        .init();
    std::env::set_var("RUST_BACKTRACE", "full");

    // Get API key
    let api_key = std::env::var("GEMINI_API_KEY")
        .expect("GEMINI_API_KEY environment variable must be set");

    // 1. Channel plumbing
    let (tx_in, mut rx_in) = mpsc::unbounded_channel::<InEvent>();
    let (tx_in_seg, mut rx_in_seg) = mpsc::unbounded_channel::<InEvent>();
    let (tx_ws, rx_ws) = mpsc::unbounded_channel::<WsOut>();
    let (tx_evt, mut rx_evt) = mpsc::unbounded_channel::<WsIn>();

    // Audio segmentation channel (legacy)
    let (seg_tx, mut seg_rx) = mpsc::unbounded_channel::<TurnInput>();
    
    // NEW: Outgoing message channel for all producers
    let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel::<Outgoing>();
    
    // NEW: Channel for websocket writer to send JSON
    let (ws_json_tx, mut ws_json_rx) = mpsc::unbounded_channel::<serde_json::Value>();
    
    // NEW: Global turn ID generator
    let turn_id_generator = Arc::new(AtomicU64::new(0));

    // UI channels
    let (ui_audio_tx, mut ui_audio_rx) = mpsc::unbounded_channel::<AudioSample>();
    let (ui_conv_tx, mut ui_conv_rx) = mpsc::unbounded_channel::<ConversationEntry>();

    // 2. Launch UI first
    info!("Starting UI...");
    let ui_state = launch_ui();

    // 3. Launch IO tasks
    info!("Starting audio capture...");
    let tx_in_for_audio = tx_in.clone();
    let tx_in_seg_for_audio = tx_in_seg.clone();
    audio_capture::spawn_with_dual_output(tx_in_for_audio, tx_in_seg_for_audio)?;

    info!("Starting video capture...");
    video_capture::spawn_with_outgoing(
        tx_in.clone(),
        outgoing_tx.clone(),
        turn_id_generator.clone()
    )?;

    // 4. Launch audio segmentation task in blocking thread
    let seg_config = SegConfig {
        open_voiced_frames: 4,      // 80ms to open (responsive)
        close_silence_ms: 250,      // 600ms silence to close (reasonable pauses)
        max_turn_ms: 8000,          // 8 seconds max (good for demo)
        min_clause_tokens: 10,       // 4 tokens for clause detection
        asr_poll_ms: 400,           // Poll every 400ms
        ring_capacity: 320_000,     // 20 seconds buffer
        asr_pool_size: 2,           // 2 worker threads
        asr_timeout_ms: 0,       // no timeout
    };
    let (audio_tx, audio_rx) = std::sync::mpsc::channel::<Vec<i16>>();
    let ui_state_seg = ui_state.clone();
    let ui_conv_tx_seg = ui_conv_tx.clone();
    
    // Spawn a task to forward from async to sync channel
    tokio::spawn(async move {
        while let Some(event) = rx_in_seg.recv().await {
            if let InEvent::AudioChunk(chunk) = event {
                if audio_tx.send(chunk).is_err() {
                    break;
                }
            }
        }
    });
    
    // Run segmenter in blocking thread
    let outgoing_tx_clone = outgoing_tx.clone();
    let turn_id_gen_clone = turn_id_generator.clone();
    std::thread::spawn(move || {
        let mut segmenter = AudioSegmenter::new(seg_config, None).unwrap();
        
        // Set up the new outgoing channel
        let (sync_outgoing_tx, sync_outgoing_rx) = std::sync::mpsc::channel();
        segmenter.set_outgoing_sender(sync_outgoing_tx, turn_id_gen_clone);
        
        // Forward outgoing events to the async channel
        std::thread::spawn(move || {
            while let Ok(event) = sync_outgoing_rx.recv() {
                let _ = outgoing_tx_clone.send(event);
            }
        });
        
        // Legacy streaming support (remove later)
        let (stream_tx, stream_rx) = std::sync::mpsc::channel();
        segmenter.set_streaming_sender(stream_tx);
        
        let seg_tx_clone = seg_tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = stream_rx.recv() {
                let _ = seg_tx_clone.send(event);
            }
        });
        
        while let Ok(chunk) = audio_rx.recv() {
            // Calculate audio level for UI
            let level = chunk.iter().map(|&s| (s as f32).abs()).sum::<f32>() 
                / chunk.len() as f32 / 32768.0;
            
            // Send audio to UI
            let _ = ui_audio_tx.send(AudioSample {
                level,
                timestamp: std::time::Instant::now(),
            });

            // Process through segmenter
            if let Some(turn) = segmenter.push_chunk(&chunk) {
                let pcm_bytes = i16_slice_to_u8(&turn.audio);
                
                // Update UI with user speech
                if let Some(ref text) = turn.text {
                    let entry = ConversationEntry {
                        role: "User".to_string(),
                        text: text.clone(),
                        timestamp: std::time::Instant::now(),
                    };
                    let _ = ui_conv_tx_seg.send(entry.clone());
                    
                    // Also update the UI state directly for immediate display
                    if let Ok(mut state) = ui_state_seg.lock() {
                        state.conversation_history.push_back(entry);
                        while state.conversation_history.len() > 50 {
                            state.conversation_history.pop_front();
                        }
                    }
                }
                
                // Update segments processed counter
                if let Ok(mut state) = ui_state_seg.lock() {
                    state.segments_processed += 1;
                }
                
                seg_tx.send(TurnInput::SpeechTurn {
                    pcm: pcm_bytes.to_vec(),
                    t_start: std::time::Instant::now(),
                    draft_text: turn.text,
                }).unwrap();
            }
        }
    });

    // 5. Launch WebSocket writer task
    let (ws_evt_tx, ws_evt_rx) = mpsc::unbounded_channel::<WsIn>();
    tokio::spawn(async move {
        ws_writer::run_writer(outgoing_rx, ws_json_tx, ws_evt_rx).await;
    });
    
    // 6. Launch Gemini websocket task (using new JSON-based handler)
    let api_key_clone = api_key.clone();
    let tx_evt_for_gemini = tx_evt.clone();
    tokio::spawn(async move {
        if let Err(e) = gemini_ws_json::run(&api_key_clone, ws_json_rx, tx_evt_for_gemini).await {
            error!("Gemini WebSocket error: {}", e);
        }
    });
    
    // We'll forward events to the writer later in the main loop

    // Response handler
    let ui_conv_tx_clone = ui_conv_tx.clone();
    let (evt_broadcast_tx, mut evt_broadcast_rx) = mpsc::unbounded_channel::<WsIn>();
    
    tokio::spawn(async move {
        let mut current_text = String::new();
        let mut turn_start = std::time::Instant::now();
        
        while let Some(event) = evt_broadcast_rx.recv().await {
            match event {
                WsIn::Text { content, is_final } => {
                    if current_text.is_empty() {
                        turn_start = std::time::Instant::now();
                    }
                    
                    current_text.push_str(&content);
                    
                    if !content.trim().is_empty() && content.trim() != "<nothing>" {
                        let _ = ui_conv_tx_clone.send(ConversationEntry {
                            role: "Gemini".to_string(),
                            text: current_text.clone(),
                            timestamp: std::time::Instant::now(),
                        });
                    }
                }
                WsIn::GenerationComplete => {
                    current_text.clear();
                }
                _ => {}
            }
        }
    });

    // 6. Connect UI state updates
    let ui_state_audio = ui_state.clone();
    let ui_state_conv = ui_state.clone();
    
    // Audio level visualization task
    tokio::spawn(async move {
        while let Some(sample) = ui_audio_rx.recv().await {
            if let Ok(mut state) = ui_state_audio.lock() {
                state.audio_samples.push_back(sample);
                // Keep only last 100 samples
                while state.audio_samples.len() > 100 {
                    state.audio_samples.pop_front();
                }
            }
        }
    });
    
    // Conversation update task
    tokio::spawn(async move {
        while let Some(entry) = ui_conv_rx.recv().await {
            if let Ok(mut state) = ui_state_conv.lock() {
                state.conversation_history.push_back(entry);
                // Keep only last 50 entries
                while state.conversation_history.len() > 50 {
                    state.conversation_history.pop_front();
                }
            }
        }
    });

    // 7. Simple event forwarding loop (broker is now just for UI updates)
    info!("Starting event forwarding loop...");
    let ui_state_broker = ui_state.clone();
    
    // Update UI connection status
    if let Ok(mut state) = ui_state_broker.lock() {
        state.connected = true;
        state.status_message = "Connected to Gemini".to_string();
    }
    
    loop {
        tokio::select! {
            // Legacy: Handle old-style speech turns from segmenter
            Some(turn) = seg_rx.recv() => {
                // For now, just log that we received it
                // The real streaming is handled by the Outgoing channel
                match turn {
                    TurnInput::SpeechTurn { pcm, .. } => {
                        info!("Received legacy SpeechTurn ({} KB)", pcm.len() / 1024);
                    }
                    TurnInput::StreamingAudio { .. } => {
                        debug!("Received legacy StreamingAudio event");
                    }
                    _ => {}
                }
            }
            // Handle WebSocket events  
            Some(e) = rx_evt.recv() => {
                match &e {
                    WsIn::GenerationComplete => {
                        info!("✅ Received GenerationComplete");
                        // Forward to writer for latency tracking
                        let _ = ws_evt_tx.send(e.clone());
                    }
                    WsIn::Text { content, .. } => {
                        debug!("📥 Received text response: {}", content.chars().take(50).collect::<String>());
                    }
                    _ => {}
                }
                // Broadcast to UI handler
                let _ = evt_broadcast_tx.send(e);
            }
            // Exit if all channels closed
            else => break,
        }
    }

    Ok(())
}