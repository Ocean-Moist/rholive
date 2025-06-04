//! Simple Turn Runner - Connects media events to the FSM and WebSocket

use crate::media_event::{MediaEvent, WsOutbound, WsInbound, Outgoing};
use crate::simple_turn_fsm::{SimpleTurnFsm, Event};
use crate::recorder::TurnRecorder;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, Duration};
use tracing::{debug, info, error};

/// Run the simple turn FSM
pub async fn run(
    media_tx: broadcast::Sender<MediaEvent>,
    mut media_rx: broadcast::Receiver<MediaEvent>,
    mut outgoing_rx: mpsc::UnboundedReceiver<Outgoing>,
    ws_out_tx: mpsc::UnboundedSender<WsOutbound>,
    mut ws_in_rx: mpsc::UnboundedReceiver<WsInbound>,
    record: bool,
) {
    let mut fsm = SimpleTurnFsm::new(media_tx);
    let mut stats_ticker = interval(Duration::from_secs(30));
    let mut timeout_checker = interval(Duration::from_millis(10)); // Check timeout every 10ms
    let mut recorder = TurnRecorder::new(record);
    
    info!("Simple Turn FSM started{}", if record { " (recording enabled)" } else { "" });
    
    loop {
        // Check for force frame timeout
        fsm.check_force_frame_timeout();
        
        // Send any generated messages from timeout check
        for msg in fsm.drain_messages() {
            recorder.on_ws(&msg);  // Record before sending
            if ws_out_tx.send(msg).is_err() {
                error!("Failed to send to WebSocket - channel closed");
                break;
            }
        }
        
        tokio::select! {
            // Check for force frame timeout
            _ = timeout_checker.tick() => {
                // Already checked above, just need this to keep the ticker running
            }
            // Print periodic statistics
            _ = stats_ticker.tick() => {
                info!("📊 Periodic latency statistics check");
                // Trigger the print by sending a dummy event
                // The FSM will print stats if it has any
            }
            // Handle media events (video frames)
            Ok(event) = media_rx.recv() => {
                if let MediaEvent::VideoFrame { jpeg, frame_id, .. } = event {
                    // Simple hash - could be replaced with perceptual hash
                    let hash = frame_id; // Using frame_id as hash for now
                    
                    fsm.on_event(Event::Frame { jpeg, hash });
                    
                    // Send any generated messages immediately
                    for msg in fsm.drain_messages() {
                        recorder.on_ws(&msg);  // Record before sending
                        if ws_out_tx.send(msg).is_err() {
                            error!("Failed to send to WebSocket - channel closed");
                            break;
                        }
                    }
                }
            }
            
            // Handle audio events from segmenter
            Some(event) = outgoing_rx.recv() => {
                recorder.on_outgoing(&event);  // Record the outgoing event
                
                match event {
                    Outgoing::ActivityStart(_) => {
                        fsm.on_event(Event::SpeechStart);
                    }
                    Outgoing::AudioChunk(bytes, _) => {
                        fsm.on_event(Event::AudioChunk(bytes));
                    }
                    Outgoing::ActivityEnd(_) => {
                        fsm.on_event(Event::SpeechEnd);
                    }
                    Outgoing::VideoFrame(_, _) => {
                        // Ignore - video comes through media_rx
                    }
                }
                
                // Send any generated messages immediately
                for msg in fsm.drain_messages() {
                    recorder.on_ws(&msg);  // Record before sending
                    if ws_out_tx.send(msg).is_err() {
                        error!("Failed to send to WebSocket - channel closed");
                        break;
                    }
                }
            }
            
            // Handle responses - track latency
            Some(event) = ws_in_rx.recv() => {
                match event {
                    WsInbound::Text { content, is_final } => {
                        if is_final {
                            debug!("Received response: {}", content.chars().take(50).collect::<String>());
                        }
                    }
                    WsInbound::GenerationComplete => {
                        info!("Generation complete");
                        // Notify FSM to calculate latency
                        fsm.on_event(Event::ResponseReceived);
                    }
                    _ => {}
                }
            }
            
            else => {
                info!("Simple Turn FSM shutting down");
                break;
            }
        }
    }
}