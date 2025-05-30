use smallvec::SmallVec;
use std::time::Instant;

#[derive(Debug, Clone)]
pub enum InEvent {
    AudioChunk(Vec<i16>),
    UniqueFrame { jpeg: Vec<u8>, hash: u64 },
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