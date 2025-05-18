//! Gemini Live API module
//! 
//! Provides a client for interacting with Google's Gemini Live API via WebSockets.
//! Handles audio, video, and text streaming with the model.

use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_tungstenite::tungstenite::Error as WsError;
use std::time::Duration;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use futures_util::{StreamExt, SinkExt};
use tracing::{debug, error, info};
use base64::engine::general_purpose;
use base64::Engine;

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
        
        // Create a client with minimal configuration
        let config = GeminiClientConfig {
            url: String::new(), // Will be set by from_api_key
            model: "models/gemini-2.0-flash-live-001".to_string(),
            response_modality: ResponseModality::Text,
            system_instruction: Some("".to_string()),
            temperature: Some(0.7),
            media_resolution: Some(MediaResolution::Medium),
            reconnect_attempts: 1,
            reconnect_delay: Duration::from_secs(1),
        };
        
        let mut client = GeminiClient::from_api_key(&api_key, Some(config));
        
        // Connect to the API
        let connect_result = client.connect().await;
        assert!(connect_result.is_ok(), "Failed to connect to Gemini API: {:?}", connect_result.err());
        
        // Set up the session
        let setup_result = client.setup().await;
        assert!(setup_result.is_ok(), "Failed to set up Gemini session: {:?}", setup_result.err());
        
        // Test with simple text input to verify it works
        let text_result = client.send_text("Hello, Gemini. Can you give me a short response for testing?").await;
        assert!(text_result.is_ok(), "Failed to send text message: {:?}", text_result.err());
        
        // Wait for a response with timeout
        let mut got_response = false;
        let wait_result = timeout(Duration::from_secs(10), async {
            while let Some(result) = client.next_response().await {
                match result {
                    Ok(ApiResponse::TextResponse { text, .. }) => {
                        println!("Received text response: {}", text);
                        got_response = true;
                        break;
                    }
                    Ok(ApiResponse::InputTranscription(transcript)) => {
                        println!("Received transcript: {}", transcript.text);
                        continue;
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        println!("Error from API: {:?}", e);
                        continue;
                    }
                }
            }
        }).await;
        
        assert!(wait_result.is_ok(), "Timed out waiting for response");
        assert!(got_response, "Did not receive text response from Gemini");
    }

    #[tokio::test]
    async fn test_handle_text_message_variants() {
        let (tx, mut rx) = mpsc::channel(10);

        // SetupComplete
        let msg = serde_json::json!({"setupComplete": {}}).to_string();
        GeminiClient::handle_text_message(&msg, &tx).await.unwrap();
        match rx.recv().await.unwrap().unwrap() {
            ApiResponse::SetupComplete => {}
            other => panic!("Unexpected response: {:?}", other),
        }

        // ToolCall
        let msg = serde_json::json!({"toolCall": {"id": "123"}}).to_string();
        GeminiClient::handle_text_message(&msg, &tx).await.unwrap();
        match rx.recv().await.unwrap().unwrap() {
            ApiResponse::ToolCall(val) => {
                assert_eq!(val["id"], "123");
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_handle_server_content_variants() {
        let (tx, mut rx) = mpsc::channel(10);

        // Input transcription
        let content = serde_json::json!({"inputTranscription": {"text": "hello", "isFinal": true}});
        GeminiClient::handle_server_content(content, &tx).await.unwrap();
        match rx.recv().await.unwrap().unwrap() {
            ApiResponse::InputTranscription(t) => {
                assert_eq!(t.text, "hello");
                assert!(t.is_final);
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        // Output transcription
        let content = serde_json::json!({"outputTranscription": {"text": "hi", "isFinal": false}});
        GeminiClient::handle_server_content(content, &tx).await.unwrap();
        match rx.recv().await.unwrap().unwrap() {
            ApiResponse::OutputTranscription(t) => {
                assert_eq!(t.text, "hi");
                assert!(!t.is_final);
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        // Text response
        let content = serde_json::json!({
            "modelTurn": {"parts": [{"text": "done"}]},
            "generationComplete": true
        });
        GeminiClient::handle_server_content(content, &tx).await.unwrap();
        match rx.recv().await.unwrap().unwrap() {
            ApiResponse::TextResponse { text, is_complete } => {
                assert_eq!(text, "done");
                assert!(is_complete);
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        // Audio response
        let data = general_purpose::STANDARD.encode(&[1u8, 2, 3]);
        let content = serde_json::json!({
            "modelTurn": {"parts": [{"inlineData": {"data": data}}]}
        });
        GeminiClient::handle_server_content(content, &tx).await.unwrap();
        match rx.recv().await.unwrap().unwrap() {
            ApiResponse::AudioResponse { data, is_complete } => {
                assert_eq!(data, vec![1, 2, 3]);
                assert!(!is_complete);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_enum_as_str() {
        assert_eq!(ResponseModality::Text.as_str(), "TEXT");
        assert_eq!(ResponseModality::Audio.as_str(), "AUDIO");
        assert_eq!(MediaResolution::Low.as_str(), "MEDIA_RESOLUTION_LOW");
        assert_eq!(MediaResolution::Medium.as_str(), "MEDIA_RESOLUTION_MEDIUM");
        assert_eq!(MediaResolution::High.as_str(), "MEDIA_RESOLUTION_HIGH");
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

/// Error type for Gemini API operations
#[derive(Debug, thiserror::Error)]
pub enum GeminiError {
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] WsError),
    
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Connection closed")]
    ConnectionClosed,
    
    #[error("Setup not complete")]
    SetupNotComplete,
    
    #[error("Channel closed")]
    ChannelClosed,
    
    #[error("Timeout")]
    Timeout,
    
    #[error("Other error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, GeminiError>;

/// Transcript from the Gemini API
#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub is_final: bool,
}

/// Response from the Gemini API
#[derive(Debug, Clone)]
pub enum ApiResponse {
    /// Setup has been completed
    SetupComplete,
    
    /// Transcription of user input
    InputTranscription(Transcript),
    
    /// Transcription of model output (if using TTS)
    OutputTranscription(Transcript),
    
    /// Text response from the model
    TextResponse {
        text: String,
        is_complete: bool,
    },
    
    /// Audio response from the model
    AudioResponse {
        data: Vec<u8>,
        is_complete: bool,
    },
    
    /// Model is requesting a tool call
    ToolCall(serde_json::Value),
    
    /// Model has cancelled a tool call
    ToolCallCancellation(String),
    
    /// Server will disconnect soon
    GoAway,
    
    /// Session resumption token provided
    SessionResumptionUpdate(String),
}

/// Configuration for the Gemini client
#[derive(Debug, Clone)]
pub struct GeminiClientConfig {
    pub url: String,
    pub model: String,
    pub response_modality: ResponseModality,
    pub system_instruction: Option<String>,
    pub temperature: Option<f32>,
    pub media_resolution: Option<MediaResolution>,
    pub reconnect_attempts: usize,
    pub reconnect_delay: Duration,
}

impl Default for GeminiClientConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            model: "models/gemini-2.0-flash-live-001".to_string(),
            response_modality: ResponseModality::Text,
            system_instruction: None,
            temperature: Some(0.7),
            media_resolution: Some(MediaResolution::Medium),
            reconnect_attempts: 3,
            reconnect_delay: Duration::from_secs(1),
        }
    }
}

/// Response modality options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseModality {
    Text,
    Audio,
}

impl ResponseModality {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "TEXT",
            Self::Audio => "AUDIO",
        }
    }
}

/// Media resolution options for video input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaResolution {
    Low,
    Medium,
    High,
}

impl MediaResolution {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "MEDIA_RESOLUTION_LOW",
            Self::Medium => "MEDIA_RESOLUTION_MEDIUM",
            Self::High => "MEDIA_RESOLUTION_HIGH",
        }
    }
}

/// Connection state of the Gemini client
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    SetupComplete,
}

/// Async Gemini Live API client with streaming support.
pub struct GeminiClient {
    config: GeminiClientConfig,
    ws: Option<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>,
    state: ConnectionState,
    session_token: Option<String>,
    response_tx: mpsc::Sender<Result<ApiResponse>>,
    response_rx: mpsc::Receiver<Result<ApiResponse>>,
}

impl GeminiClient {
    /// Create a new Gemini client with the given configuration.
    pub fn new(config: GeminiClientConfig) -> Self {
        let (response_tx, response_rx) = mpsc::channel(100);
        
        Self {
            config,
            ws: None,
            state: ConnectionState::Disconnected,
            session_token: None,
            response_tx,
            response_rx,
        }
    }
    
    /// Create a new Gemini client from an API key and optional configuration.
    pub fn from_api_key(api_key: &str, config: Option<GeminiClientConfig>) -> Self {
        let mut config = config.unwrap_or_default();
        config.url = format!(
            "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key={}",
            api_key
        );
        Self::new(config)
    }
    
    /// Connect to the Live API endpoint and set up the session.
    pub async fn connect_and_setup(&mut self) -> Result<()> {
        self.connect().await?;
        self.setup().await
    }
    
    /// Connect to the Live API endpoint.
    pub async fn connect(&mut self) -> Result<()> {
        if self.state != ConnectionState::Disconnected {
            return Ok(());
        }
        
        self.state = ConnectionState::Connecting;
        info!("Connecting to Gemini API at {}", self.config.url);
        
        let (ws, _resp) = connect_async(&self.config.url).await
            .map_err(GeminiError::WebSocket)?;
        
        self.ws = Some(ws);
        self.state = ConnectionState::Connected;
        info!("Connected to Gemini API");
        
        // Start the message processing loop
        self.start_message_processing();
        
        Ok(())
    }
    
    /// Initialize a session by sending the setup message.
    pub async fn setup(&mut self) -> Result<()> {
        if self.state == ConnectionState::Disconnected {
            return Err(GeminiError::ConnectionClosed);
        }
        
        if self.state == ConnectionState::SetupComplete {
            return Ok(());
        }
        
        info!("Setting up Gemini session");
        
        // Create the setup message
        let mut setup = BidiGenerateContentSetup {
            model: self.config.model.clone(),
            system_instruction: self.config.system_instruction.clone(),
            ..Default::default()
        };
        
        // Set up generation config
        let mut generation_config = GenerationConfig {
            response_modalities: vec![self.config.response_modality.as_str().to_string()],
            temperature: self.config.temperature,
            ..Default::default()
        };
        
        // Add media resolution if specified
        if let Some(resolution) = self.config.media_resolution {
            generation_config.media_resolution = Some(resolution.as_str().to_string());
        }
        
        setup.generation_config = Some(generation_config);
        
        // Set resumption token if we have one
        if let Some(token) = &self.session_token {
            setup.realtime_input_config = Some(serde_json::json!({
                "sessionResumptionConfig": {
                    "handle": token
                }
            }));
        }
        
        // Send the setup message
        let msg = ClientMessage::Setup { setup };
        self.send(&msg).await?;
        
        // Wait for setup complete response
        let timeout = tokio::time::timeout(
            Duration::from_secs(10),
            self.wait_for_setup_complete()
        ).await.map_err(|_| GeminiError::Timeout)?;
        
        if timeout {
            self.state = ConnectionState::SetupComplete;
            info!("Gemini session setup complete");
            Ok(())
        } else {
            error!("Failed to complete Gemini session setup");
            Err(GeminiError::SetupNotComplete)
        }
    }
    
    /// Wait for the setup complete message.
    async fn wait_for_setup_complete(&mut self) -> bool {
        let mut attempts = 0;
        while attempts < 10 {
            match self.response_rx.recv().await {
                Some(Ok(ApiResponse::SetupComplete)) => {
                    return true;
                }
                Some(_) => {
                    // Ignore other messages
                    attempts += 1;
                    continue;
                }
                None => {
                    return false;
                }
            }
        }
        false
    }
    
    /// Start the message processing loop in a background task.
    fn start_message_processing(&mut self) {
        if let Some(ws) = self.ws.take() {
            let ws = Arc::new(Mutex::new(ws));
            let response_tx = self.response_tx.clone();
            
            tokio::spawn(async move {
                Self::process_messages(ws, response_tx).await;
            });
        }
    }
    
    /// Process incoming messages from the WebSocket.
    async fn process_messages(
        ws: Arc<Mutex<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>>,
        response_tx: mpsc::Sender<Result<ApiResponse>>,
    ) {
        loop {
            let message = {
                let mut ws_guard = ws.lock().await;
                match ws_guard.next().await {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => {
                        if let Err(_) = response_tx.send(Err(GeminiError::WebSocket(e))).await {
                            error!("Failed to send error to response channel");
                        }
                        break;
                    }
                    None => {
                        if let Err(_) = response_tx.send(Err(GeminiError::ConnectionClosed)).await {
                            error!("Failed to send connection closed error");
                        }
                        break;
                    }
                }
            };
            
            match message {
                Message::Text(text) => {
                    debug!("Received text message: {}", text);
                    if let Err(e) = Self::handle_text_message(&text, &response_tx).await {
                        error!("Error handling text message: {:?}", e);
                    }
                }
                Message::Binary(bytes) => {
                    debug!("Received binary message ({} bytes)", bytes.len());
                    // Could be audio data or other binary content
                    // Handle based on context
                }
                Message::Close(frame) => {
                    info!("WebSocket closed: {:?}", frame);
                    if let Err(_) = response_tx.send(Err(GeminiError::ConnectionClosed)).await {
                        error!("Failed to send connection closed notification");
                    }
                    break;
                }
                _ => {
                    // Ignore other message types
                }
            }
        }
    }
    
    /// Handle a text message from the WebSocket.
    async fn handle_text_message(text: &str, response_tx: &mpsc::Sender<Result<ApiResponse>>) -> Result<()> {
        let server_message = serde_json::from_str::<ServerMessage>(text)
            .map_err(GeminiError::Serialization)?;
            
        match server_message {
            ServerMessage::SetupComplete { .. } => {
                response_tx.send(Ok(ApiResponse::SetupComplete)).await
                    .map_err(|_| GeminiError::ChannelClosed)?;
            }
            ServerMessage::ServerContent { server_content } => {
                Self::handle_server_content(server_content, response_tx).await?;
            }
            ServerMessage::ToolCall { tool_call } => {
                response_tx.send(Ok(ApiResponse::ToolCall(tool_call))).await
                    .map_err(|_| GeminiError::ChannelClosed)?;
            }
            ServerMessage::ToolCallCancellation { tool_call_cancellation } => {
                let id = tool_call_cancellation["id"].as_str()
                    .unwrap_or("unknown")
                    .to_string();
                response_tx.send(Ok(ApiResponse::ToolCallCancellation(id))).await
                    .map_err(|_| GeminiError::ChannelClosed)?;
            }
            ServerMessage::GoAway { .. } => {
                response_tx.send(Ok(ApiResponse::GoAway)).await
                    .map_err(|_| GeminiError::ChannelClosed)?;
            }
            ServerMessage::SessionResumptionUpdate { session_resumption_update } => {
                let handle = session_resumption_update["newHandle"].as_str()
                    .unwrap_or("")
                    .to_string();
                response_tx.send(Ok(ApiResponse::SessionResumptionUpdate(handle))).await
                    .map_err(|_| GeminiError::ChannelClosed)?;
            }
        }
        
        Ok(())
    }
    
    /// Handle server content messages which can contain different types of data.
    async fn handle_server_content(
        content: serde_json::Value, 
        response_tx: &mpsc::Sender<Result<ApiResponse>>
    ) -> Result<()> {
        // Check for input transcription (from audio we sent)
        if let Some(input_transcription) = content.get("inputTranscription") {
            if let Some(text) = input_transcription.get("text").and_then(|t| t.as_str()) {
                let is_final = input_transcription.get("isFinal")
                    .and_then(|f| f.as_bool())
                    .unwrap_or(false);
                
                response_tx.send(Ok(ApiResponse::InputTranscription(Transcript {
                    text: text.to_string(),
                    is_final,
                }))).await.map_err(|_| GeminiError::ChannelClosed)?;
            }
        }
        
        // Check for output transcription (text of model's speech)
        if let Some(output_transcription) = content.get("outputTranscription") {
            if let Some(text) = output_transcription.get("text").and_then(|t| t.as_str()) {
                let is_final = output_transcription.get("isFinal")
                    .and_then(|f| f.as_bool())
                    .unwrap_or(false);
                
                response_tx.send(Ok(ApiResponse::OutputTranscription(Transcript {
                    text: text.to_string(),
                    is_final,
                }))).await.map_err(|_| GeminiError::ChannelClosed)?;
            }
        }
        
        // Check for model turn (the actual response)
        if let Some(model_turn) = content.get("modelTurn") {
            // For text response
            if let Some(parts) = model_turn.get("parts").and_then(|p| p.as_array()) {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        let is_complete = content.get("generationComplete")
                            .and_then(|g| g.as_bool())
                            .unwrap_or(false);
                        
                        response_tx.send(Ok(ApiResponse::TextResponse {
                            text: text.to_string(),
                            is_complete,
                        })).await.map_err(|_| GeminiError::ChannelClosed)?;
                    } else if let Some(inline_data) = part.get("inlineData") {
                        // Audio response
                        if let Some(data_str) = inline_data.get("data").and_then(|d| d.as_str()) {
                            if let Ok(data) = general_purpose::STANDARD.decode(data_str) {
                                let is_complete = content.get("generationComplete")
                                    .and_then(|g| g.as_bool())
                                    .unwrap_or(false);
                                
                                response_tx.send(Ok(ApiResponse::AudioResponse {
                                    data,
                                    is_complete,
                                })).await.map_err(|_| GeminiError::ChannelClosed)?;
                            }
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Send a client message to the server.
    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let json = match msg {
            ClientMessage::Setup { setup } => {
                // Format the JSON manually to avoid nesting issues
                let setup_json = serde_json::to_string(setup)
                    .map_err(GeminiError::Serialization)?;
                // Remove outer braces and wrap in setup: {...}
                let inner = &setup_json[1..setup_json.len()-1];
                format!("{{\"setup\":{{{}}}}}", inner)
            },
            ClientMessage::ClientContent { client_content } => {
                format!("{{\"clientContent\":{}}}", 
                    serde_json::to_string(client_content).map_err(GeminiError::Serialization)?)
            },
            ClientMessage::RealtimeInput { realtime_input } => {
                format!("{{\"realtimeInput\":{}}}", 
                    serde_json::to_string(realtime_input).map_err(GeminiError::Serialization)?)
            },
            ClientMessage::ToolResponse { tool_response } => {
                format!("{{\"toolResponse\":{}}}", 
                    serde_json::to_string(tool_response).map_err(GeminiError::Serialization)?)
            },
        };
        
        debug!("Sending message: {}", json);
        
        if let Some(ws) = &mut self.ws {
            ws.send(Message::text(json)).await.map_err(GeminiError::WebSocket)?;
            Ok(())
        } else {
            Err(GeminiError::ConnectionClosed)
        }
    }
    
    /// Send an audio chunk to the server.
    pub async fn send_audio(&mut self, audio_data: &[u8], is_start: bool, is_end: bool) -> Result<()> {
        // Encode audio data as base64
        let data = general_purpose::STANDARD.encode(audio_data);
        
        let realtime_input = RealtimeInput {
            audio: Some(RealtimeAudio {
                data,
                mime_type: "audio/pcm;rate=16000".to_string(),
            }),
            video: None,
            text: None,
            activity_start: if is_start { Some(true) } else { None },
            activity_end: if is_end { Some(true) } else { None },
            audio_stream_end: if is_end { Some(true) } else { None },
        };
        
        let msg = ClientMessage::RealtimeInput { realtime_input };
        self.send(&msg).await
    }
    
    /// Send a video frame to the server.
    pub async fn send_video(&mut self, frame_data: &[u8], mime_type: &str) -> Result<()> {
        // Encode video data as base64
        let data = general_purpose::STANDARD.encode(frame_data);
        
        let realtime_input = RealtimeInput {
            audio: None,
            video: Some(RealtimeVideo {
                data,
                mime_type: mime_type.to_string(),
            }),
            text: None,
            activity_start: None,
            activity_end: None,
            audio_stream_end: None,
        };
        
        let msg = ClientMessage::RealtimeInput { realtime_input };
        self.send(&msg).await
    }
    
    /// Send a text message to the server.
    pub async fn send_text(&mut self, text: &str) -> Result<()> {
        let client_content = serde_json::json!({
            "turns": [{
                "role": "user",
                "parts": [{
                    "text": text
                }]
            }],
            "turnComplete": true
        });
        
        let msg = ClientMessage::ClientContent { client_content };
        self.send(&msg).await
    }
    
    /// Send streaming text to the server (e.g. for partial typing).
    pub async fn send_streaming_text(&mut self, text: &str) -> Result<()> {
        let realtime_input = RealtimeInput {
            audio: None,
            video: None,
            text: Some(text.to_string()),
            activity_start: None,
            activity_end: None,
            audio_stream_end: None,
        };
        
        let msg = ClientMessage::RealtimeInput { realtime_input };
        self.send(&msg).await
    }
    
    /// Receive the next response from the server.
    pub async fn next_response(&mut self) -> Option<Result<ApiResponse>> {
        self.response_rx.recv().await
    }
    
    /// Stream responses until a condition is met.
    pub async fn stream_responses<F>(&mut self, mut callback: F) -> Result<()>
    where
        F: FnMut(&ApiResponse) -> bool,
    {
        while let Some(response) = self.response_rx.recv().await {
            match response {
                Ok(resp) => {
                    let should_stop = callback(&resp);
                    if should_stop {
                        break;
                    }
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
        
        Ok(())
    }
}

