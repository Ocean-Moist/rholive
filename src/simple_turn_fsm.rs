//! Simple Turn FSM - Minimal state machine leveraging Gemini's natural batching
//! 
//! With NO_INTERRUPTION mode, Gemini acts as a double buffer:
//! - Active slot: Currently generating response
//! - Next slot: Accumulating new input
//! 
//! This allows us to send everything immediately with no client-side queuing.

use crate::media_event::{WsOutbound, MediaEvent};
use base64::Engine;
use serde_json::json;
use std::collections::VecDeque;
use std::time::{Instant, Duration};
use tokio::sync::broadcast;
use tracing::{debug, info};

/// Number of frames to batch in each turn when idle
/// - Set to 1 for original behavior (one frame per turn)
/// - Set to 2+ to batch multiple frames before requesting a response
const FRAMES_PER_TURN: usize = 2;

/// Maximum time to wait for forced frame before sending activityEnd
const FORCE_FRAME_TIMEOUT_MS: u64 = 50;

/// Events that can occur
#[derive(Debug)]
pub enum Event {
    /// Speech detected by VAD
    SpeechStart,
    /// Audio chunk (20ms PCM)
    AudioChunk(Vec<u8>),
    /// Speech ended (VAD closed or semantic boundary)
    SpeechEnd,
    /// Video frame with perceptual hash
    Frame { jpeg: Vec<u8>, hash: u64 },
    /// Response received from Gemini
    ResponseReceived,
}

/// FSM states
#[derive(Debug, Clone, Copy)]
enum State {
    /// No activity - waiting for frames or audio
    Idle,
    /// Collecting frames for a batch turn
    FrameBatch,
    /// Audio turn active - streaming audio + piggybacking frames
    AudioTurn,
    /// Waiting for forced frame before ending audio turn
    WaitingForForcedFrame,
}

/// Minimal turn state machine
pub struct SimpleTurnFsm {
    /// Current state
    state: State,
    
    /// Frame deduplication
    last_frame_hash: u64,
    
    /// Frames collected in current batch
    frame_batch: Vec<Vec<u8>>,
    
    /// Track if video frame was sent in current audio turn
    video_sent_in_audio_turn: bool,
    
    /// Store last frame data for audio turn completion
    last_frame_data: Option<Vec<u8>>,
    
    /// Media broadcast channel for force capture
    media_tx: broadcast::Sender<MediaEvent>,
    
    /// Time when we started waiting for forced frame
    force_frame_wait_start: Option<Instant>,
    
    /// Outbound message queue (drained after each event)
    outbound: Vec<WsOutbound>,
    
    /// Latency tracking
    turn_end_times: VecDeque<(Instant, bool)>, // (end_time, was_video_turn)
    recent_latencies: VecDeque<(Instant, u64)>, // (timestamp, latency_ms)
    max_latencies: usize,
    
    // === NEW BOOK-KEEPING ======================================
    last_turn_was_video: bool,           // what kind of turn ended last?
    pending_turn_types: VecDeque<bool>,  // queue parallels turn_end_times: true=video
    need_activity_reset: bool,           // do we owe Gemini a reset to NO_INTERRUPTION?
    // ===========================================================
}

impl SimpleTurnFsm {
    pub fn new(media_tx: broadcast::Sender<MediaEvent>) -> Self {
        Self {
            state: State::Idle,
            last_frame_hash: 0,
            frame_batch: Vec::new(),
            video_sent_in_audio_turn: false,
            last_frame_data: None,
            media_tx,
            force_frame_wait_start: None,
            outbound: Vec::new(),
            turn_end_times: VecDeque::new(),
            recent_latencies: VecDeque::with_capacity(100),
            max_latencies: 100,
            last_turn_was_video: false,
            pending_turn_types: VecDeque::new(),
            need_activity_reset: false,
        }
    }
    
    /// Process an event and generate output messages
    pub fn on_event(&mut self, event: Event) {
        match (&self.state, event) {
            // ===== IDLE STATE =====
            
            // Unique frame → start frame batch or send single frame
            (State::Idle, Event::Frame { jpeg, hash }) if hash != self.last_frame_hash => {
                // Always store the last frame data
                self.last_frame_data = Some(jpeg.clone());
                
                if FRAMES_PER_TURN > 1 {
                    // Start batching frames
                    info!("📹 Starting frame batch (1/{})", FRAMES_PER_TURN);
                    self.frame_batch.push(jpeg);
                    self.last_frame_hash = hash;
                    self.state = State::FrameBatch;
                } else {
                    // Single frame turn (original behavior)
                    info!("📹 Sending single video turn");
                    self.send_activity_start();
                    self.send_video(&jpeg);
                    self.send_activity_end();
                    self.last_frame_hash = hash;
                    self.last_turn_was_video = true;
                    self.turn_end_times.push_back((Instant::now(), true));
                    self.pending_turn_types.push_back(true);
                }
            }
            
            // Speech starts → begin audio turn
            (State::Idle, Event::SpeechStart) => {
                let pending_video = self.pending_turn_types.iter().any(|is_video| *is_video);
                if pending_video {
                    // Cancel the video generation that's still running.
                    info!("🚫 Interrupting pending video turn(s) for audio");
                    self.send_activity_handling_update("START_OF_ACTIVITY_INTERRUPTS");
                    self.need_activity_reset = true;
                }
                
                info!("🎤 Starting audio turn");
                self.send_activity_start();
                self.last_turn_was_video = false;      // this is an audio turn
                self.video_sent_in_audio_turn = false; // Reset flag
                self.state = State::AudioTurn;
            }
            
            // ===== FRAME BATCH STATE =====
            
            // Collect more unique frames
            (State::FrameBatch, Event::Frame { jpeg, hash }) if hash != self.last_frame_hash => {
                // Always store the last frame data
                self.last_frame_data = Some(jpeg.clone());
                self.frame_batch.push(jpeg);
                self.last_frame_hash = hash;
                
                if self.frame_batch.len() >= FRAMES_PER_TURN {
                    // Batch is full, send it
                    info!("📹 Sending frame batch ({} frames)", self.frame_batch.len());
                    self.send_activity_start();
                    let frames = std::mem::take(&mut self.frame_batch);
                    for frame in &frames {
                        self.send_video(frame);
                    }
                    self.send_activity_end();
                    self.state = State::Idle;
                    self.last_turn_was_video = true;
                    self.turn_end_times.push_back((Instant::now(), true));
                    self.pending_turn_types.push_back(true);
                } else {
                    info!("📹 Frame batch ({}/{})", self.frame_batch.len(), FRAMES_PER_TURN);
                }
            }
            
            // Speech starts while batching → send current batch and start audio turn
            (State::FrameBatch, Event::SpeechStart) => {
                if !self.frame_batch.is_empty() {
                    info!("📹 Sending partial frame batch ({} frames) before audio", self.frame_batch.len());
                    self.send_activity_start();
                    let frames = std::mem::take(&mut self.frame_batch);
                    for frame in &frames {
                        self.send_video(frame);
                    }
                    self.send_activity_end();
                    self.last_turn_was_video = true;
                    self.turn_end_times.push_back((Instant::now(), true));
                    self.pending_turn_types.push_back(true);
                }
                
                let pending_video = self.pending_turn_types.iter().any(|is_video| *is_video);
                if pending_video {
                    // Cancel the video generation that's still running.
                    info!("🚫 Interrupting pending video turn(s) for audio");
                    self.send_activity_handling_update("START_OF_ACTIVITY_INTERRUPTS");
                    self.need_activity_reset = true;
                }
                
                info!("🎤 Starting audio turn");
                self.send_activity_start();
                self.last_turn_was_video = false;      // this is an audio turn
                self.video_sent_in_audio_turn = false; // Reset flag
                self.state = State::AudioTurn;
            }
            
            // ===== AUDIO TURN STATE =====
            
            // Stream audio chunks
            (State::AudioTurn, Event::AudioChunk(pcm)) => {
                debug!("Streaming {} bytes of audio", pcm.len());
                self.send_audio(&pcm);
            }
            
            // Piggyback unique frames
            (State::AudioTurn, Event::Frame { jpeg, hash }) if hash != self.last_frame_hash => {
                info!("📹 Piggybacking video in audio turn");
                // Always store the last frame data
                self.last_frame_data = Some(jpeg.clone());
                self.send_video(&jpeg);
                self.last_frame_hash = hash;
                self.video_sent_in_audio_turn = true; // Mark that we sent a video
            }
            
            // Speech ends → wait for forced frame
            (State::AudioTurn, Event::SpeechEnd) => {
                // Force capture a fresh frame right before ending
                info!("📹 Force capturing frame before ending audio turn");
                let _ = self.media_tx.send(MediaEvent::ForceCaptureRequest {
                    requester_id: "SimpleTurnFsm::SpeechEnd".to_string(),
                });
                
                // Transition to waiting state
                info!("⏳ Waiting for forced frame before ending turn");
                self.force_frame_wait_start = Some(Instant::now());
                self.state = State::WaitingForForcedFrame;
            }
            
            // ===== WAITING FOR FORCED FRAME STATE =====
            
            // Receive the forced frame and complete the turn
            (State::WaitingForForcedFrame, Event::Frame { jpeg, hash }) => {
                info!("📹 Received forced frame, sending and ending turn");
                // Always store the frame data
                self.last_frame_data = Some(jpeg.clone());
                self.send_video(&jpeg);
                self.last_frame_hash = hash;
                
                // Now end the turn
                info!("🎤 Ending audio turn with fresh frame");
                if self.need_activity_reset {
                    // Flush the audio turn, then revert to NO_INTERRUPTION
                    self.send_activity_handling_update("NO_INTERRUPTION");
                    self.need_activity_reset = false;
                }
                self.send_activity_end();
                self.state = State::Idle;
                self.force_frame_wait_start = None; // Clear timer
                self.last_turn_was_video = false;
                
                // Track turn end time
                self.turn_end_times.push_back((Instant::now(), false));
                self.pending_turn_types.push_back(false);
            }
            
            // If speech starts while waiting, abandon wait and start new turn
            (State::WaitingForForcedFrame, Event::SpeechStart) => {
                info!("⚠️ Speech started while waiting for frame, ending previous turn");
                // Send any cached frame we have
                if let Some(frame_data) = self.last_frame_data.clone() {
                    self.send_video(&frame_data);
                }
                if self.need_activity_reset {
                    // Flush the audio turn, then revert to NO_INTERRUPTION
                    self.send_activity_handling_update("NO_INTERRUPTION");
                    self.need_activity_reset = false;
                }
                self.send_activity_end();
                self.last_turn_was_video = false;
                self.turn_end_times.push_back((Instant::now(), false));
                self.pending_turn_types.push_back(false);
                
                // Start new audio turn
                info!("🎤 Starting new audio turn");
                self.send_activity_start();
                self.last_turn_was_video = false;
                self.video_sent_in_audio_turn = false;
                self.state = State::AudioTurn;
            }
            
            // Response received - calculate latency
            (_, Event::ResponseReceived) => {
                if let Some((turn_end_time, _was_video)) = self.turn_end_times.pop_front() {
                    self.pending_turn_types.pop_front();
                    let now = Instant::now();
                    let latency = now.duration_since(turn_end_time);
                    let latency_ms = latency.as_millis() as u64;
                    
                    // Store latency
                    self.recent_latencies.push_back((now, latency_ms));
                    if self.recent_latencies.len() > self.max_latencies {
                        self.recent_latencies.pop_front();
                    }
                    
                    // Print latency report
                    self.print_latency_report(latency_ms);
                }
            }
            
            // Ignore duplicates and invalid transitions
            _ => {}
        }
    }
    
    /// Drain all pending outbound messages
    pub fn drain_messages(&mut self) -> Vec<WsOutbound> {
        std::mem::take(&mut self.outbound)
    }
    
    /// Check if we've been waiting too long for forced frame
    pub fn check_force_frame_timeout(&mut self) {
        if let State::WaitingForForcedFrame = self.state {
            if let Some(start) = self.force_frame_wait_start {
                if start.elapsed() > Duration::from_millis(FORCE_FRAME_TIMEOUT_MS) {
                    info!("⏱️ Force frame timeout ({}ms), ending turn with cached frame", FORCE_FRAME_TIMEOUT_MS);
                    
                    // Send cached frame if available
                    if let Some(frame_data) = self.last_frame_data.clone() {
                        self.send_video(&frame_data);
                    }
                    
                    // End the turn
                    if self.need_activity_reset {
                        // Flush the audio turn, then revert to NO_INTERRUPTION
                        self.send_activity_handling_update("NO_INTERRUPTION");
                        self.need_activity_reset = false;
                    }
                    self.send_activity_end();
                    self.state = State::Idle;
                    self.force_frame_wait_start = None;
                    self.last_turn_was_video = false;
                    self.turn_end_times.push_back((Instant::now(), false));
                    self.pending_turn_types.push_back(false);
                }
            }
        }
    }
    
    // === Helper methods ===
    
    fn send_activity_start(&mut self) {
        let msg = json!({ "activityStart": {} });
        self.outbound.push(WsOutbound::Json(msg));
    }
    
    fn send_activity_end(&mut self) {
        let msg = json!({ "activityEnd": {} });
        self.outbound.push(WsOutbound::Json(msg));
    }
    
    fn send_audio(&mut self, pcm: &[u8]) {
        let msg = json!({
            "audio": {
                "data": base64::engine::general_purpose::STANDARD.encode(pcm),
                "mimeType": "audio/pcm;rate=16000"
            }
        });
        self.outbound.push(WsOutbound::Json(msg));
    }
    
    fn send_video(&mut self, jpeg: &[u8]) {
        let msg = json!({
            "video": {
                "data": base64::engine::general_purpose::STANDARD.encode(jpeg),
                "mimeType": "image/jpeg"
            }
        });
        self.outbound.push(WsOutbound::Json(msg));
    }
    
    fn send_activity_handling_update(&mut self, mode: &str) {
        let msg = json!({
            "setup": {
                "realtimeInputConfig": {
                    "activityHandling": mode
                }
            }
        });
        self.outbound.push(WsOutbound::Json(msg));
    }
    
    fn print_latency_report(&self, current_latency_ms: u64) {
        let pending = self.turn_end_times.len();
        
        println!("\n========== LATENCY REPORT ==========");
        println!("Current latency:  {}ms", current_latency_ms);
        println!("Pending turns:    {}", pending);
        
        if !self.recent_latencies.is_empty() {
            let sum: u64 = self.recent_latencies.iter().map(|(_, ms)| ms).sum();
            let avg = sum / self.recent_latencies.len() as u64;
            let min = self.recent_latencies.iter().map(|(_, ms)| ms).min().unwrap();
            let max = self.recent_latencies.iter().map(|(_, ms)| ms).max().unwrap();
            
            println!("Recent {} responses:", self.recent_latencies.len());
            println!("  Min:     {}ms", min);
            println!("  Max:     {}ms", max);
            println!("  Average: {}ms", avg);
        }
        println!("====================================\n");
    }
}