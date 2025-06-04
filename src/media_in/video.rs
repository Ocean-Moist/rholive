//! Video capture with built-in deduplication

use crate::media_event::MediaEvent;
use crate::screen::{ScreenCapturer, quick_hash};
use anyhow::Result;
use tokio::sync::broadcast;
use tokio::time::{interval, Duration};
use std::time::Instant;
use tracing::{debug, error, info};
use std::sync::atomic::{AtomicU64, Ordering};

const FRAME_INTERVAL_MS: u64 = 500; // Capture a frame every .5 seconds

pub fn spawn_video_capture(tx: broadcast::Sender<MediaEvent>) -> Result<()> {
    info!("Starting video capture every {}ms", FRAME_INTERVAL_MS);
    
    tokio::spawn(async move {
        if let Err(e) = capture_loop(tx).await {
            error!("Video capture error: {}", e);
        }
    });
    
    Ok(())
}

async fn capture_loop(tx: broadcast::Sender<MediaEvent>) -> Result<()> {
    let mut capturer = ScreenCapturer::new()?;
    let mut ticker = interval(Duration::from_millis(FRAME_INTERVAL_MS));
    let mut last_hash = 0u64;
    let frame_counter = AtomicU64::new(0);
    
    // Subscribe to our own broadcast to listen for force capture requests
    let mut rx = tx.subscribe();
    
    info!("Video capture loop started");
    
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // Regular capture at FPS rate
                capture_and_send_frame(&mut capturer, &tx, &mut last_hash, &frame_counter, false);
            }
            
            Ok(event) = rx.recv() => {
                // Handle force capture requests
                if let MediaEvent::ForceCaptureRequest { requester_id } = event {
                    info!("Force capture requested by: {}", requester_id);
                    // Force capture always sends, ignoring deduplication
                    capture_and_send_frame(&mut capturer, &tx, &mut last_hash, &frame_counter, true);
                }
            }
        }
    }
}

fn capture_and_send_frame(
    capturer: &mut ScreenCapturer,
    tx: &broadcast::Sender<MediaEvent>,
    last_hash: &mut u64,
    frame_counter: &AtomicU64,
    force: bool,
) {
    // Use force_capture_frame when forced to bypass throttling
    let result = if force {
        capturer.force_capture_frame()
    } else {
        capturer.capture_frame()
    };
    
    match result {
        Ok(mut frame) => {
            let hash = quick_hash(&frame.frame);
            
            // Send if frame changed OR if forced
            // disable deduplication for testing
            // if hash != *last_hash || force {
            if true {
                *last_hash = hash;
                
                match frame.to_jpeg() {
                    Ok(jpeg_data) => {
                        let jpeg = jpeg_data.to_vec();
                        let frame_id = frame_counter.fetch_add(1, Ordering::SeqCst);
                        
                        info!("{} frame #{}: {} KB (hash: {})", 
                              if force { "Forced" } else { "New" },
                              frame_id, jpeg.len() / 1024, hash);
                        
                        let event = MediaEvent::VideoFrame {
                            jpeg,
                            frame_id,
                            timestamp: Instant::now(),
                        };
                        
                        // It's ok if there are no subscribers
                        let _ = tx.send(event);
                    }
                    Err(e) => {
                        error!("JPEG conversion error: {}", e);
                    }
                }
            } else {
                debug!("Duplicate frame skipped");
            }
        }
        Err(e) => {
            debug!("Frame capture error: {}", e);
        }
    }
}