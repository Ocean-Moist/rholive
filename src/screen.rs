use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::Receiver;
use std::time::Duration;
use tracing::{debug, info};
use xcap::{Frame, Monitor, VideoRecorder};

/// Screen capture error that is Send + Sync
#[derive(Debug)]
pub enum ScreenError {
    XcapError(String),
    NoMonitors,
    FrameConversionError(String),
    Other(String),
}

impl fmt::Display for ScreenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScreenError::XcapError(e) => write!(f, "Xcap error: {}", e),
            ScreenError::NoMonitors => write!(f, "No monitors found"),
            ScreenError::FrameConversionError(e) => write!(f, "Frame conversion error: {}", e),
            ScreenError::Other(e) => write!(f, "Screen capture error: {}", e),
        }
    }
}

impl Error for ScreenError {}

// Make it Send + Sync
unsafe impl Send for ScreenError {}
unsafe impl Sync for ScreenError {}

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
    pub fn to_jpeg(&mut self) -> Result<&[u8], ScreenError> {
        use tracing::{debug, info};
        
        if self.jpeg_data.is_none() {
            // Convert the raw RGBA buffer to JPEG using turbojpeg
            let width = self.frame.width;
            let height = self.frame.height;
            
            debug!("🔄 Converting {}x{} RGBA frame to JPEG using turbojpeg...", width, height);

            let start = std::time::Instant::now();
            
            // Use turbojpeg for fast JPEG encoding
            let jpeg_buffer = to_jpeg_fast(&self.frame.raw, width, height, 75)
                .map_err(|e| ScreenError::FrameConversionError(format!("TurboJPEG error: {}", e)))?;

            let encoding_time = start.elapsed();
            let jpeg_size_kb = jpeg_buffer.len() / 1024;
            info!("✅ TurboJPEG encoding complete: {} KB in {:.1}ms", jpeg_size_kb, encoding_time.as_secs_f64() * 1000.0);
            self.jpeg_data = Some(jpeg_buffer);
        } else {
            debug!("🔄 Using cached JPEG data");
        }

        let jpeg_data = self.jpeg_data.as_ref().unwrap();
        debug!("📤 Returning JPEG data: {} bytes", jpeg_data.len());
        Ok(jpeg_data)
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
    
    /// Calculate a hash of the frame for duplicate detection
    pub fn hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        
        // Hash a subset of pixels for efficiency
        let step = (self.frame.raw.len() / 1000).max(1);
        for i in (0..self.frame.raw.len()).step_by(step) {
            self.frame.raw[i].hash(&mut hasher);
        }
        
        // Include dimensions in hash
        self.frame.width.hash(&mut hasher);
        self.frame.height.hash(&mut hasher);
        
        hasher.finish()
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
    pub fn new() -> Result<Self, ScreenError> {
        Self::with_options(Duration::from_millis(500))
    }

    /// Create a new screen capturer for the primary monitor with specified capture interval.
    pub fn with_options(capture_interval: Duration) -> Result<Self, ScreenError> {
        // Get all monitors and use the first one
        let monitors = Monitor::all()
            .map_err(|e| ScreenError::XcapError(e.to_string()))?;
        if monitors.is_empty() {
            return Err(ScreenError::NoMonitors);
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

        let (video_recorder, frame_rx) = monitor.video_recorder()
            .map_err(|e| ScreenError::XcapError(e.to_string()))?;
        video_recorder.start()
            .map_err(|e| ScreenError::XcapError(e.to_string()))?;

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
    pub fn capture_frame(&mut self) -> Result<CapturedFrame, ScreenError> {
        use tracing::{debug, info};
        let now = std::time::Instant::now();

        // Check if we need to throttle frame captures
        if now.duration_since(self.last_capture) < self.capture_interval {
            debug!("🚫 Frame capture throttled: interval not reached");
            return Err(ScreenError::Other("Capture interval not reached".to_string()));
        }
        
        debug!("📸 Starting screen capture...");

        // Try to receive a frame with timeout
        match self.frame_rx.recv_timeout(Duration::from_millis(800)) {
            // Increased timeout
            Ok(mut frame) => {
                // Drain the channel to get the newest frame
                while let Ok(f) = self.frame_rx.try_recv() {
                    frame = f;
                }
                info!("📸 Captured raw frame: {}x{} pixels", frame.width, frame.height);

                // Calculate hash for deduplication
                debug!("🔢 Calculating frame hash for deduplication...");
                let frame_hash = Self::calculate_frame_hash(&frame);

                // Check if it's a duplicate
                if let Some(last_hash) = self.last_frame_hash {
                    if frame_hash == last_hash {
                        debug!("🔄 Duplicate frame detected (hash: {}), skipping", frame_hash);
                        return Err(ScreenError::Other("Duplicate frame".to_string()));
                    } else {
                        debug!("✅ New unique frame detected (hash: {} -> {})", last_hash, frame_hash);
                    }
                } else {
                    debug!("✅ First frame captured (hash: {})", frame_hash);
                }

                // Update state
                self.last_capture = now;
                self.last_frame_hash = Some(frame_hash);

                info!("✅ Screen capture successful, creating CapturedFrame");
                Ok(CapturedFrame::new(frame))
            }
            Err(e) => {
                // Log the error but don't propagate timeout errors as they're expected
                if let std::sync::mpsc::RecvTimeoutError::Timeout = e {
                    debug!("Timed out waiting for screen frame, this is normal");
                    Err(ScreenError::Other("Frame capture timeout".to_string()))
                } else {
                    tracing::error!("Error receiving frame from xcap: {:?}", e);
                    Err(ScreenError::Other(format!("Receive error: {:?}", e)))
                }
            }
        }
    }

    /// Force a frame capture regardless of interval
    pub fn force_capture_frame(&mut self) -> Result<CapturedFrame, ScreenError> {
        // Reset the last capture time
        self.last_capture =
            std::time::Instant::now() - self.capture_interval - Duration::from_millis(1);

        // For forced captures, we'll still capture even if it's a duplicate
        match self.frame_rx.recv_timeout(Duration::from_millis(800)) {
            Ok(mut frame) => {
                // Drain the channel to get the newest frame
                while let Ok(f) = self.frame_rx.try_recv() {
                    frame = f;
                }
                debug!("Forced capture of frame: {}x{}", frame.width, frame.height);

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
                    debug!("Timed out waiting for forced screen frame");
                    Err(ScreenError::Other("Frame capture timeout".to_string()))
                } else {
                    tracing::error!("Error receiving forced frame from xcap: {:?}", e);
                    Err(ScreenError::Other(format!("Receive error: {:?}", e)))
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

/// Public function to calculate a quick hash for a frame
pub fn quick_hash(frame: &Frame) -> u64 {
    let mut hasher = DefaultHasher::new();

    // Sample the frame data for faster hashing
    let data = &frame.raw;
    let step = data.len() / 64; // Sample 64 points

    for i in (0..data.len()).step_by(step.max(1)) {
        data[i].hash(&mut hasher);
    }

    // Also hash the dimensions
    frame.width.hash(&mut hasher);
    frame.height.hash(&mut hasher);

    hasher.finish()
}

/// Fast JPEG encoding using libjpeg-turbo
pub fn to_jpeg_fast(rgba: &[u8], width: u32, height: u32, quality: i32) -> turbojpeg::Result<Vec<u8>> {
    use turbojpeg::{compress, Image, PixelFormat, Subsamp};

    // libjpeg-turbo can accept 4-channel input; we just tell it RGBA
    let img = Image {
        pixels: rgba,
        width: width as usize,
        pitch: (width * 4) as usize, // bytes per scanline
        height: height as usize,
        format: PixelFormat::RGBA,
    };

    // Use 4:2:0 subsampling for good compression/quality balance
    let compressed = compress(img, quality, Subsamp::Sub2x2)?;
    Ok(compressed.as_ref().to_vec())
}
