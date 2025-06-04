//! Unified media event types for the refactored architecture

use std::time::Instant;

/// Media events emitted by capture tasks
#[derive(Clone, Debug)]
pub enum MediaEvent {
    /// Raw audio frame from microphone (16kHz mono PCM)
    AudioFrame {
        pcm: Vec<i16>,
        timestamp: Instant,
    },
    /// Deduplicated video frame (JPEG encoded)
    VideoFrame {
        jpeg: Vec<u8>,
        frame_id: u64,
        timestamp: Instant,
    },
    /// Request to force capture a video frame
    ForceCaptureRequest {
        requester_id: String,
    },
}

/// WebSocket outbound messages (Gemini protocol)
#[derive(Clone, Debug)]
pub enum WsOutbound {
    /// JSON message to send to Gemini
    Json(serde_json::Value),
}

/// WebSocket inbound messages from Gemini
#[derive(Clone, Debug)]
pub enum WsInbound {
    /// Text response from model
    Text {
        content: String,
        is_final: bool,
    },
    /// Generation completed
    GenerationComplete,
    /// Tool call request
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    /// Error from API
    Error(String),
}

/// Turn boundary events from audio segmentation
#[derive(Clone, Debug)]
pub enum TurnBoundary {
    /// Start of speech detected
    TurnStart {
        timestamp: Instant,
    },
    /// End of speech with transcription
    TurnEnd {
        pcm: Vec<u8>,
        text: Option<String>,
        timestamp: Instant,
        duration_ms: u64,
    },
    /// Streaming audio chunk during turn
    StreamingAudio {
        pcm: Vec<u8>,
        timestamp: Instant,
    },
}

/// Messages sent from producers (audio/video) to the websocket writer
#[derive(Debug, Clone)]
pub enum Outgoing {
    ActivityStart(u64),           // turn-id
    AudioChunk(Vec<u8>, u64),     // data, turn-id
    VideoFrame(Vec<u8>, u64),     // jpeg data, turn-id  
    ActivityEnd(u64),             // turn-id
}