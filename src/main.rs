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

pub mod audio;
pub mod audio_async;
pub mod audio_seg;
mod gemini;
mod gemini_client;
mod screen;
pub mod ui;
mod util;

use crate::events::{InEvent, TurnInput, WsOut, WsIn};
use crate::broker::{Broker, Event};
use audio_seg::{AudioSegmenter, SegConfig, SegmentedTurn, i16_slice_to_u8};
use ui::{launch_ui, AudioSample, ConversationEntry};

use anyhow::Result;
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tracing::{error, info};

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

    // Audio segmentation channel
    let (seg_tx, mut seg_rx) = mpsc::unbounded_channel::<TurnInput>();

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
    video_capture::spawn(tx_in.clone())?;

    // 4. Launch audio segmentation task in blocking thread
    let seg_config = SegConfig::default();
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
    std::thread::spawn(move || {
        let mut segmenter = AudioSegmenter::new(seg_config, None).unwrap();
        
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

    // 5. Launch Gemini websocket task
    let api_key_clone = api_key.clone();
    tokio::spawn(async move {
        if let Err(e) = gemini_ws::run(&api_key_clone, rx_ws, tx_evt).await {
            error!("Gemini WebSocket error: {}", e);
        }
    });

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

    // 7. Run the broker FSM
    info!("Starting turn broker...");
    let mut broker = Broker::new();
    let ui_state_broker = ui_state.clone();
    
    // Update UI connection status
    if let Ok(mut state) = ui_state_broker.lock() {
        state.connected = true;
        state.status_message = "Connected to Gemini".to_string();
    }
    
    loop {
        let evt = tokio::select! {
            // Handle speech turns from segmenter
            Some(turn) = seg_rx.recv() => {
                let messages = broker.handle_speech_turn(turn);
                
                // Update frames sent counter
                let frame_count = messages.iter().filter(|m| {
                    if let WsOut::RealtimeInput(json) = m {
                        json.get("video").is_some()
                    } else {
                        false
                    }
                }).count();
                
                if frame_count > 0 {
                    if let Ok(mut state) = ui_state_broker.lock() {
                        state.frames_sent += frame_count as u32;
                    }
                }
                
                for msg in messages {
                    tx_ws.send(msg)?;
                }
                continue;
            }
            // Handle input events
            Some(e) = rx_in.recv() => Event::Input(e),
            // Handle WebSocket events  
            Some(e) = rx_evt.recv() => {
                let _ = evt_broadcast_tx.send(e.clone());
                Event::Ws(e)
            }
            // Exit if all channels closed
            else => break,
        };
        
        for msg in broker.handle(evt) {
            tx_ws.send(msg)?;
        }
    }

    Ok(())
}