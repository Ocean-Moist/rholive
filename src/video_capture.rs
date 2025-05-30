use crate::events::InEvent;
use crate::screen::{ScreenCapturer, quick_hash};
use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{interval, Duration};

const FPS: u64 = 2;

pub fn spawn(tx: UnboundedSender<InEvent>) -> Result<()> {
    tokio::spawn(async move {
        if let Err(e) = capture_loop(tx).await {
            eprintln!("Video capture error: {}", e);
        }
    });
    Ok(())
}

async fn capture_loop(tx: UnboundedSender<InEvent>) -> Result<()> {
    let mut capturer = ScreenCapturer::new()?;
    let mut ticker = interval(Duration::from_millis(1000 / FPS));
    let mut last_hash = 0u64;

    loop {
        ticker.tick().await;
        
        match capturer.capture_frame() {
            Ok(mut frame) => {
                let hash = quick_hash(&frame.frame);
                
                if hash != last_hash {
                    last_hash = hash;
                    
                    match frame.to_jpeg() {
                        Ok(jpeg_data) => {
                            let jpeg = jpeg_data.to_vec();
                            if tx.send(InEvent::UniqueFrame { jpeg, hash }).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("JPEG conversion error: {}", e);
                            continue;
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Frame capture error: {}", e);
                continue;
            }
        }
    }

    Ok(())
}