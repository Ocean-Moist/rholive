#[cfg(feature = "capture")]
use xcap::{Monitor, Capturer, Frame};
#[cfg(feature = "capture")]
use std::error::Error;

/// Captures frames from the primary monitor using the `xcap` crate.
#[cfg(feature = "capture")]
pub struct ScreenCapturer {
    capturer: Capturer,
}

#[cfg(feature = "capture")]
impl ScreenCapturer {
    /// Create a new screen capturer for the primary monitor.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let monitor = Monitor::primary()?;
        let capturer = Capturer::new(monitor)?;
        Ok(Self { capturer })
    }

    /// Capture a single frame of the screen.
    pub fn capture_frame(&mut self) -> Result<Frame, Box<dyn Error>> {
        let frame = self.capturer.capture_frame()?;
        Ok(frame)
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
