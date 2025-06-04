//! Refactored main.rs with simplified three-layer architecture

mod media_event;
mod media_in;
mod simple_turn_fsm;
mod simple_turn_runner;
mod gemini_ws_unified;
mod recorder;

// Keep existing modules we still need
mod gemini;
mod gemini_client;
mod screen;
mod audio_seg;
mod ui;
mod util;

use media_event::{MediaEvent, WsOutbound, WsInbound, Outgoing};
use audio_seg::{AudioSegmenter, SegConfig};
use ui::{launch_ui, AudioSample, ConversationEntry};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Audio source to capture
    #[arg(short, long, value_enum, default_value = "both")]
    audio_source: AudioSourceArg,
    
    /// Enable test recorder (writes turns/frames to ./recordings/)
    #[arg(long, help = "Enable test recorder (writes turns/frames to ./recordings/)")]
    record: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AudioSourceArg {
    /// Capture microphone only
    Mic,
    /// Capture system audio only (what you hear)
    System,
    /// Capture both microphone and system audio
    Both,
}

impl From<AudioSourceArg> for media_in::AudioSource {
    fn from(arg: AudioSourceArg) -> Self {
        match arg {
            AudioSourceArg::Mic => media_in::AudioSource::Microphone,
            AudioSourceArg::System => media_in::AudioSource::System,
            AudioSourceArg::Both => media_in::AudioSource::Both,
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    // Initialize logging
    use tracing_subscriber::{EnvFilter, prelude::*};
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(
                    EnvFilter::new("debug")
                        .add_directive("egui_window_glfw_passthrough=warn".parse().unwrap())
                        .add_directive("rholive=debug".parse().unwrap())
                )
        )
        .init();
    
    info!("Starting RhoLive - Refactored Architecture");
    
    // Get API key
    let api_key = std::env::var("GEMINI_API_KEY")
        .expect("GEMINI_API_KEY environment variable must be set");
    
    // === Layer 1: Media Capture ===
    // Single broadcast channel for all media events
    let (media_tx, _) = broadcast::channel::<MediaEvent>(256);
    
    // === Layer 2: Turn FSM ===
    // Channel for AudioSegmenter -> Turn FSM
    let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel::<Outgoing>();
    
    // === Layer 3: Gemini I/O ===
    // Channels for WebSocket communication
    let (ws_out_tx, ws_out_rx) = mpsc::unbounded_channel::<WsOutbound>();
    let (ws_in_tx, mut ws_in_rx) = mpsc::unbounded_channel::<WsInbound>();
    
    // UI channels
    let (ui_audio_tx, mut ui_audio_rx) = mpsc::unbounded_channel::<AudioSample>();
    let (ui_conv_tx, mut ui_conv_rx) = mpsc::unbounded_channel::<ConversationEntry>();
    
    // Turn ID generator (shared between all producers)
    let turn_id_generator = Arc::new(AtomicU64::new(1));
    
    // ===== Launch UI =====
    info!("Starting UI...");
    let ui_state = launch_ui();
    
    if let Ok(mut state) = ui_state.lock() {
        state.connected = true;
        state.status_message = "Connected to Gemini".to_string();
    }
    
    // ===== Layer 1: Media Capture =====
    info!("Starting media capture with audio source: {:?}", args.audio_source);
    media_in::spawn_audio_capture_with_source(media_tx.clone(), args.audio_source.into())?;
    media_in::spawn_video_capture(media_tx.clone())?;
    
    // ===== Audio Segmentation Task =====
    // This bridges Layer 1 -> Layer 2
    let seg_config = SegConfig {
        open_voiced_frames: 4,      // 80ms to open
        close_silence_ms: 500,      // 250ms silence to close
        max_turn_ms: 8000,          // 8 seconds max
        min_clause_tokens: 5,      // 10 tokens for clause
        asr_poll_ms: 400,           // Poll every 400ms
        ring_capacity: 320_000,     // 20 seconds buffer
        asr_pool_size: 2,           // 2 worker threads
        asr_timeout_ms: 0,          // no timeout
    };
    
    let mut audio_rx = media_tx.subscribe();
    let outgoing_tx_seg = outgoing_tx.clone();
    let turn_id_gen_seg = turn_id_generator.clone();
    let ui_conv_tx_seg = ui_conv_tx.clone();
    let ui_state_seg = ui_state.clone();
    
    // Run segmenter in a dedicated thread
    std::thread::spawn(move || {
        let mut segmenter = AudioSegmenter::new(seg_config, None).unwrap();
        
        // Create sync channel for the segmenter
        let (sync_outgoing_tx, sync_outgoing_rx) = std::sync::mpsc::channel();
        segmenter.set_outgoing_sender(sync_outgoing_tx, turn_id_gen_seg);
        
        // Forward sync events to async channel
        let outgoing_tx_forward = outgoing_tx_seg.clone();
        std::thread::spawn(move || {
            while let Ok(event) = sync_outgoing_rx.recv() {
                let _ = outgoing_tx_forward.send(event);
            }
        });
        
        // Create async-to-sync bridge for audio
        let (audio_sync_tx, audio_sync_rx) = std::sync::mpsc::channel::<Vec<i16>>();
        
        // Bridge async audio to sync
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                while let Ok(event) = audio_rx.recv().await {
                    if let MediaEvent::AudioFrame { pcm, .. } = event {
                        if audio_sync_tx.send(pcm).is_err() {
                            break;
                        }
                    }
                }
            });
        });
        
        // Process audio chunks
        while let Ok(chunk) = audio_sync_rx.recv() {
            if let Some(turn) = segmenter.push_chunk(&chunk) {
                // Update UI with transcription
                if let Some(ref text) = turn.text {
                    let entry = ConversationEntry {
                        role: "User".to_string(),
                        text: text.clone(),
                        timestamp: Instant::now(),
                        is_streaming: false, // User entries are never streaming
                    };
                    let _ = ui_conv_tx_seg.send(entry);
                }
                
                // Update segments counter
                if let Ok(mut state) = ui_state_seg.lock() {
                    state.segments_processed += 1;
                }
            }
        }
    });
    
    // ===== Layer 2: Simple Turn FSM =====
    info!("Starting Simple Turn FSM...");
    let media_tx_fsm = media_tx.clone();
    let media_rx_fsm = media_tx.subscribe();
    let (ws_in_fsm_tx, ws_in_rx_fsm) = mpsc::unbounded_channel::<WsInbound>();
    let record_flag = args.record;
    
    tokio::spawn(async move {
        simple_turn_runner::run(
            media_tx_fsm,
            media_rx_fsm,
            outgoing_rx,
            ws_out_tx,
            ws_in_rx_fsm,
            record_flag,
        ).await;
    });
    
    // ===== Layer 3: Gemini WebSocket =====
    info!("Starting Gemini connection...");
    tokio::spawn(async move {
        if let Err(e) = gemini_ws_unified::run(&api_key, ws_out_rx, ws_in_tx).await {
            error!("Gemini WebSocket error: {}", e);
        }
    });
    
    // ===== UI Update Tasks =====
    
    // Audio visualization
    let mut ui_media_rx = media_tx.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = ui_media_rx.recv().await {
            if let MediaEvent::AudioFrame { pcm, timestamp } = event {
                let level = pcm.iter().map(|&s| (s as f32).abs()).sum::<f32>() 
                    / pcm.len() as f32 / 32768.0;
                
                let _ = ui_audio_tx.send(AudioSample { level, timestamp });
            }
        }
    });
    
    // Audio samples update
    let ui_state_audio = ui_state.clone();
    tokio::spawn(async move {
        while let Some(sample) = ui_audio_rx.recv().await {
            if let Ok(mut state) = ui_state_audio.lock() {
                state.audio_samples.push_back(sample);
                while state.audio_samples.len() > 100 {
                    state.audio_samples.pop_front();
                }
            }
        }
    });
    
    // Conversation update with live streaming support
    let ui_state_conv = ui_state.clone();
    tokio::spawn(async move {
        while let Some(entry) = ui_conv_rx.recv().await {
            if let Ok(mut state) = ui_state_conv.lock() {
                // Check if we should update the last entry or add a new one
                if entry.is_streaming && entry.role == "Gemini" {
                    // Look for an existing streaming Gemini entry to update
                    if let Some(last_entry) = state.conversation_history.back_mut() {
                        if last_entry.role == "Gemini" && last_entry.is_streaming {
                            // Update the existing streaming entry
                            last_entry.text = entry.text;
                            last_entry.timestamp = entry.timestamp;
                            continue;
                        }
                    }
                }
                
                // Add new entry
                state.conversation_history.push_back(entry);
                while state.conversation_history.len() > 50 {
                    state.conversation_history.pop_front();
                }
            }
        }
    });
    
    // WebSocket event forwarder and UI handler
    let ui_conv_tx_resp = ui_conv_tx.clone();
    tokio::spawn(async move {
        let mut current_text = String::new();
        
        while let Some(event) = ws_in_rx.recv().await {
            // Forward to FSM
            let _ = ws_in_fsm_tx.send(event.clone());
            
            // Handle UI updates
            match event {
                WsInbound::Text { content, is_final } => {
                    current_text.push_str(&content);
                    
                    // Remove any <nothing> responses from the accumulated text
                    if current_text.contains("<nothing>") {
                        current_text = current_text.replace("<nothing>", "");
                    }
                    
                    let trimmed = current_text.trim();
                    
                    // Only send to UI if not empty after cleaning
                    if !trimmed.is_empty() {
                        let _ = ui_conv_tx_resp.send(ConversationEntry {
                            role: "Gemini".to_string(),
                            text: trimmed.to_string(),
                            timestamp: Instant::now(),
                            is_streaming: !is_final, // Mark as streaming if not final
                        });
                    }
                    
                    if is_final {
                        current_text.clear();
                    }
                }
                _ => {}
            }
        }
    });
    
    // Keep main thread alive
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");
    
    Ok(())
}