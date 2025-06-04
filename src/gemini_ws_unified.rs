//! Unified Gemini WebSocket handler

use crate::media_event::{WsOutbound, WsInbound};
use crate::gemini_client::GeminiClient;
use crate::gemini::{ApiResponse, GeminiClientConfig};
use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info};

pub async fn run(
    api_key: &str,
    mut rx_out: UnboundedReceiver<WsOutbound>,
    tx_in: UnboundedSender<WsInbound>,
) -> Result<()> {
    let mut config = GeminiClientConfig::default();
    config.system_instruction = Some(
        "you are, rholive, a silent helper meant to assist the user in whatever task they choose. if you see a leetcode problem on the screen, solve it without waiting for them to say anything. if someone they are on call with asks you a question, answer it. you are effectively their second mind, they should not have to do any thinking, they should not have to ask you for anything. you are their brain, they should not have to think, respond to whatever is on screen or whatever someone says like the user would.
          you have access to the users screen, microphone and system audio.
            when there is no change or nothing to work, do, or comment on, respond only with '<nothing>' (without quotes). if you don't understand what is going on, respond only with '<nothing>'. please be quiet until the user asks you something or u know what to do (i.e. respond with '<nothing>').
            keep in mind, the user can only really see a few lines, so when you respond start with first thing wait and continually do more.
            "
        .to_string()
    );
    
    let mut client = GeminiClient::from_api_key(api_key, Some(config));
    
    client.connect().await?;
    client.setup().await?;
    
    let mut response_rx = client.subscribe();
    
    // Handle outgoing messages
    tokio::spawn(async move {
        while let Some(msg) = rx_out.recv().await {
            match msg {
                WsOutbound::Json(json) => {
                    // Log message type for debugging
                    if json.get("activityStart").is_some() {
                        info!(">>> Sending activityStart");
                    } else if json.get("activityEnd").is_some() {
                        info!(">>> Sending activityEnd");
                    } else if json.get("audio").is_some() {
                        debug!(">>> Sending audio chunk");
                    } else if json.get("video").is_some() {
                        debug!(">>> Sending video frame");
                    }
                    
                    if let Err(e) = client.send_realtime_input(json).await {
                        error!("Error sending to Gemini: {}", e);
                    }
                }
            }
        }
    });
    
    // Handle incoming responses
    while let Some(response) = response_rx.recv().await {
        match response {
            Ok(api_response) => {
                let ws_in = match api_response {
                    ApiResponse::TextResponse { text, is_complete } => {
                        if is_complete {
                            info!("<<< Complete response: {}", 
                                  text.chars().take(50).collect::<String>());
                        }
                        Some(WsInbound::Text { content: text, is_final: is_complete })
                    }
                    ApiResponse::GenerationComplete => {
                        info!("<<< Generation complete");
                        Some(WsInbound::GenerationComplete)
                    }
                    ApiResponse::ToolCall(tool_call) => {
                        Some(WsInbound::ToolCall { 
                            name: tool_call.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            args: tool_call
                        })
                    }
                    ApiResponse::ConnectionClosed => {
                        error!("Gemini connection closed");
                        break;
                    }
                    _ => None,
                };
                
                if let Some(event) = ws_in {
                    if tx_in.send(event).is_err() {
                        error!("Failed to send event - channel closed");
                        break;
                    }
                }
            }
            Err(e) => {
                error!("Gemini API error: {:?}", e);
                if tx_in.send(WsInbound::Error(format!("{:?}", e))).is_err() {
                    break;
                }
            }
        }
    }
    
    Ok(())
}