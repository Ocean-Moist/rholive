use xcap::{Monitor, Capturer, Frame};
use std::error::Error;

/// Captures frames from the primary monitor using the `xcap` crate.
pub struct ScreenCapturer {
    capturer: Capturer,
}

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
