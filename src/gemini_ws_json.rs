//! Gemini WebSocket handler that accepts JSON messages directly

use crate::events::WsIn;
use crate::gemini_client::GeminiClient;
use crate::gemini::ApiResponse;
use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{info, error};

pub async fn run(
    api_key: &str,
    mut rx_json: UnboundedReceiver<serde_json::Value>,
    tx_evt: UnboundedSender<WsIn>,
) -> Result<()> {
    use crate::gemini::GeminiClientConfig;
    
    let mut config = GeminiClientConfig::default();
    config.system_instruction = Some(
        "\
            describe what you see on the screen, if it hasn't changed, respond with '<nothing>' (no quotes). ignore audio. do not repeat yourself.
        \
        ".to_string()
    );
    
    let mut client = GeminiClient::from_api_key(api_key, Some(config));
    
    client.connect().await?;
    client.setup().await?;
    
    let mut response_rx = client.subscribe();
    
    // Handle outgoing JSON messages
    tokio::spawn(async move {
        while let Some(json) = rx_json.recv().await {
            info!("📨 Sending JSON to Gemini: {}", 
                  serde_json::to_string(&json).unwrap_or_default().chars().take(100).collect::<String>());
            if let Err(e) = client.send_realtime_input(json).await {
                error!("❌ Error sending to Gemini: {}", e);
            }
        }
    });
    
    // Handle incoming responses
    while let Some(response) = response_rx.recv().await {
        match response {
            Ok(api_response) => {
                let ws_in = match api_response {
                    ApiResponse::TextResponse { text, is_complete } => {
                        WsIn::Text { content: text, is_final: is_complete }
                    }
                    ApiResponse::GenerationComplete => {
                        WsIn::GenerationComplete
                    }
                    ApiResponse::ToolCall(tool_call) => {
                        WsIn::ToolCall { 
                            name: tool_call.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            args: tool_call
                        }
                    }
                    ApiResponse::ConnectionClosed => {
                        error!("Gemini connection closed");
                        break;
                    }
                    _ => continue,
                };
                
                if tx_evt.send(ws_in).is_err() {
                    error!("Failed to send event - channel closed");
                    break;
                }
            }
            Err(e) => {
                error!("Gemini API error: {:?}", e);
                if tx_evt.send(WsIn::Error(format!("{:?}", e))).is_err() {
                    break;
                }
            }
        }
    }
    
    Ok(())
}