mod audio;
mod screen;

use audio::AudioCapturer;
use screen::ScreenCapturer;

mod gemini;
use gemini::*;
use tracing::info;

#[tokio::main]
async fn main() {
    // Initialize audio and screen capture to demonstrate bindings are callable.
    let mut audio = AudioCapturer::new("rholive").expect("audio init");
    let mut screen = ScreenCapturer::new().expect("screen init");

    // Read a small chunk of audio and capture one frame.
    let mut buffer = [0u8; 3200]; // ~100ms of 16 kHz mono S16LE
    audio.read(&mut buffer).expect("audio read");

    let _frame = screen.capture_frame().expect("screen capture");

    println!("Captured audio chunk and screen frame");
  
    tracing_subscriber::fmt::init();
    info!("starting gemini client example");

    let url = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService/BidiGenerateContent?key=YOUR_API_KEY";

    match GeminiClient::connect(url).await {
        Ok(mut client) => {
            // Example: send setup then wait for response
            let setup = BidiGenerateContentSetup {
                model: "models/gemini-2.0-flash-live-001".to_string(),
                ..Default::default()
            };
            let msg = ClientMessage::Setup { setup };
            let _ = client.send(&msg).await;
            if let Some(Ok(ServerMessage::SetupComplete { .. })) = client.next().await {
                info!("setup complete");
            }
        }
        Err(e) => {
            eprintln!("failed to connect: {e}");
        }
    }
}
