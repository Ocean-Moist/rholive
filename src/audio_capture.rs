use crate::events::InEvent;
use anyhow::Result;
use libpulse_binding::sample::{Format, Spec};
use libpulse_simple_binding::Simple;
use libpulse_binding::stream::Direction;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{interval, Duration};

pub fn spawn(tx: UnboundedSender<InEvent>) -> Result<()> {
    tokio::spawn(async move {
        if let Err(e) = capture_loop(tx).await {
            eprintln!("Audio capture error: {}", e);
        }
    });
    Ok(())
}

pub fn spawn_with_dual_output(tx1: UnboundedSender<InEvent>, tx2: UnboundedSender<InEvent>) -> Result<()> {
    tokio::spawn(async move {
        if let Err(e) = capture_loop_dual(tx1, tx2).await {
            eprintln!("Audio capture error: {}", e);
        }
    });
    Ok(())
}

async fn capture_loop(tx: UnboundedSender<InEvent>) -> Result<()> {
    let spec = Spec {
        format: Format::S16le,
        channels: 1,
        rate: 16000,
    };

    let simple = Simple::new(
        None,
        "rholive",
        Direction::Record,
        None,
        "record",
        &spec,
        None,
        None,
    )?;

    let mut ticker = interval(Duration::from_millis(20));
    let samples_per_chunk = 320;

    loop {
        ticker.tick().await;
        
        let mut buffer = vec![0i16; samples_per_chunk];
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                buffer.as_mut_ptr() as *mut u8,
                buffer.len() * 2,
            )
        };

        match simple.read(bytes) {
            Ok(_) => {
                if tx.send(InEvent::AudioChunk(buffer)).is_err() {
                    break;
                }
            }
            Err(e) => {
                eprintln!("Audio read error: {}", e);
                continue;
            }
        }
    }

    Ok(())
}

async fn capture_loop_dual(tx1: UnboundedSender<InEvent>, tx2: UnboundedSender<InEvent>) -> Result<()> {
    let spec = Spec {
        format: Format::S16le,
        channels: 1,
        rate: 16000,
    };

    let simple = Simple::new(
        None,
        "rholive",
        Direction::Record,
        None,
        "record",
        &spec,
        None,
        None,
    )?;

    let mut ticker = interval(Duration::from_millis(20));
    let samples_per_chunk = 320;

    loop {
        ticker.tick().await;
        
        let mut buffer = vec![0i16; samples_per_chunk];
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                buffer.as_mut_ptr() as *mut u8,
                buffer.len() * 2,
            )
        };

        match simple.read(bytes) {
            Ok(_) => {
                let event = InEvent::AudioChunk(buffer);
                if tx1.send(event.clone()).is_err() || tx2.send(event).is_err() {
                    break;
                }
            }
            Err(e) => {
                eprintln!("Audio read error: {}", e);
                continue;
            }
        }
    }

    Ok(())
}