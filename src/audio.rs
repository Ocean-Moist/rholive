//! Audio capture module
//!
//! Provides functionality to capture audio from system sources using PulseAudio.
//! The captured audio is in 16-bit little-endian PCM format at 16 kHz, which is
//! compatible with the Gemini Live API requirements.

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::{Context, FlagSet as ContextFlagSet};
use libpulse_binding::def::Retval;
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::proplist::Proplist;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

/// Represents an audio device
#[derive(Debug, Clone)]
pub struct AudioDevice {
    /// Device name (PulseAudio source name)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Sample rate
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u8,
    /// Is this device a monitor (system playback) or a microphone
    pub is_monitor: bool,
}

/// Audio device type for easy filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// Microphone or other input device
    Microphone,
    /// Monitor of system audio output
    Monitor,
    /// Any device type
    Any,
}

/// Custom error for audio operations
#[derive(Debug)]
pub enum AudioError {
    /// No audio devices were found
    NoDevicesFound,
    /// Failed to create PulseAudio context
    PulseContextError(String),
    /// Failed to connect to PulseAudio
    ConnectionError(String),
    /// Operation error
    OperationError(String),
    /// Other error
    Other(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevicesFound => write!(f, "No audio devices found"),
            AudioError::PulseContextError(msg) => write!(f, "PulseAudio context error: {}", msg),
            AudioError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
            AudioError::OperationError(msg) => write!(f, "Operation error: {}", msg),
            AudioError::Other(msg) => write!(f, "Audio error: {}", msg),
        }
    }
}

impl Error for AudioError {}

/// Captures audio from the default system source using PulseAudio's
/// simple API. The audio is 16-bit little-endian PCM at 16 kHz.
pub struct AudioCapturer {
    simple: Simple,
    /// Current device name
    device_name: Option<String>,
}

impl AudioCapturer {
    /// Create a new `AudioCapturer` using the default device.
    pub fn new(app_name: &str) -> Result<Self, Box<dyn Error>> {
        let spec = Spec {
            format: Format::S16le,
            channels: 1,
            rate: 16_000,
        };
        let simple = Simple::new(
            None,     // default server
            app_name, // application name
            Direction::Record,
            None,     // default device
            "record", // stream description
            &spec,
            None, // default channel map
            None, // default buffering
        )?;

        Ok(Self {
            simple,
            device_name: None,
        })
    }

    /// Create a new `AudioCapturer` using a specific input device.
    pub fn with_device(app_name: &str, device_name: &str) -> Result<Self, Box<dyn Error>> {
        info!("Creating audio capturer with device: {}", device_name);
        let spec = Spec {
            format: Format::S16le,
            channels: 1,
            rate: 16_000,
        };
        let simple = Simple::new(
            None,     // default server
            app_name, // application name
            Direction::Record,
            Some(device_name), // specific device
            "record",          // stream description
            &spec,
            None, // default channel map
            None, // default buffering
        )?;

        Ok(Self {
            simple,
            device_name: Some(device_name.to_string()),
        })
    }

    /// Read a chunk of PCM data into the provided buffer.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<(), Box<dyn Error>> {
        self.simple.read(buffer)?;
        Ok(())
    }

    /// Get the current device name, if any
    pub fn device_name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }

    /// Create a new AudioCapturer with automatic device fallback.
    /// Tries each device type in order until one works.
    pub fn with_fallback(app_name: &str) -> Result<Self, Box<dyn Error>> {
        // First try to get a list of devices
        let devices = Self::list_devices(DeviceType::Any)?;

        if devices.is_empty() {
            return Err(Box::new(AudioError::NoDevicesFound));
        }

        // Try microphones first
        let microphones: Vec<_> = devices.iter().filter(|d| !d.is_monitor).collect();

        if !microphones.is_empty() {
            for mic in microphones {
                info!("Trying microphone device: {}", mic.name);
                match Self::with_device(app_name, &mic.name) {
                    Ok(capturer) => {
                        info!("Successfully connected to microphone: {}", mic.name);
                        return Ok(capturer);
                    }
                    Err(e) => {
                        warn!("Failed to connect to microphone {}: {}", mic.name, e);
                        // Continue to next device
                    }
                }
            }
        }

        // Then try monitor devices
        let monitors: Vec<_> = devices.iter().filter(|d| d.is_monitor).collect();

        if !monitors.is_empty() {
            for monitor in monitors {
                info!("Trying monitor device: {}", monitor.name);
                match Self::with_device(app_name, &monitor.name) {
                    Ok(capturer) => {
                        info!("Successfully connected to monitor: {}", monitor.name);
                        return Ok(capturer);
                    }
                    Err(e) => {
                        warn!("Failed to connect to monitor {}: {}", monitor.name, e);
                        // Continue to next device
                    }
                }
            }
        }

        // Finally try default device
        info!("Trying default audio input device");
        match Self::new(app_name) {
            Ok(capturer) => {
                info!("Successfully connected to default device");
                Ok(capturer)
            }
            Err(e) => {
                error!("Failed to connect to any audio device");
                Err(e)
            }
        }
    }

    /// List available audio input devices
    pub fn list_devices(device_type: DeviceType) -> Result<Vec<AudioDevice>, Box<dyn Error>> {
        let devices = Arc::new(Mutex::new(Vec::new()));
        let devices_clone = devices.clone();

        let mut proplist = Proplist::new().unwrap();
        proplist
            .set_str(
                libpulse_binding::proplist::properties::APPLICATION_NAME,
                "rholive-device-lister",
            )
            .map_err(|e| {
                AudioError::PulseContextError(format!("Failed to set proplist: {:?}", e))
            })?;

        let mut mainloop = Mainloop::new().ok_or_else(|| {
            AudioError::PulseContextError("Failed to create mainloop".to_string())
        })?;

        let mut context = Context::new_with_proplist(&mainloop, "rholive-context", &proplist)
            .ok_or_else(|| AudioError::PulseContextError("Failed to create context".to_string()))?;

        context.connect(None, ContextFlagSet::NOFLAGS, None)?;

        // Wait for context to be ready
        loop {
            match mainloop.iterate(false) {
                IterateResult::Quit(_) | IterateResult::Err(_) => {
                    return Err(Box::new(AudioError::PulseContextError(
                        "Mainloop iterate failed".to_string(),
                    )));
                }
                IterateResult::Success(_) => {}
            }

            match context.get_state() {
                libpulse_binding::context::State::Ready => {
                    break;
                }
                libpulse_binding::context::State::Failed
                | libpulse_binding::context::State::Terminated => {
                    return Err(Box::new(AudioError::ConnectionError(
                        "Connection failed".to_string(),
                    )));
                }
                _ => {} // Wait for Ready state
            }
        }

        // Create a flag to track operation completion
        let operation_done = Arc::new(Mutex::new(false));
        let operation_done_clone = operation_done.clone();

        // Get source information
        let introspector = context.introspect();
        let _op = introspector.get_source_info_list(move |source_info_list| {
            match source_info_list {
                ListResult::Item(source_info) => {
                    // Filter based on device type
                    let is_monitor = source_info.monitor_of_sink.is_some()
                        || source_info
                            .name
                            .as_ref()
                            .map(|name| name.contains("monitor"))
                            .unwrap_or(false);

                    let should_include = match device_type {
                        DeviceType::Microphone => !is_monitor,
                        DeviceType::Monitor => is_monitor,
                        DeviceType::Any => true,
                    };

                    if should_include {
                        if let (Some(name), Some(description)) = (
                            source_info.name.as_ref().map(|s| s.to_string()),
                            source_info.description.as_ref().map(|s| s.to_string()),
                        ) {
                            if let Ok(mut devices) = devices_clone.lock() {
                                devices.push(AudioDevice {
                                    name,
                                    description,
                                    sample_rate: source_info.sample_spec.rate,
                                    channels: source_info.sample_spec.channels,
                                    is_monitor,
                                });
                            }
                        }
                    }
                }
                ListResult::End => {
                    // Mark operation as complete
                    if let Ok(mut done) = operation_done_clone.lock() {
                        *done = true;
                    }
                }
                ListResult::Error => {
                    if let Ok(mut done) = operation_done_clone.lock() {
                        *done = true;
                    }
                    error!("Error listing audio devices");
                }
            }
        });

        // Wait for the operation to complete
        loop {
            match mainloop.iterate(false) {
                IterateResult::Quit(_) | IterateResult::Err(_) => {
                    return Err(Box::new(AudioError::OperationError(
                        "Mainloop iterate failed".to_string(),
                    )));
                }
                IterateResult::Success(_) => {}
            }

            // Check if we're done
            if let Ok(done) = operation_done.lock() {
                if *done {
                    break;
                }
            }
        }

        // Get the collected devices
        let result = if let Ok(devices) = devices.lock() {
            Ok(devices.clone())
        } else {
            Err(Box::new(AudioError::Other(
                "Failed to access devices list".to_string(),
            )))
        };

        // Clean up PulseAudio context and mainloop
        context.disconnect();
        mainloop.quit(Retval(0));

        Ok(result?)
    }
}
