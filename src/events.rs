use smallvec::SmallVec;
use std::time::Instant;

#[derive(Debug, Clone)]
pub enum InEvent {
    AudioChunk(Vec<i16>),
    UniqueFrame { jpeg: Vec<u8>, hash: u64 },
}

/// Messages sent from producers (audio/video) to the websocket writer
#[derive(Debug, Clone)]
pub enum Outgoing {
    ActivityStart(u64),           // turn-id
    AudioChunk(Vec<u8>, u64),     // data, turn-id
    VideoFrame(Vec<u8>, u64),     // jpeg data, turn-id  
    ActivityEnd(u64),             // turn-id
}

#[derive(Debug, Clone)]
pub enum TurnInput {
    SpeechTurn {
        pcm: Vec<u8>,
        t_start: Instant,
        draft_text: Option<String>,
    },
    VideoTurn {
        frames: SmallVec<[FrameId; 8]>,
        t_start: Instant,
    },
    StreamingAudio {
        bytes: Vec<u8>,
        is_start: bool,
        is_end: bool,
    },
}

#[derive(Debug, Clone)]
pub struct FrameId {
    pub jpeg: Vec<u8>,
    pub hash: u64,
    pub timestamp: Instant,
}

#[derive(Debug, Clone)]
pub enum WsOut {
    Setup(serde_json::Value),
    RealtimeInput(serde_json::Value),
    ClientContent(serde_json::Value),
}

#[derive(Debug, Clone)]
pub enum WsIn {
    Text { content: String, is_final: bool },
    GenerationComplete,
    ToolCall { name: String, args: serde_json::Value },
    Error(String),
}