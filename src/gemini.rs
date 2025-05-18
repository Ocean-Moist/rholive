// Gemini Live API client wrapper
// Based on IMPLEMENTATION_PLAN.md and GEMINI_LIVE_API.md
// Provides minimal structures and async WebSocket client for interacting with the API.

use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_tungstenite::tungstenite::Error as WsError;
use std::io;
use futures_util::{StreamExt, SinkExt};

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;
    use std::time::Duration;

    #[test]
    fn test_generation_config_serialization() {
        let config = GenerationConfig {
            response_modalities: vec!["TEXT".to_string()],
            temperature: Some(0.7),
            media_resolution: Some("MEDIA_RESOLUTION_MEDIUM".to_string()),
            speech_config: None,
        };
        
        let json = serde_json::to_string(&config).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        
        assert_eq!(parsed["responseModalities"][0], "TEXT");
        assert_eq!(parsed["temperature"], 0.7);
        assert_eq!(parsed["mediaResolution"], "MEDIA_RESOLUTION_MEDIUM");
        assert!(parsed.get("speechConfig").is_none());
    }
    
    #[test]
    fn test_client_message_serialization() {
        // Test setup message
        let setup = BidiGenerateContentSetup {
            model: "models/gemini-2.0-flash-live-001".to_string(),
            generation_config: Some(GenerationConfig {
                response_modalities: vec!["TEXT".to_string()],
                temperature: Some(0.7),
                media_resolution: None,
                speech_config: None,
            }),
            system_instruction: Some("You are a helpful assistant.".to_string()),
            tools: None,
            realtime_input_config: None,
        };
        
        let msg = ClientMessage::Setup { setup: setup.clone() };
        
        // Test direct serialization of the JSON we send to the server
        let json = match &msg {
            ClientMessage::Setup { setup } => {
                let setup_json = serde_json::to_string(setup).unwrap();
                let inner = &setup_json[1..setup_json.len()-1];
                format!("{{\"setup\":{{{}}}}}", inner)
            },
            _ => panic!("Unexpected message type"),
        };
        
        println!("JSON output: {}", json);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        
        // With the manual serialization, we expect {"setup": {...}}
        assert!(parsed.get("setup").is_some());
        assert_eq!(parsed["setup"]["model"], "models/gemini-2.0-flash-live-001");
        assert_eq!(parsed["setup"]["systemInstruction"], "You are a helpful assistant.");
        
        // Test realtime input message
        let audio_input = RealtimeInput {
            audio: Some(RealtimeAudio {
                data: "base64data".to_string(),
                mime_type: "audio/pcm;rate=16000".to_string(),
            }),
            video: None,
            text: None,
            activity_start: Some(true),
            activity_end: None,
            audio_stream_end: None,
        };
        
        let msg = ClientMessage::RealtimeInput { realtime_input: audio_input.clone() };
        
        // Test direct serialization of the JSON we send to the server
        let json = format!("{{\"realtimeInput\":{}}}", serde_json::to_string(&audio_input).unwrap());
        
        println!("Audio JSON output: {}", json);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        
        // With the manual serialization, we expect {"realtimeInput": {...}}
        assert!(parsed.get("realtimeInput").is_some());
        
        // Checking the realtimeInput field structure
        assert!(parsed["realtimeInput"].get("audio").is_some());
        assert_eq!(parsed["realtimeInput"]["audio"]["data"], "base64data");
        assert_eq!(parsed["realtimeInput"]["audio"]["mime_type"], "audio/pcm;rate=16000");
        assert_eq!(parsed["realtimeInput"]["activityStart"], true);
    }
    
    #[test]
    fn test_server_message_deserialization() {
        // Create and serialize a ServerMessage manually to ensure format matches
        let complete_value = serde_json::json!({"handle": "session123"});
        let setup_message = ServerMessage::SetupComplete { setup_complete: complete_value };
        let setup_json = serde_json::to_string(&setup_message).unwrap();
        
        // Deserialize back to check
        let parsed: ServerMessage = serde_json::from_str(&setup_json).unwrap();
        if let ServerMessage::SetupComplete { setup_complete } = parsed {
            assert_eq!(setup_complete["handle"], "session123");
        } else {
            panic!("Expected SetupComplete message");
        }
        
        // Test server content message
        let model_turn = serde_json::json!({
            "modelTurn": {
                "parts": [{"text": "Hello, how can I help?"}]
            },
            "generationComplete": true,
            "turnComplete": true
        });
        
        let content_message = ServerMessage::ServerContent { server_content: model_turn };
        let content_json = serde_json::to_string(&content_message).unwrap();
        
        // Deserialize back
        let parsed: ServerMessage = serde_json::from_str(&content_json).unwrap();
        if let ServerMessage::ServerContent { server_content } = parsed {
            let parts = &server_content["modelTurn"]["parts"];
            assert_eq!(parts[0]["text"], "Hello, how can I help?");
            assert_eq!(server_content["generationComplete"], true);
            assert_eq!(server_content["turnComplete"], true);
        } else {
            panic!("Expected ServerContent message");
        }
    }
    
    // To run this test, set the GEMINI_API_KEY environment variable
    #[tokio::test]
    async fn test_api_connection() {
        let api_key = match std::env::var("GEMINI_API_KEY") {
            Ok(key) => key,
            Err(_) => {
                println!("GEMINI_API_KEY environment variable not set, skipping test");
                return;
            }
        };
        
        let url = format!("wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key={}", api_key);
        
        let client_result = GeminiClient::connect(&url).await;
        assert!(client_result.is_ok(), "Failed to connect to Gemini API: {:?}", client_result.err());
        
        let mut client = client_result.unwrap();
        
        // Send setup message
        let setup = BidiGenerateContentSetup {
            model: "models/gemini-2.0-flash-live-001".to_string(),
            ..Default::default()
        };
        let msg = ClientMessage::Setup { setup };
        
        let send_result = client.send(&msg).await;
        assert!(send_result.is_ok(), "Failed to send setup message: {:?}", send_result.err());
        
        // Wait for setup complete with timeout
        let setup_complete = timeout(Duration::from_secs(5), async {
            while let Some(result) = client.next().await {
                match result {
                    Ok(ServerMessage::SetupComplete { .. }) => return true,
                    Ok(_) => continue, // Skip other message types
                    Err(e) => panic!("Error receiving message: {:?}", e),
                }
            }
            false
        }).await;
        
        assert!(setup_complete.is_ok() && setup_complete.unwrap(), "Did not receive SetupComplete message in time");
        
        // Test with simple text input to verify it works
        let content_obj = serde_json::json!({
            "turns": [{
                "role": "user",
                "parts": [{
                    "text": "Hello, Gemini. Can you give me a short response for testing?"
                }]
            }],
            "turnComplete": true
        });
        
        let msg = ClientMessage::ClientContent { client_content: content_obj };
        let send_result = client.send(&msg).await;
        assert!(send_result.is_ok(), "Failed to send client content: {:?}", send_result.err());
        
        // Wait for response with timeout
        let got_response = timeout(Duration::from_secs(10), async {
            while let Some(result) = client.next().await {
                match result {
                    Ok(ServerMessage::ServerContent { server_content }) => {
                        // Check if we got model text response 
                        if server_content.get("modelTurn").is_some() {
                            return true;
                        }
                    },
                    Ok(_) => continue, // Skip other message types
                    Err(e) => panic!("Error receiving message: {:?}", e),
                }
            }
            false
        }).await;
        
        assert!(got_response.is_ok() && got_response.unwrap(), "Did not receive server content response in time");
    }
}

/// Generation configuration for setup.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RealtimeAudio {
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RealtimeVideo {
    pub data: String,
    pub mime_type: String,
}

/// Message sent from client to server.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ClientMessage {
    Setup {
        setup: BidiGenerateContentSetup
    },
    ClientContent {
        client_content: serde_json::Value
    },
    RealtimeInput {
        realtime_input: RealtimeInput
    },
    ToolResponse {
        tool_response: serde_json::Value
    },
}

/// Server -> client messages
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    SetupComplete {
        #[serde(rename = "setupComplete")]
        setup_complete: serde_json::Value 
    },
    ServerContent {
        #[serde(rename = "serverContent")]
        server_content: serde_json::Value 
    },
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: serde_json::Value 
    },
    ToolCallCancellation {
        #[serde(rename = "toolCallCancellation")]
        tool_call_cancellation: serde_json::Value 
    },
    GoAway {
        #[serde(rename = "goAway")]
        go_away: serde_json::Value 
    },
    SessionResumptionUpdate {
        #[serde(rename = "sessionResumptionUpdate")]
        session_resumption_update: serde_json::Value 
    },
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
        let json = match msg {
            ClientMessage::Setup { setup } => {
                // Format the JSON manually to avoid nesting issues
                let setup_json = serde_json::to_string(setup)
                    .map_err(|e| WsError::Io(io::Error::new(io::ErrorKind::Other, e)))?;
                // Remove outer braces and wrap in setup: {...}
                let inner = &setup_json[1..setup_json.len()-1];
                format!("{{\"setup\":{{{}}}}}", inner)
            },
            ClientMessage::ClientContent { client_content } => {
                format!("{{\"clientContent\":{}}}", serde_json::to_string(client_content)
                    .map_err(|e| WsError::Io(io::Error::new(io::ErrorKind::Other, e)))?)
            },
            ClientMessage::RealtimeInput { realtime_input } => {
                format!("{{\"realtimeInput\":{}}}", serde_json::to_string(realtime_input)
                    .map_err(|e| WsError::Io(io::Error::new(io::ErrorKind::Other, e)))?)
            },
            ClientMessage::ToolResponse { tool_response } => {
                format!("{{\"toolResponse\":{}}}", serde_json::to_string(tool_response)
                    .map_err(|e| WsError::Io(io::Error::new(io::ErrorKind::Other, e)))?)
            },
        };
        println!("Sending message: {}", json);
        self.ws.send(Message::text(json)).await
    }

    /// Receive the next server message.
    pub async fn next(&mut self) -> Option<Result<ServerMessage, WsError>> {
        loop {
            match self.ws.next().await? {
                Ok(Message::Text(text)) => {
                    println!("Received text message: {}", text);
                    let parsed = serde_json::from_str::<ServerMessage>(&text)
                        .map_err(|e| {
                            println!("Error parsing message: {}", e);
                            WsError::Io(io::Error::new(io::ErrorKind::Other, e))
                        });
                    return Some(parsed);
                }
                Ok(Message::Binary(bytes)) => {
                    // Handle binary data - convert to string and parse
                    match std::str::from_utf8(&bytes) {
                        Ok(text) => {
                            println!("Received binary message (as text): {}", text);
                            let parsed = serde_json::from_str::<ServerMessage>(text)
                                .map_err(|e| {
                                    println!("Error parsing binary message: {}", e);
                                    WsError::Io(io::Error::new(io::ErrorKind::Other, e))
                                });
                            return Some(parsed);
                        }
                        Err(e) => {
                            println!("Error converting binary to string: {}", e);
                            continue;
                        }
                    }
                }
                Ok(Message::Close(frame)) => {
                    println!("WebSocket closed: {:?}", frame);
                    return None;
                }
                Ok(other) => {
                    println!("Received other message type: {:?}", other);
                    continue;
                }
                Err(e) => {
                    println!("WebSocket error: {:?}", e);
                    return Some(Err(e));
                }
            }
        }
    }
}

