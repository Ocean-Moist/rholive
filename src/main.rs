mod audio;
#[cfg(feature = "capture")]
mod screen;

use audio::AudioCapturer;
#[cfg(feature = "capture")]
use screen::ScreenCapturer;

mod gemini;
use gemini::*;
use tracing::info;

#[tokio::main]
async fn main() {
    // Initialize audio capture and optionally screen capture.
    let mut audio = AudioCapturer::new("rholive").expect("audio init");
    #[cfg(feature = "capture")]
    let mut screen = ScreenCapturer::new().expect("screen init");

    // Read a small chunk of audio and capture one frame.
    let mut buffer = [0u8; 3200]; // ~100ms of 16 kHz mono S16LE
    audio.read(&mut buffer).expect("audio read");

    #[cfg(feature = "capture")]
    let _frame = screen.capture_frame().expect("screen capture");

    #[cfg(feature = "capture")]
    println!("Captured audio chunk and screen frame");
    #[cfg(not(feature = "capture"))]
    println!("Captured audio chunk");
  
    tracing_subscriber::fmt::init();
    info!("starting gemini client example");

    // Get API key from environment variable
    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY environment variable not set");
    
    // Use the Gemini Live API endpoint with your API key
    let url = format!("wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key={}", api_key);

    match GeminiClient::connect(&*url).await {
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
