#[cfg(feature = "capture")]
use xcap::{Monitor, VideoRecorder, Frame};
#[cfg(feature = "capture")]
use std::error::Error;
#[cfg(feature = "capture")]
use std::sync::mpsc::Receiver;

/// Captures frames from the primary monitor using the `xcap` crate.
#[cfg(feature = "capture")]
pub struct ScreenCapturer {
    video_recorder: VideoRecorder,
    frame_rx: Receiver<Frame>,
}

#[cfg(feature = "capture")]
impl ScreenCapturer {
    /// Create a new screen capturer for the primary monitor.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        // Get all monitors and use the first one
        let monitors = Monitor::all()?;
        if monitors.is_empty() {
            return Err("No monitors found".into());
        }
        
        // Find primary monitor if available
        let monitor = monitors.iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .unwrap_or(&monitors[0])
            .clone();
            
        let (video_recorder, frame_rx) = monitor.video_recorder()?;
        video_recorder.start()?;
        
        Ok(Self { video_recorder, frame_rx })
    }

    /// Capture a single frame of the screen.
    pub fn capture_frame(&mut self) -> Result<Frame, Box<dyn Error>> {
        match self.frame_rx.recv() {
            Ok(frame) => Ok(frame),
            Err(e) => Err(Box::new(e)),
        }
    }
}

#[cfg(not(feature = "capture"))]
pub struct ScreenCapturer;

#[cfg(not(feature = "capture"))]
impl ScreenCapturer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Err("screen capture feature not enabled".into())
    }

    pub fn capture_frame(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Err("screen capture feature not enabled".into())
    }
}
