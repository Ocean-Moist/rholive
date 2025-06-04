//! Async audio capture using PulseAudio with support for both microphone and system audio

use crate::media_event::MediaEvent;
use anyhow::{Context, Result};
use libpulse_binding as pulse;
use libpulse_simple_binding as psimple;
use tokio::sync::broadcast;
use std::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error, info, warn};

const SAMPLE_RATE: u32 = 16000;
const CHANNELS: u8 = 1;
const CHUNK_DURATION_MS: u64 = 20;
const SAMPLES_PER_CHUNK: usize = (SAMPLE_RATE as u64 * CHUNK_DURATION_MS / 1000) as usize;

/// Audio source configuration
#[derive(Debug, Clone, Copy)]
pub enum AudioSource {
    /// Capture from microphone only
    Microphone,
    /// Capture from system audio only (what you hear)
    System,
    /// Capture from both and mix them
    Both,
}

impl Default for AudioSource {
    fn default() -> Self {
        AudioSource::Both  // Default to both microphone and system audio
    }
}

pub fn spawn_audio_capture(tx: broadcast::Sender<MediaEvent>) -> Result<()> {
    spawn_audio_capture_with_source(tx, AudioSource::default())
}

pub fn spawn_audio_capture_with_source(
    tx: broadcast::Sender<MediaEvent>, 
    source: AudioSource
) -> Result<()> {
    info!("Starting audio capture at {}Hz, {}ms chunks, source: {:?}", 
          SAMPLE_RATE, CHUNK_DURATION_MS, source);
    
    match source {
        AudioSource::Microphone => {
            std::thread::spawn(move || {
                if let Err(e) = capture_microphone(tx) {
                    error!("Microphone capture error: {}", e);
                }
            });
        }
        AudioSource::System => {
            std::thread::spawn(move || {
                if let Err(e) = capture_system_audio(tx) {
                    error!("System audio capture error: {}", e);
                }
            });
        }
        AudioSource::Both => {
            let tx1 = tx.clone();
            
            // Use shared flags to coordinate the mixer
            let mic_ready = Arc::new(AtomicBool::new(false));
            let sys_ready = Arc::new(AtomicBool::new(false));
            let mic_ready_clone = mic_ready.clone();
            let sys_ready_clone = sys_ready.clone();
            
            // Spawn mixer thread
            let (mic_tx, mic_rx) = std::sync::mpsc::channel();
            let (sys_tx, sys_rx) = std::sync::mpsc::channel();
            
            std::thread::spawn(move || {
                if let Err(e) = audio_mixer(mic_rx, sys_rx, tx1, mic_ready, sys_ready) {
                    error!("Audio mixer error: {}", e);
                }
            });
            
            // Spawn microphone capture
            std::thread::spawn(move || {
                if let Err(e) = capture_microphone_to_channel(mic_tx, mic_ready_clone) {
                    error!("Microphone capture error: {}", e);
                }
            });
            
            // Spawn system audio capture
            std::thread::spawn(move || {
                if let Err(e) = capture_system_audio_to_channel(sys_tx, sys_ready_clone) {
                    error!("System audio capture error: {}", e);
                }
            });
        }
    }
    
    Ok(())
}

fn capture_microphone(tx: broadcast::Sender<MediaEvent>) -> Result<()> {
    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::S16le,
        channels: CHANNELS,
        rate: SAMPLE_RATE,
    };
    
    let capture = psimple::Simple::new(
        None,                   // Use default server
        "rholive_mic",         // Application name
        pulse::stream::Direction::Record,
        None,                   // Use default device (microphone)
        "microphone",          // Stream description
        &spec,
        None,                   // Use default channel map
        None,                   // Use default buffering attributes
    ).context("Failed to create PulseAudio microphone connection")?;
    
    info!("Microphone capture connected successfully");
    capture_audio_stream(capture, tx, "microphone")
}

fn capture_system_audio(tx: broadcast::Sender<MediaEvent>) -> Result<()> {
    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::S16le,
        channels: CHANNELS,
        rate: SAMPLE_RATE,
    };
    
    // Get the default sink monitor
    let device = get_default_monitor_source()?;
    info!("Attempting to use system audio monitor: {:?}", device);
    
    let capture = match psimple::Simple::new(
        None,                   // Use default server
        "rholive_system",      // Application name
        pulse::stream::Direction::Record,
        Some(&device),         // Use monitor device
        "system_audio",        // Stream description
        &spec,
        None,                   // Use default channel map
        None,                   // Use default buffering attributes
    ) {
        Ok(capture) => {
            info!("System audio capture connected successfully to monitor: {}", device);
            capture
        }
        Err(e) => {
            warn!("Failed to connect to monitor source '{}': {}", device, e);
            warn!("Falling back to default source (may not capture system audio)");
            
            // Try without specifying device (will use default microphone)
            psimple::Simple::new(
                None,
                "rholive_system_fallback",
                pulse::stream::Direction::Record,
                None,  // Use default device
                "system_audio_fallback",
                &spec,
                None,
                None,
            ).context("Failed to create any PulseAudio connection")?
        }
    };
    
    capture_audio_stream(capture, tx, "system")
}

fn capture_microphone_to_channel(
    tx: std::sync::mpsc::Sender<Vec<i16>>, 
    ready: Arc<AtomicBool>
) -> Result<()> {
    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::S16le,
        channels: CHANNELS,
        rate: SAMPLE_RATE,
    };
    
    let capture = psimple::Simple::new(
        None,
        "rholive_mic",
        pulse::stream::Direction::Record,
        None,
        "microphone",
        &spec,
        None,
        None,
    ).context("Failed to create PulseAudio microphone connection")?;
    
    info!("Microphone capture for mixer connected");
    ready.store(true, Ordering::SeqCst);
    capture_to_channel(capture, tx, "microphone")
}

fn capture_system_audio_to_channel(
    tx: std::sync::mpsc::Sender<Vec<i16>>, 
    ready: Arc<AtomicBool>
) -> Result<()> {
    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::S16le,
        channels: CHANNELS,
        rate: SAMPLE_RATE,
    };
    
    let device = get_default_monitor_source()?;
    
    let capture = match psimple::Simple::new(
        None,
        "rholive_system",
        pulse::stream::Direction::Record,
        Some(&device),
        "system_audio",
        &spec,
        None,
        None,
    ) {
        Ok(capture) => {
            info!("System audio capture for mixer connected to monitor: {}", device);
            capture
        }
        Err(e) => {
            warn!("Failed to connect to monitor source '{}': {}", device, e);
            warn!("System audio mixing disabled - using microphone only");
            
            // Signal ready but don't capture - mixer will use silence
            ready.store(true, Ordering::SeqCst);
            
            // Sleep forever to keep thread alive
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    };
    
    ready.store(true, Ordering::SeqCst);
    capture_to_channel(capture, tx, "system")
}

fn capture_audio_stream(
    capture: psimple::Simple,
    tx: broadcast::Sender<MediaEvent>,
    _source_name: &str,
) -> Result<()> {
    let mut buffer = vec![0i16; SAMPLES_PER_CHUNK];
    let bytes_per_chunk = SAMPLES_PER_CHUNK * 2;
    
    loop {
        let timestamp = Instant::now();
        
        // Read exactly one chunk worth of audio
        let mut bytes = vec![0u8; bytes_per_chunk];
        capture.read(&mut bytes).context("Failed to read audio")?;
        
        // Convert bytes to i16 samples
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            buffer[i] = i16::from_le_bytes([chunk[0], chunk[1]]);
        }
        
        // Broadcast to all subscribers
        let event = MediaEvent::AudioFrame {
            pcm: buffer.clone(),
            timestamp,
        };
        
        // It's ok if there are no subscribers
        let _ = tx.send(event);
    }
}

fn capture_to_channel(
    capture: psimple::Simple,
    tx: std::sync::mpsc::Sender<Vec<i16>>,
    source_name: &str,
) -> Result<()> {
    let mut buffer = vec![0i16; SAMPLES_PER_CHUNK];
    let bytes_per_chunk = SAMPLES_PER_CHUNK * 2;
    
    loop {
        // Read exactly one chunk worth of audio
        let mut bytes = vec![0u8; bytes_per_chunk];
        capture.read(&mut bytes).context("Failed to read audio")?;
        
        // Convert bytes to i16 samples
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            buffer[i] = i16::from_le_bytes([chunk[0], chunk[1]]);
        }
        
        // Send to mixer
        if tx.send(buffer.clone()).is_err() {
            warn!("{} channel closed, exiting", source_name);
            break;
        }
    }
    
    Ok(())
}

fn audio_mixer(
    mic_rx: std::sync::mpsc::Receiver<Vec<i16>>,
    sys_rx: std::sync::mpsc::Receiver<Vec<i16>>,
    tx: broadcast::Sender<MediaEvent>,
    mic_ready: Arc<AtomicBool>,
    sys_ready: Arc<AtomicBool>,
) -> Result<()> {
    use std::sync::mpsc::TryRecvError;
    use std::collections::VecDeque;
    
    // Wait for both sources to be ready
    while !mic_ready.load(Ordering::SeqCst) || !sys_ready.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    
    info!("Audio mixer started, both sources ready");
    
    // Buffers for each stream to handle timing differences
    let mut mic_buffer: VecDeque<i16> = VecDeque::with_capacity(SAMPLES_PER_CHUNK * 10);
    let mut sys_buffer: VecDeque<i16> = VecDeque::with_capacity(SAMPLES_PER_CHUNK * 10);
    
    // Timing control
    let chunk_duration = std::time::Duration::from_millis(CHUNK_DURATION_MS);
    let mut next_output_time = Instant::now() + chunk_duration;
    
    loop {
        // Collect all available audio from both sources without blocking
        loop {
            match mic_rx.try_recv() {
                Ok(audio) => {
                    mic_buffer.extend(audio.into_iter());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    error!("Microphone channel disconnected");
                    return Err(anyhow::anyhow!("Microphone channel disconnected"));
                }
            }
        }
        
        loop {
            match sys_rx.try_recv() {
                Ok(audio) => {
                    sys_buffer.extend(audio.into_iter());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    error!("System audio channel disconnected");
                    return Err(anyhow::anyhow!("System audio channel disconnected"));
                }
            }
        }
        
        // Wait until it's time to output the next chunk
        let now = Instant::now();
        if now < next_output_time {
            std::thread::sleep(next_output_time - now);
        }
        next_output_time += chunk_duration;
        
        // Generate output chunk
        let mut mixed = vec![0i16; SAMPLES_PER_CHUNK];
        
        for i in 0..SAMPLES_PER_CHUNK {
            let mic_sample = if mic_buffer.len() > i {
                mic_buffer[i] as i32
            } else {
                0
            };
            
            let sys_sample = if sys_buffer.len() > i {
                sys_buffer[i] as i32
            } else {
                0
            };
            
            // Mix with slight attenuation to prevent clipping
            mixed[i] = ((mic_sample * 7 + sys_sample * 3) / 10) as i16;
        }
        
        // Remove consumed samples
        mic_buffer.drain(..SAMPLES_PER_CHUNK.min(mic_buffer.len()));
        sys_buffer.drain(..SAMPLES_PER_CHUNK.min(sys_buffer.len()));
        
        // Send mixed audio
        let event = MediaEvent::AudioFrame {
            pcm: mixed,
            timestamp: Instant::now(),
        };
        
        let _ = tx.send(event);
        
        // Log buffer status occasionally
        static mut LOG_COUNTER: u32 = 0;
        unsafe {
            LOG_COUNTER += 1;
            if LOG_COUNTER % 250 == 0 {  // Every 5 seconds
                debug!("Audio mixer buffers - mic: {} samples, sys: {} samples", 
                       mic_buffer.len(), sys_buffer.len());
            }
        }
    }
}

fn get_default_monitor_source() -> Result<String> {
    // Try different common monitor source names
    // Most systems will have one of these
    let monitor_sources = vec![
        "@DEFAULT_MONITOR@",  // PulseAudio 15+ syntax
        "auto_null.monitor",  // Common fallback
        "0",                  // Sometimes the first source is the monitor
    ];
    
    // For now, try the modern syntax first
    // TODO: Use pulse::context to enumerate actual sources
    Ok(monitor_sources[0].to_string())
}