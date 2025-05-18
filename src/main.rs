mod gemini;
use gemini::*;
use tracing::info;

#[tokio::main]
async fn main() {
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
