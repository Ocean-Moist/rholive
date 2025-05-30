//! Async audio capture module using PulseAudio's threaded mainloop
//! 
//! This provides proper async integration with tokio by using PulseAudio's
//! threaded mainloop and callback-based API.

use libpulse_binding as pulse;
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::mainloop::threaded::Mainloop;
use pulse::proplist::Proplist;
use pulse::sample::{Format, Spec};
use pulse::stream::{FlagSet as StreamFlagSet, State as StreamState, Stream};
use std::cell::RefCell;
use std::error::Error;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Async audio capturer using PulseAudio's threaded mainloop
pub struct AsyncAudioCapturer {
    /// Channel for receiving audio chunks
    rx: mpsc::Receiver<Vec<i16>>,
    /// Shutdown flag
    shutdown: Arc<AtomicBool>,
    /// Handle to the background thread
    _handle: std::thread::JoinHandle<()>,
}

impl AsyncAudioCapturer {
    /// Create a new async audio capturer
    pub fn new(app_name: &str, device_name: Option<&str>) -> Result<Self, Box<dyn Error>> {
        let (tx, rx) = mpsc::channel::<Vec<i16>>(32);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        
        let app_name = app_name.to_string();
        let device_name = device_name.map(|s| s.to_string());
        
        // Spawn the audio capture thread (not a tokio task, a real OS thread)
        let handle = std::thread::spawn(move || {
            if let Err(e) = run_audio_capture(app_name, device_name, tx, shutdown_clone) {
                error!("Audio capture error: {}", e);
            }
        });
        
        Ok(Self {
            rx,
            shutdown,
            _handle: handle,
        })
    }
    
    /// Read the next chunk of audio data (100ms worth)
    /// Returns None if the capture has ended
    pub async fn read_chunk(&mut self) -> Option<Vec<i16>> {
        self.rx.recv().await
    }
    
    /// Get the device name being used
    pub fn device_name(&self) -> &str {
        "pulse" // TODO: track actual device name
    }
}

impl Drop for AsyncAudioCapturer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Thread will exit when it sees shutdown flag
    }
}

/// Run the audio capture in a dedicated thread with PulseAudio's threaded mainloop
fn run_audio_capture(
    app_name: String,
    device_name: Option<String>,
    tx: mpsc::Sender<Vec<i16>>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    // Create the mainloop
    let mainloop = Rc::new(RefCell::new(
        Mainloop::new().ok_or("Failed to create mainloop")?
    ));
    
    // Create property list for the application
    let mut proplist = Proplist::new().ok_or("Failed to create proplist")?;
    proplist.set_str(pulse::proplist::properties::APPLICATION_NAME, &app_name)
        .map_err(|()| "Failed to set application name")?;
    
    // Create context
    let context = Rc::new(RefCell::new(
        Context::new_with_proplist(
            mainloop.borrow().deref(),
            "AudioContext",
            &proplist
        ).ok_or("Failed to create context")?
    ));
    
    // Set state callback to know when we're connected
    let ml_ref = mainloop.clone();
    let context_ref = context.clone();
    context.borrow_mut().set_state_callback(Some(Box::new(move || {
        let state = unsafe { (*context_ref.as_ptr()).get_state() };
        match state {
            ContextState::Ready => {
                let ml = unsafe { &mut *ml_ref.as_ptr() };
                ml.signal(false);
            }
            ContextState::Failed | ContextState::Terminated => {
                let ml = unsafe { &mut *ml_ref.as_ptr() };
                ml.signal(false);
            }
            _ => {}
        }
    })));
    
    // Connect the context
    mainloop.borrow_mut().lock();
    context.borrow_mut().connect(None, ContextFlagSet::NOFLAGS, None)
        .map_err(|e| format!("Failed to connect context: {:?}", e))?;
    mainloop.borrow_mut().unlock();
    
    // Start the mainloop
    mainloop.borrow_mut().start()
        .map_err(|e| format!("Failed to start mainloop: {:?}", e))?;
    
    // Wait for context to be ready
    mainloop.borrow_mut().lock();
    loop {
        match context.borrow().get_state() {
            ContextState::Ready => break,
            ContextState::Failed | ContextState::Terminated => {
                mainloop.borrow_mut().unlock();
                mainloop.borrow_mut().stop();
                return Err("Context connection failed".into());
            }
            _ => {
                mainloop.borrow_mut().wait();
            }
        }
    }
    mainloop.borrow_mut().unlock();
    
    info!("PulseAudio context connected");
    
    // Create the recording stream - 16kHz mono S16LE
    let spec = Spec {
        format: Format::S16le,
        channels: 1,
        rate: 16000,
    };
    
    let stream = Rc::new(RefCell::new(
        Stream::new(
            &mut context.borrow_mut(),
            "AudioStream",
            &spec,
            None
        ).ok_or("Failed to create stream")?
    ));
    
    // Buffer for accumulating samples
    let buffer = Rc::new(RefCell::new(Vec::<i16>::with_capacity(1600)));
    
    // Set up the read callback
    let tx_clone = tx.clone();
    let ml_ref = mainloop.clone();
    let stream_ref = stream.clone();
    let buffer_ref = buffer.clone();
    let shutdown_ref = shutdown.clone();
    
    stream.borrow_mut().set_read_callback(Some(Box::new(move |length| {
        if length == 0 {
            return;
        }
        
        // Check for shutdown
        if shutdown_ref.load(Ordering::Relaxed) {
            unsafe {
                let ml = &mut *ml_ref.as_ptr();
                ml.stop();
            }
            return;
        }
        
        // Peek at the data
        let peek_result = unsafe {
            let stream = &mut *stream_ref.as_ptr();
            stream.peek()
        };
        
        match peek_result {
            Ok(pulse::stream::PeekResult::Data(data)) => {
                if !data.is_empty() {
                    // Convert bytes to i16 samples
                    let samples: Vec<i16> = data.chunks_exact(2)
                        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                        .collect();
                    
                    // Accumulate in buffer
                    unsafe {
                        let buffer = &mut *buffer_ref.as_ptr();
                        buffer.extend_from_slice(&samples);
                        
                        // Send complete 100ms chunks (1600 samples)
                        while buffer.len() >= 1600 {
                            let chunk: Vec<i16> = buffer.drain(..1600).collect();
                            // Use blocking send since we're in a thread
                            if tx_clone.blocking_send(chunk).is_err() {
                                // Receiver dropped, initiate shutdown
                                let ml = &mut *ml_ref.as_ptr();
                                ml.stop();
                                return;
                            }
                        }
                    }
                    
                    // Discard the data from the stream
                    unsafe {
                        let stream = &mut *stream_ref.as_ptr();
                        let _ = stream.discard();
                    }
                }
            }
            Ok(pulse::stream::PeekResult::Empty) => {
                // No data available
            }
            Ok(pulse::stream::PeekResult::Hole(_)) => {
                // There's a hole in the buffer, skip it
                unsafe {
                    let stream = &mut *stream_ref.as_ptr();
                    let _ = stream.discard();
                }
            }
            Err(e) => {
                error!("Failed to peek stream data: {:?}", e);
            }
        }
    })));
    
    // Set stream state callback
    let ml_ref = mainloop.clone();
    let stream_ref = stream.clone();
    stream.borrow_mut().set_state_callback(Some(Box::new(move || {
        let state = unsafe {
            let stream = &*stream_ref.as_ptr();
            stream.get_state()
        };
        match state {
            StreamState::Ready => {
                info!("Stream ready");
                unsafe {
                    let ml = &mut *ml_ref.as_ptr();
                    ml.signal(false);
                }
            }
            StreamState::Failed | StreamState::Terminated => {
                error!("Stream failed/terminated");
                unsafe {
                    let ml = &mut *ml_ref.as_ptr();
                    ml.signal(false);
                }
            }
            _ => {}
        }
    })));
    
    // Set buffer attributes for low latency
    let buffer_attr = pulse::def::BufferAttr {
        maxlength: 16000, // 1 second max
        tlength: std::u32::MAX,
        prebuf: std::u32::MAX,
        minreq: std::u32::MAX,
        fragsize: 3200, // 100ms chunks (1600 samples * 2 bytes)
    };
    
    // Connect the stream for recording
    mainloop.borrow_mut().lock();
    stream.borrow_mut().connect_record(
        device_name.as_deref(),
        Some(&buffer_attr),
        StreamFlagSet::ADJUST_LATENCY | StreamFlagSet::AUTO_TIMING_UPDATE
    ).map_err(|e| format!("Failed to connect recording stream: {:?}", e))?;
    mainloop.borrow_mut().unlock();
    
    // Wait for stream to be ready
    mainloop.borrow_mut().lock();
    loop {
        match stream.borrow().get_state() {
            StreamState::Ready => break,
            StreamState::Failed | StreamState::Terminated => {
                mainloop.borrow_mut().unlock();
                mainloop.borrow_mut().stop();
                return Err("Stream connection failed".into());
            }
            _ => {
                mainloop.borrow_mut().wait();
            }
        }
    }
    mainloop.borrow_mut().unlock();
    
    info!("Audio stream ready, starting capture");
    
    // The threaded mainloop runs in its own thread
    // We just need to wait for shutdown signal
    loop {
        std::thread::sleep(Duration::from_millis(100));
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
    }
    
    // Cleanup
    mainloop.borrow_mut().lock();
    stream.borrow_mut().disconnect().ok();
    context.borrow_mut().disconnect();
    mainloop.borrow_mut().unlock();
    mainloop.borrow_mut().stop();
    
    Ok(())
}