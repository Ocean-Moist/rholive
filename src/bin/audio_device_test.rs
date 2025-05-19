use rholive::audio::{AudioCapturer, DeviceType};
use std::error::Error;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // List all available audio input devices
    println!("Available audio input devices:");

    // Try to list devices
    match AudioCapturer::list_devices(DeviceType::Any) {
        Ok(devices) => {
            if devices.is_empty() {
                println!("No audio devices found!");
            } else {
                for (i, device) in devices.iter().enumerate() {
                    println!(
                        "{}: {} ({})",
                        i + 1,
                        device.description,
                        if device.is_monitor {
                            "Monitor"
                        } else {
                            "Microphone"
                        }
                    );
                    println!("   Name: {}", device.name);
                    println!(
                        "   Rate: {} Hz, Channels: {}",
                        device.sample_rate, device.channels
                    );
                    println!();
                }
            }
        }
        Err(e) => {
            println!("Error listing devices: {}", e);
        }
    }

    // Try automatic device fallback
    println!("\nTesting automatic device fallback...");
    match AudioCapturer::with_fallback("rholive-test") {
        Ok(capturer) => {
            println!(
                "Successfully connected to device: {:?}",
                capturer.device_name()
            );
            // Sleep briefly to keep connection alive
            std::thread::sleep(Duration::from_secs(1));
        }
        Err(e) => {
            println!("Failed to connect to any audio device: {}", e);
        }
    }

    // Try to connect to each available input device directly
    println!("\nTesting direct device connections...");
    if let Ok(devices) = AudioCapturer::list_devices(DeviceType::Any) {
        for device in devices {
            println!(
                "Trying to connect to: {} ({})",
                device.description,
                if device.is_monitor {
                    "Monitor"
                } else {
                    "Microphone"
                }
            );

            match AudioCapturer::with_device("rholive-test", &device.name) {
                Ok(_) => {
                    println!("  ✅ Success");
                    // Sleep briefly to keep connection alive
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(e) => {
                    println!("  ❌ Failed: {}", e);
                }
            }
        }
    }

    println!("\nAudio device test complete!");
    Ok(())
}
