use crate::events::{InEvent, Outgoing};
use crate::screen::{ScreenCapturer, quick_hash};
use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{interval, Duration};
use tracing::{debug, info, error};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const FPS: u64 = 2;

pub fn spawn(tx: UnboundedSender<InEvent>) -> Result<()> {
    info!("🎬 Starting video capture task at {} FPS", FPS);
    tokio::spawn(async move {
        if let Err(e) = capture_loop(tx).await {
            error!("Video capture error: {}", e);
        }
    });
    Ok(())
}

pub fn spawn_with_outgoing(
    tx: UnboundedSender<InEvent>, 
    outgoing_tx: UnboundedSender<Outgoing>,
    turn_id_gen: Arc<AtomicU64>,
) -> Result<()> {
    info!("🎬 Starting video capture task at {} FPS (with outgoing channel)", FPS);
    tokio::spawn(async move {
        if let Err(e) = capture_loop_with_outgoing(tx, outgoing_tx, turn_id_gen).await {
            error!("Video capture error: {}", e);
        }
    });
    Ok(())
}

async fn capture_loop(tx: UnboundedSender<InEvent>) -> Result<()> {
    info!("🎬 Initializing video capture loop...");
    let mut capturer = ScreenCapturer::new()?;
    let mut ticker = interval(Duration::from_millis(1000 / FPS));
    let mut last_hash = 0u64;
    info!("🎬 Video capture loop started, waiting for frames...");

    loop {
        ticker.tick().await;
        debug!("⏰ Video capture tick - attempting frame capture...");


        match capturer.capture_frame() {
            Ok(mut frame) => {
                debug!("📸 Frame captured successfully, calculating hash...");
                let hash = quick_hash(&frame.frame);
                
                if hash != last_hash {
                    info!("🆕 New unique frame detected (hash: {} -> {})", last_hash, hash);
                    last_hash = hash;
                    
                    match frame.to_jpeg() {
                        Ok(jpeg_data) => {
                            let jpeg = jpeg_data.to_vec();
                            let jpeg_size_kb = jpeg.len() / 1024;
                            info!("📤 Sending UniqueFrame event: {} KB JPEG (hash: {})", jpeg_size_kb, hash);
                            if tx.send(InEvent::UniqueFrame { jpeg, hash }).is_err() {
                                error!("❌ Failed to send frame event - channel closed");
                                break;
                            }
                            debug!("✅ Frame event sent successfully");
                        }
                        Err(e) => {
                            error!("❌ JPEG conversion error: {}", e);
                            continue;
                        }
                    }
                } else {
                    debug!("🔄 Duplicate frame skipped (hash: {})", hash);
                }
            }
            Err(e) => {
                debug!("❌ Frame capture error: {}", e);
                continue;
            }
        }
    }

    Ok(())
}

async fn capture_loop_with_outgoing(
    tx: UnboundedSender<InEvent>,
    outgoing_tx: UnboundedSender<Outgoing>,
    turn_id_gen: Arc<AtomicU64>,
) -> Result<()> {
    info!("🎬 Initializing video capture loop with outgoing channel...");
    let mut capturer = ScreenCapturer::new()?;
    let mut ticker = interval(Duration::from_millis(1000 / FPS));
    let mut last_hash = 0u64;
    let mut current_turn_id: Option<u64> = None;
    info!("🎬 Video capture loop started, waiting for frames...");

    loop {
        ticker.tick().await;
        debug!("⏰ Video capture tick - attempting frame capture...");

        match capturer.capture_frame() {
            Ok(mut frame) => {
                debug!("📸 Frame captured successfully, calculating hash...");
                let hash = quick_hash(&frame.frame);
                
                if hash != last_hash {
                    info!("🆕 New unique frame detected (hash: {} -> {})", last_hash, hash);
                    last_hash = hash;
                    
                    match frame.to_jpeg() {
                        Ok(jpeg_data) => {
                            let jpeg = jpeg_data.to_vec();
                            let jpeg_size_kb = jpeg.len() / 1024;
                            
                            // Get or create turn ID for this frame
                            let turn_id = current_turn_id.unwrap_or_else(|| {
                                let id = turn_id_gen.load(Ordering::SeqCst).saturating_sub(1);
                                if id == 0 {
                                    // No active turn yet, frames will be queued
                                    0
                                } else {
                                    id
                                }
                            });
                            
                            // Send via new outgoing channel
                            info!("📤 Sending video frame for turn {}: {} KB JPEG (hash: {})", 
                                  turn_id, jpeg_size_kb, hash);
                            if outgoing_tx.send(Outgoing::VideoFrame(jpeg.clone(), turn_id)).is_err() {
                                error!("❌ Failed to send frame via outgoing channel - channel closed");
                                break;
                            }
                            
                            // Also send legacy event
                            if tx.send(InEvent::UniqueFrame { jpeg, hash }).is_err() {
                                error!("❌ Failed to send frame event - channel closed");
                                break;
                            }
                            debug!("✅ Frame sent successfully");
                        }
                        Err(e) => {
                            error!("❌ JPEG conversion error: {}", e);
                            continue;
                        }
                    }
                } else {
                    debug!("🔄 Duplicate frame skipped (hash: {})", hash);
                }
            }
            Err(e) => {
                debug!("❌ Frame capture error: {}", e);
                continue;
            }
        }
    }

    Ok(())
}