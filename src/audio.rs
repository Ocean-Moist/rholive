use libpulse_simple_binding::{Simple, Direction};
use libpulse_simple_binding::sample::{Spec, Format};
use std::error::Error;

/// Captures audio from the default system source using PulseAudio's
/// simple API. The audio is 16-bit little-endian PCM at 16 kHz.
pub struct AudioCapturer {
    simple: Simple,
}

impl AudioCapturer {
    /// Create a new `AudioCapturer` using the default device.
    pub fn new(app_name: &str) -> Result<Self, Box<dyn Error>> {
        let spec = Spec {
            format: Format::S16LE,
            channels: 1,
            rate: 16_000,
        };
        let simple = Simple::new(
            None,                      // default server
            app_name,                  // application name
            Direction::Record,
            None,                      // default device
            "record",                 // stream description
            &spec,
            None,                      // default channel map
            None,                      // default buffering
        )?;

        Ok(Self { simple })
    }

    /// Read a chunk of PCM data into the provided buffer.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<(), Box<dyn Error>> {
        self.simple.read(buffer)?;
        Ok(())
    }
}
