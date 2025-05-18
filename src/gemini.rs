// Gemini Live API client wrapper
// Based on IMPLEMENTATION_PLAN.md and GEMINI_LIVE_API.md
// Provides minimal structures and async WebSocket client for interacting with the API.

use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_tungstenite::tungstenite::Error as WsError;
use futures_util::{StreamExt, SinkExt};

/// Generation configuration for setup.
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_modalities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech_config: Option<serde_json::Value>,
}

/// Session setup message.
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BidiGenerateContentSetup {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realtime_input_config: Option<serde_json::Value>,
}

/// A chunk of realtime input (audio/video/text)
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<RealtimeAudio>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<RealtimeVideo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_end: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_end: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RealtimeAudio {
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RealtimeVideo {
    pub data: String,
    pub mime_type: String,
}

/// Message sent from client to server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientMessage {
    Setup { setup: BidiGenerateContentSetup },
    ClientContent { client_content: serde_json::Value },
    RealtimeInput { realtime_input: RealtimeInput },
    ToolResponse { tool_response: serde_json::Value },
}

/// Server -> client messages
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerMessage {
    SetupComplete { setup_complete: serde_json::Value },
    ServerContent { server_content: serde_json::Value },
    ToolCall { tool_call: serde_json::Value },
    ToolCallCancellation { tool_call_cancellation: serde_json::Value },
    GoAway { go_away: serde_json::Value },
    SessionResumptionUpdate { session_resumption_update: serde_json::Value },
}

/// Async Gemini Live API client.
pub struct GeminiClient {
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, 
}

impl GeminiClient {
    /// Connect to the Live API endpoint using the given url (should include api key query param).
    pub async fn connect(url: &str) -> Result<Self, WsError> {
        let (ws, _resp) = connect_async(url).await?;
        Ok(GeminiClient { ws })
    }

    /// Send a client message to the server.
    pub async fn send(&mut self, msg: &ClientMessage) -> Result<(), WsError> {
        let text = serde_json::to_string(msg).map_err(|e| WsError::Protocol(e.to_string()))?;
        self.ws.send(Message::Text(text)).await
    }

    /// Receive the next server message.
    pub async fn next(&mut self) -> Option<Result<ServerMessage, WsError>> {
        match self.ws.next().await? {
            Ok(Message::Text(text)) => {
                let parsed = serde_json::from_str::<ServerMessage>(&text)
                    .map_err(|e| WsError::Protocol(e.to_string()));
                Some(parsed)
            }
            Ok(Message::Close(_)) => None,
            Ok(_) => self.next().await,
            Err(e) => Some(Err(e)),
        }
    }
}

