#[cfg(feature = "capture")]
use xcap::{Monitor, VideoRecorder, Frame};
#[cfg(feature = "capture")]
use std::error::Error;
#[cfg(feature = "capture")]
use std::sync::mpsc::Receiver;
#[cfg(feature = "capture")]
use std::time::Duration;
#[cfg(feature = "capture")]
use tracing::{info, warn, error};

/// Represents a captured screen frame with conversion options.
#[cfg(feature = "capture")]
#[derive(Debug)]
pub struct CapturedFrame {
    /// The raw frame data from XCap
    pub frame: Frame,
    /// The JPEG encoded data, lazily computed
    jpeg_data: Option<Vec<u8>>,
}

#[cfg(feature = "capture")]
impl CapturedFrame {
    /// Create a new CapturedFrame from an XCap Frame
    pub fn new(frame: Frame) -> Self {
        Self {
            frame,
            jpeg_data: None,
        }
    }
    
    /// Convert the frame to JPEG format for sending to the Gemini API
    pub fn to_jpeg(&mut self) -> Result<&[u8], Box<dyn Error>> {
        if self.jpeg_data.is_none() {
            // Convert the raw RGBA buffer to JPEG
            let width = self.frame.width;
            let height = self.frame.height;
            
            // Create an RgbaImage from the raw data
            let image = image::RgbaImage::from_raw(width, height, self.frame.raw.clone())
                .ok_or_else(|| "Failed to create image from raw data".to_string())?;
            
            // Encode as JPEG with reasonable quality
            let mut jpeg_buffer = Vec::new();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buffer, 75);
            encoder.encode(
                &image.as_raw(),
                width,
                height,
                image::ColorType::Rgb8
            )?;
            
            self.jpeg_data = Some(jpeg_buffer);
        }
        
        Ok(self.jpeg_data.as_ref().unwrap())
    }
    
    /// Returns the MIME type for the encoded image format
    pub fn mime_type(&self) -> &'static str {
        "image/jpeg"
    }
    
    /// Get the width of the frame
    pub fn width(&self) -> u32 {
        self.frame.width
    }
    
    /// Get the height of the frame
    pub fn height(&self) -> u32 {
        self.frame.height
    }
}

/// Captures frames from the primary monitor using the `xcap` crate.
#[cfg(feature = "capture")]
pub struct ScreenCapturer {
    video_recorder: VideoRecorder,
    frame_rx: Receiver<Frame>,
    capture_interval: Duration,
    last_capture: std::time::Instant,
    monitor_info: MonitorInfo,
}

#[cfg(feature = "capture")]
#[derive(Debug, Clone)]
struct MonitorInfo {
    name: String,
    width: u32,
    height: u32,
    is_primary: bool,
}

#[cfg(feature = "capture")]
impl ScreenCapturer {
    /// Create a new screen capturer for the primary monitor with default options.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Self::with_options(Duration::from_millis(500))
    }
    
    /// Create a new screen capturer for the primary monitor with specified capture interval.
    pub fn with_options(capture_interval: Duration) -> Result<Self, Box<dyn Error>> {
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
        
        // Store monitor information
        let monitor_info = MonitorInfo {
            name: monitor.name().unwrap_or_else(|_| "Unknown".to_string()),
            width: monitor.width().unwrap_or(0),
            height: monitor.height().unwrap_or(0),
            is_primary: monitor.is_primary().unwrap_or(false),
        };
        
        info!("Using monitor: {} ({}x{}, primary: {})", 
            monitor_info.name, monitor_info.width, monitor_info.height, monitor_info.is_primary);
            
        let (video_recorder, frame_rx) = monitor.video_recorder()?;
        video_recorder.start()?;
        
        Ok(Self { 
            video_recorder, 
            frame_rx, 
            capture_interval,
            last_capture: std::time::Instant::now(),
            monitor_info,
        })
    }

    /// Capture a single frame of the screen.
    /// This method respects the configured capture interval.
    pub fn capture_frame(&mut self) -> Result<CapturedFrame, Box<dyn Error>> {
        let now = std::time::Instant::now();
        
        // Check if we need to throttle frame captures
        if now.duration_since(self.last_capture) < self.capture_interval {
            return Err("Capture interval not reached".into());
        }
        
        // Try to receive a frame with timeout
        match self.frame_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(frame) => {
                self.last_capture = now;
                Ok(CapturedFrame::new(frame))
            },
            Err(e) => Err(Box::new(e)),
        }
    }
    
    /// Configure the capture interval (minimum time between frames)
    pub fn set_capture_interval(&mut self, interval: Duration) {
        self.capture_interval = interval;
    }
    
    /// Get information about the monitor being captured
    pub fn monitor_info(&self) -> &MonitorInfo {
        &self.monitor_info
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
