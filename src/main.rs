mod audio;
mod screen;

use audio::AudioCapturer;
use screen::ScreenCapturer;

fn main() {
    // Initialize audio and screen capture to demonstrate bindings are callable.
    let mut audio = AudioCapturer::new("rholive").expect("audio init");
    let mut screen = ScreenCapturer::new().expect("screen init");

    // Read a small chunk of audio and capture one frame.
    let mut buffer = [0u8; 3200]; // ~100ms of 16 kHz mono S16LE
    audio.read(&mut buffer).expect("audio read");

    let _frame = screen.capture_frame().expect("screen capture");

    println!("Captured audio chunk and screen frame");
}
