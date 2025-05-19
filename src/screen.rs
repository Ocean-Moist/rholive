use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::Receiver;
use std::time::Duration;
use tracing::{debug, info, warn};
use xcap::{Frame, Monitor, VideoRecorder};

/// Represents a captured screen frame with conversion options.
#[derive(Debug)]
pub struct CapturedFrame {
    /// The raw frame data from XCap
    pub frame: Frame,
    /// The JPEG encoded data, lazily computed
    jpeg_data: Option<Vec<u8>>,
}

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
            let rgba_image = image::RgbaImage::from_raw(width, height, self.frame.raw.clone())
                .ok_or_else(|| "Failed to create image from raw data".to_string())?;

            // First convert RGBA to RGB by dropping the alpha channel
            let rgb_image = image::DynamicImage::ImageRgba8(rgba_image).into_rgb8();

            // Encode as JPEG with reasonable quality
            let mut jpeg_buffer = Vec::new();
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buffer, 75);
            encoder.encode(&rgb_image, width, height, image::ExtendedColorType::Rgb8)?;

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
pub struct ScreenCapturer {
    video_recorder: VideoRecorder,
    frame_rx: Receiver<Frame>,
    capture_interval: Duration,
    last_capture: std::time::Instant,
    monitor_info: MonitorInfo,
    // Frame deduplication tracking
    last_frame_hash: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    name: String,
    width: u32,
    height: u32,
    is_primary: bool,
}

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
        let monitor = monitors
            .iter()
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

        info!(
            "Using monitor: {} ({}x{}, primary: {})",
            monitor_info.name, monitor_info.width, monitor_info.height, monitor_info.is_primary
        );

        let (video_recorder, frame_rx) = monitor.video_recorder()?;
        video_recorder.start()?;

        Ok(Self {
            video_recorder,
            frame_rx,
            capture_interval,
            last_capture: std::time::Instant::now(),
            monitor_info,
            last_frame_hash: None,
        })
    }

    /// Calculate a hash for a frame to use for deduplication
    fn calculate_frame_hash(frame: &Frame) -> u64 {
        let mut hasher = DefaultHasher::new();

        // Create a smaller sampling of the frame for faster hashing
        // Sample every 20th pixel to get a representative hash
        if !frame.raw.is_empty() {
            let stride = 20 * 4; // Every 20th RGBA pixel
            for i in (0..frame.raw.len()).step_by(stride) {
                if i < frame.raw.len() {
                    frame.raw[i].hash(&mut hasher);
                }
            }
        }

        // Also hash the dimensions
        frame.width.hash(&mut hasher);
        frame.height.hash(&mut hasher);

        hasher.finish()
    }

    /// Capture a single frame of the screen.
    /// This method respects the configured capture interval.
    pub fn capture_frame(&mut self) -> Result<CapturedFrame, Box<dyn Error>> {
        let now = std::time::Instant::now();

        // Check if we need to throttle frame captures
        if now.duration_since(self.last_capture) < self.capture_interval {
            debug!("Capture interval not reached, throttling capture");
            return Err("Capture interval not reached".into());
        }

        // Try to receive a frame with timeout
        match self.frame_rx.recv_timeout(Duration::from_millis(800)) {
            // Increased timeout
            Ok(frame) => {
                tracing::debug!("Captured frame: {}x{}", frame.width, frame.height);

                // Calculate hash for deduplication
                let frame_hash = Self::calculate_frame_hash(&frame);

                // Check if it's a duplicate
                if let Some(last_hash) = self.last_frame_hash {
                    if frame_hash == last_hash {
                        warn!("Duplicate frame detected, skipping");
                        return Err("Duplicate frame".into());
                    }
                }

                // Update state
                self.last_capture = now;
                self.last_frame_hash = Some(frame_hash);

                Ok(CapturedFrame::new(frame))
            }
            Err(e) => {
                // Log the error but don't propagate timeout errors as they're expected
                if let std::sync::mpsc::RecvTimeoutError::Timeout = e {
                    tracing::debug!("Timed out waiting for screen frame, this is normal");
                    Err("Frame capture timeout".into())
                } else {
                    tracing::error!("Error receiving frame from xcap: {:?}", e);
                    Err(Box::new(e))
                }
            }
        }
    }

    /// Force a frame capture regardless of interval
    pub fn force_capture_frame(&mut self) -> Result<CapturedFrame, Box<dyn Error>> {
        // Reset the last capture time
        self.last_capture =
            std::time::Instant::now() - self.capture_interval - Duration::from_millis(1);

        // For forced captures, we'll still capture even if it's a duplicate
        match self.frame_rx.recv_timeout(Duration::from_millis(800)) {
            Ok(frame) => {
                tracing::debug!("Forced capture of frame: {}x{}", frame.width, frame.height);

                // Calculate hash for future comparison
                let frame_hash = Self::calculate_frame_hash(&frame);
                self.last_frame_hash = Some(frame_hash);

                // Update state
                self.last_capture = std::time::Instant::now();

                Ok(CapturedFrame::new(frame))
            }
            Err(e) => {
                // Log the error but don't propagate timeout errors as they're expected
                if let std::sync::mpsc::RecvTimeoutError::Timeout = e {
                    tracing::debug!("Timed out waiting for forced screen frame");
                    Err("Frame capture timeout".into())
                } else {
                    tracing::error!("Error receiving forced frame from xcap: {:?}", e);
                    Err(Box::new(e))
                }
            }
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
