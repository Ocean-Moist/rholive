//! WebSocket writer task that serializes and sends all outgoing messages
//! This is the single point where all producers' messages are serialized to JSON

use crate::events::{Outgoing, WsIn};
use base64::Engine;
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, error};
use std::collections::VecDeque;
use std::time::Instant;

/// Latency tracking for turns
struct TurnTracker {
    pending_turns: VecDeque<(u64, Instant)>,
    completed_turns: VecDeque<(u64, std::time::Duration)>,
    max_completed: usize,
}

impl TurnTracker {
    fn new() -> Self {
        Self {
            pending_turns: VecDeque::new(),
            completed_turns: VecDeque::new(),
            max_completed: 100,
        }
    }
    
    fn start_turn(&mut self, turn_id: u64) {
        self.pending_turns.push_back((turn_id, Instant::now()));
        println!("\n>>> TURN START: ID={} | Pending={}", turn_id, self.pending_turns.len());
    }
    
    fn complete_turn(&mut self) {
        if let Some((turn_id, start_time)) = self.pending_turns.pop_front() {
            let latency = start_time.elapsed();
            self.completed_turns.push_back((turn_id, latency));
            
            if self.completed_turns.len() > self.max_completed {
                self.completed_turns.pop_front();
            }
            
            // Print latency report
            println!("\n========== LATENCY REPORT ==========");
            println!("Turn ID:          {}", turn_id);
            println!("Latency:          {:.2}s ({:.0}ms)", latency.as_secs_f32(), latency.as_millis());
            println!("Pending Turns:    {}", self.pending_turns.len());
            
            if !self.completed_turns.is_empty() {
                let sum: std::time::Duration = self.completed_turns.iter().map(|(_, d)| *d).sum();
                let avg = sum / self.completed_turns.len() as u32;
                println!("Average Latency:  {:.2}s ({:.0}ms)", avg.as_secs_f32(), avg.as_millis());
            }
            println!("====================================\n");
        }
    }
    
    fn print_summary(&self) {
        println!("\n########## LATENCY SUMMARY ##########");
        println!("Currently Pending:     {}", self.pending_turns.len());
        
        if !self.pending_turns.is_empty() {
            println!("\nPending Turn Details:");
            for (id, start_time) in &self.pending_turns {
                println!("  - Turn {} waiting for {:.2}s", id, start_time.elapsed().as_secs_f32());
            }
        }
        
        if !self.completed_turns.is_empty() {
            let latencies: Vec<_> = self.completed_turns.iter().map(|(_, d)| *d).collect();
            let min = latencies.iter().min().unwrap();
            let max = latencies.iter().max().unwrap();
            let sum: std::time::Duration = latencies.iter().sum();
            let avg = sum / latencies.len() as u32;
            
            println!("\nLatency Statistics (last {} turns):", latencies.len());
            println!("  Min:     {:.2}s ({:.0}ms)", min.as_secs_f32(), min.as_millis());
            println!("  Max:     {:.2}s ({:.0}ms)", max.as_secs_f32(), max.as_millis());
            println!("  Average: {:.2}s ({:.0}ms)", avg.as_secs_f32(), avg.as_millis());
        }
        println!("#####################################\n");
    }
}

/// Run the websocket writer task
pub async fn run_writer(
    mut outgoing_rx: UnboundedReceiver<Outgoing>,
    websocket_tx: UnboundedSender<serde_json::Value>,
    mut ws_event_rx: UnboundedReceiver<WsIn>,
) {
    info!("WebSocket writer task started");
    
    let mut tracker = TurnTracker::new();
    let mut last_summary = Instant::now();
    let summary_interval = std::time::Duration::from_secs(30);
    
    loop {
        tokio::select! {
            // Handle outgoing messages from producers
            Some(msg) = outgoing_rx.recv() => {
                let json = match msg {
                    Outgoing::ActivityStart(turn_id) => {
                        tracker.start_turn(turn_id);
                        info!("📤 Sending activityStart for turn {}", turn_id);
                        json!({"activityStart": {}})
                    }
                    Outgoing::AudioChunk(bytes, turn_id) => {
                        debug!("🎤 Sending audio chunk for turn {} ({} bytes)", turn_id, bytes.len());
                        json!({
                            "audio": {
                                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                                "mimeType": "audio/pcm;rate=16000"
                            }
                        })
                    }
                    Outgoing::VideoFrame(jpeg, turn_id) => {
                        debug!("📹 Sending video frame for turn {} ({} KB)", turn_id, jpeg.len() / 1024);
                        json!({
                            "video": {
                                "data": base64::engine::general_purpose::STANDARD.encode(&jpeg),
                                "mimeType": "image/jpeg"
                            }
                        })
                    }
                    Outgoing::ActivityEnd(turn_id) => {
                        info!("📤 Sending activityEnd for turn {}", turn_id);
                        json!({"activityEnd": {}})
                    }
                };
                
                if let Err(e) = websocket_tx.send(json) {
                    error!("Failed to send to websocket: {}", e);
                    break;
                }
            }
            
            // Handle incoming WS events for latency tracking
            Some(event) = ws_event_rx.recv() => {
                match event {
                    WsIn::GenerationComplete => {
                        tracker.complete_turn();
                    }
                    _ => {}
                }
            }
            
            // All channels closed
            else => {
                info!("WebSocket writer task shutting down");
                break;
            }
        }
        
        // Print periodic summary
        if last_summary.elapsed() >= summary_interval {
            tracker.print_summary();
            last_summary = Instant::now();
        }
    }
}