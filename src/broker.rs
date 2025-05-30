use crate::events::{InEvent, TurnInput, WsOut, WsIn, FrameId};
use base64::Engine;
use std::time::{Duration, Instant};

const VIDEO_GRACE_MS: u64 = 1000;
const MIN_NEW_FRAMES: usize = 2;
const MAX_FRAMES_PER_TURN: usize = 5;

#[derive(Debug)]
pub enum State {
    Idle,
    CollectingSpeech { start: Instant },
    CollectingVideoOnly { start: Instant },
}

pub struct Broker {
    state: State,
    recent_frames: Vec<FrameId>,
    last_audio_time: Instant,
    frames_sent_in_turn: usize,
}

#[derive(Debug)]
pub enum Event {
    Input(InEvent),
    Ws(WsIn),
}

impl Broker {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            recent_frames: Vec::new(),
            last_audio_time: Instant::now(),
            frames_sent_in_turn: 0,
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<WsOut> {
        match event {
            Event::Input(input) => self.handle_input(input),
            Event::Ws(ws) => self.handle_ws(ws),
        }
    }

    fn handle_input(&mut self, input: InEvent) -> Vec<WsOut> {
        match input {
            InEvent::AudioChunk(chunk) => {
                self.last_audio_time = Instant::now();
                vec![]
            }
            InEvent::UniqueFrame { jpeg, hash } => {
                self.recent_frames.push(FrameId {
                    jpeg,
                    hash,
                    timestamp: Instant::now(),
                });
                
                if self.recent_frames.len() > 10 {
                    self.recent_frames.remove(0);
                }

                match &self.state {
                    State::Idle => {
                        let frames_since_turn = self.recent_frames.len() - self.frames_sent_in_turn;
                        let time_since_audio = self.last_audio_time.elapsed();
                        
                        if frames_since_turn >= MIN_NEW_FRAMES 
                            && time_since_audio > Duration::from_millis(VIDEO_GRACE_MS) {
                            self.start_video_turn()
                        } else {
                            vec![]
                        }
                    }
                    _ => vec![]
                }
            }
        }
    }

    fn handle_ws(&mut self, ws: WsIn) -> Vec<WsOut> {
        match ws {
            WsIn::GenerationComplete => {
                self.state = State::Idle;
                vec![]
            }
            _ => vec![]
        }
    }

    pub fn handle_speech_turn(&mut self, turn: TurnInput) -> Vec<WsOut> {
        if !matches!(self.state, State::Idle) {
            return vec![];
        }

        match turn {
            TurnInput::SpeechTurn { pcm, t_start, draft_text } => {
                self.state = State::CollectingSpeech { start: t_start };
                let mut messages = vec![];
                
                // Send activityStart
                let start_input = serde_json::json!({
                    "activityStart": {}
                });
                messages.push(WsOut::RealtimeInput(start_input));

                // Send audio data
                let audio_input = serde_json::json!({
                    "audio": {
                        "data": base64::engine::general_purpose::STANDARD.encode(&pcm),
                        "mimeType": "audio/pcm;rate=16000"
                    }
                });
                messages.push(WsOut::RealtimeInput(audio_input));

                let recent_frame_cutoff = Instant::now() - Duration::from_secs(1);
                let frames_to_send: Vec<_> = self.recent_frames.iter()
                    .filter(|f| f.timestamp > recent_frame_cutoff)
                    .take(MAX_FRAMES_PER_TURN)
                    .collect();

                // Send video frames
                for frame in &frames_to_send {
                    let video_input = serde_json::json!({
                        "video": {
                            "data": base64::engine::general_purpose::STANDARD.encode(&frame.jpeg),
                            "mimeType": "image/jpeg"
                        }
                    });
                    messages.push(WsOut::RealtimeInput(video_input));
                }

                self.frames_sent_in_turn = self.recent_frames.len();

                messages.push(WsOut::RealtimeInput(serde_json::json!({
                    "activityEnd": {}
                })));

                messages
            }
            _ => vec![]
        }
    }

    fn start_video_turn(&mut self) -> Vec<WsOut> {
        self.state = State::CollectingVideoOnly { start: Instant::now() };
        let mut messages = vec![];

        messages.push(WsOut::RealtimeInput(serde_json::json!({
            "activityStart": {}
        })));

        let frames_to_send = self.recent_frames.len() - self.frames_sent_in_turn;
        let frames_to_send = frames_to_send.min(MAX_FRAMES_PER_TURN);
        let start_idx = self.recent_frames.len().saturating_sub(frames_to_send);

        // Send video frames
        for frame in &self.recent_frames[start_idx..] {
            let video_input = serde_json::json!({
                "video": {
                    "data": base64::engine::general_purpose::STANDARD.encode(&frame.jpeg),
                    "mimeType": "image/jpeg"
                }
            });
            messages.push(WsOut::RealtimeInput(video_input));
        }

        self.frames_sent_in_turn = self.recent_frames.len();

        messages.push(WsOut::RealtimeInput(serde_json::json!({
            "activityEnd": {}
        })));

        messages
    }
}