//! Gemini Live API module
//!
//! Provides a client for interacting with Google's Gemini Live API via WebSockets.
//! Handles audio, video, and text streaming with the model.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Error as WsError;
use tracing::error;

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

/// Content structure for system instructions and messages
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>, // "SYSTEM" | "USER" | "MODEL"
    pub parts: Vec<Part>,
}

/// Part of a content message
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Session setup message.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BidiGenerateContentSetup {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>, // Changed from String to Content
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
    pub activity_start: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_end: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_end: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RealtimeAudio {
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RealtimeVideo {
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

/// Message sent from client to server.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum ClientMessage {
    Setup { setup: BidiGenerateContentSetup },
    ClientContent { 
        #[serde(rename = "clientContent")]
        client_content: serde_json::Value 
    },
    RealtimeInput { 
        #[serde(rename = "realtimeInput")]
        realtime_input: RealtimeInput 
    },
    ToolResponse { 
        #[serde(rename = "toolResponse")]
        tool_response: serde_json::Value 
    },
}

/// Server -> client messages
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    SetupComplete {
        #[serde(rename = "setupComplete")]
        setup_complete: serde_json::Value,
    },
    ServerContent {
        #[serde(rename = "serverContent")]
        server_content: serde_json::Value,
    },
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: serde_json::Value,
    },
    ToolCallCancellation {
        #[serde(rename = "toolCallCancellation")]
        tool_call_cancellation: serde_json::Value,
    },
    GoAway {
        #[serde(rename = "goAway")]
        go_away: serde_json::Value,
    },
    SessionResumptionUpdate {
        #[serde(rename = "sessionResumptionUpdate")]
        session_resumption_update: serde_json::Value,
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

// Manual Clone implementation that converts non-cloneable errors to strings
impl Clone for GeminiError {
    fn clone(&self) -> Self {
        match self {
            Self::WebSocket(e) => Self::Other(format!("WebSocket error: {}", e)),
            Self::Serialization(e) => Self::Other(format!("Serialization error: {}", e)),
            Self::Io(e) => Self::Other(format!("I/O error: {}", e)),
            Self::ConnectionClosed => Self::ConnectionClosed,
            Self::SetupNotComplete => Self::SetupNotComplete,
            Self::ChannelClosed => Self::ChannelClosed,
            Self::Timeout => Self::Timeout,
            Self::Other(s) => Self::Other(s.clone()),
        }
    }
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
    TextResponse { text: String, is_complete: bool },

    /// Audio response from the model
    AudioResponse { data: Vec<u8>, is_complete: bool },

    /// Model is requesting a tool call
    ToolCall(serde_json::Value),

    /// Model has cancelled a tool call
    ToolCallCancellation(String),

    /// Server will disconnect soon
    GoAway,

    /// Session resumption token provided
    SessionResumptionUpdate(String),

    /// Generation of a response is complete
    GenerationComplete,

    /// Special message indicating connection closed, should trigger client cleanup
    ConnectionClosed,
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
    pub fn as_str(&self) -> &'static str {
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
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "MEDIA_RESOLUTION_LOW",
            Self::Medium => "MEDIA_RESOLUTION_MEDIUM",
            Self::High => "MEDIA_RESOLUTION_HIGH",
        }
    }
}
