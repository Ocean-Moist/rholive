use crate::events::{InEvent, TurnInput, WsOut, WsIn, FrameId};
use base64::Engine;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tracing::{info, debug, warn};

const VIDEO_GRACE_MS: u64 = 1000;
const MIN_NEW_FRAMES: usize = 2;
const MAX_FRAMES_PER_TURN: usize = 5;

#[derive(Debug)]
pub enum State {
    Idle,
    CollectingSpeech { start: Instant, turn_id: u64 },
    CollectingVideoOnly { start: Instant, turn_id: u64 },
    StreamingSpeech { start: Instant, turn_id: u64 },
}

pub struct Broker {
    state: State,
    recent_frames: Vec<FrameId>,
    last_audio_time: Instant,
    frames_sent_in_turn: usize,
    
    // Latency Tracking
    next_turn_id: u64,
    pending_turns: VecDeque<(u64, Instant)>, // (turn_id, start_time)
    turn_latencies: VecDeque<Duration>,      // Store recent latencies
    max_latencies_to_store: usize,
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
            next_turn_id: 0,
            pending_turns: VecDeque::new(),
            turn_latencies: VecDeque::new(),
            max_latencies_to_store: 100, // Store last 100 latencies for averaging
        }
    }
    
    fn start_new_tracked_turn(&mut self) -> u64 {
        let turn_id = self.next_turn_id;
        self.next_turn_id += 1;
        let now = Instant::now();
        self.pending_turns.push_back((turn_id, now));
        
        // Console output for turn start
        println!("\n>>> TURN START: ID={} | Pending={} | Time={:?}", 
            turn_id, 
            self.pending_turns.len(),
            now.elapsed()
        );
        
        info!(
            "TURN_TRACKING: Initiated turn_id {}. Pending turns: {}.",
            turn_id,
            self.pending_turns.len()
        );
        turn_id
    }
    
    fn complete_tracked_turn(&mut self) {
        if let Some((turn_id, start_time)) = self.pending_turns.pop_front() {
            let latency = start_time.elapsed();
            self.turn_latencies.push_back(latency);
            if self.turn_latencies.len() > self.max_latencies_to_store {
                self.turn_latencies.pop_front();
            }
            let avg_latency = self.average_latency();
            
            // Detailed console output
            println!("\n========== LATENCY REPORT ==========");
            println!("Turn ID:          {}", turn_id);
            println!("Latency:          {:.2}s ({:.0}ms)", latency.as_secs_f32(), latency.as_millis());
            println!("Pending Turns:    {}", self.pending_turns.len());
            if let Some(avg) = avg_latency {
                println!("Average Latency:  {:.2}s ({:.0}ms)", avg.as_secs_f32(), avg.as_millis());
            }
            println!("====================================\n");
            
            // Also log with tracing
            info!(
                "TURN_TRACKING: Completed turn_id {}. Latency: {:?}. Pending turns: {}. Avg Latency: {:?}.",
                turn_id,
                latency,
                self.pending_turns.len(),
                avg_latency
            );
        } else {
            warn!("TURN_TRACKING: Received GenerationComplete but no pending turns found.");
        }
    }
    
    fn average_latency(&self) -> Option<Duration> {
        if self.turn_latencies.is_empty() {
            None
        } else {
            let sum: Duration = self.turn_latencies.iter().sum();
            Some(sum / self.turn_latencies.len() as u32)
        }
    }
    
    pub fn get_pending_turns_count(&self) -> usize {
        self.pending_turns.len()
    }
    
    pub fn get_average_latency(&self) -> Option<Duration> {
        self.average_latency()
    }
    
    pub fn print_latency_summary(&self) {
        println!("\n########## LATENCY SUMMARY ##########");
        println!("Total Turns Completed: {}", self.next_turn_id);
        println!("Currently Pending:     {}", self.pending_turns.len());
        
        if !self.pending_turns.is_empty() {
            println!("\nPending Turn Details:");
            for (id, start_time) in &self.pending_turns {
                println!("  - Turn {} waiting for {:.2}s", id, start_time.elapsed().as_secs_f32());
            }
        }
        
        if !self.turn_latencies.is_empty() {
            let min = self.turn_latencies.iter().min().unwrap();
            let max = self.turn_latencies.iter().max().unwrap();
            let avg = self.average_latency().unwrap();
            
            println!("\nLatency Statistics (last {} turns):", self.turn_latencies.len());
            println!("  Min:     {:.2}s ({:.0}ms)", min.as_secs_f32(), min.as_millis());
            println!("  Max:     {:.2}s ({:.0}ms)", max.as_secs_f32(), max.as_millis());
            println!("  Average: {:.2}s ({:.0}ms)", avg.as_secs_f32(), avg.as_millis());
        }
        println!("#####################################\n");
    }

    pub fn handle(&mut self, event: Event) -> Vec<WsOut> {
        match event {
            Event::Input(input) => self.handle_input(input),
            Event::Ws(ws) => self.handle_ws(ws),
        }
    }

    fn handle_input(&mut self, input: InEvent) -> Vec<WsOut> {
        match input {
            InEvent::AudioChunk(_chunk) => {
                // Don't update last_audio_time here! 
                // Only completed speech turns should count as "recent audio"
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
                        
                        debug!("🤔 Video-only turn check: {} frames since last turn, {}ms since audio (need {} frames, {}ms grace)", 
                               frames_since_turn, time_since_audio.as_millis(), MIN_NEW_FRAMES, VIDEO_GRACE_MS);
                        
                        if frames_since_turn >= MIN_NEW_FRAMES 
                            && time_since_audio > Duration::from_millis(VIDEO_GRACE_MS) {
                            info!("✅ Starting video-only turn: {} new frames, {}ms audio silence", 
                                  frames_since_turn, time_since_audio.as_millis());
                            self.start_video_turn()
                        } else {
                            if frames_since_turn < MIN_NEW_FRAMES {
                                debug!("❌ Not enough new frames for video turn ({} < {})", frames_since_turn, MIN_NEW_FRAMES);
                            }
                            if time_since_audio <= Duration::from_millis(VIDEO_GRACE_MS) {
                                debug!("❌ Not enough audio silence for video turn ({}ms <= {}ms)", 
                                       time_since_audio.as_millis(), VIDEO_GRACE_MS);
                            }
                            vec![]
                        }
                    }
                    _ => {
                        debug!("❌ Cannot start video turn - broker not idle (state: {:?})", self.state);
                        vec![]
                    }
                }
            }
        }
    }

    fn handle_ws(&mut self, ws: WsIn) -> Vec<WsOut> {
        match ws {
            WsIn::GenerationComplete => {
                info!("🔄 Broker state transition: {:?} -> Idle (due to GenerationComplete)", self.state);
                self.complete_tracked_turn(); // Process latency for the completed turn
                self.state = State::Idle;
                vec![]
            }
            _ => vec![]
        }
    }

    pub fn handle_speech_turn(&mut self, turn: TurnInput) -> Vec<WsOut> {
        match turn {
            TurnInput::StreamingAudio { bytes, is_start, is_end } => {
                self.handle_streaming_audio(bytes, is_start, is_end)
            }
            TurnInput::SpeechTurn { pcm, t_start, draft_text: _ } => {
                if !matches!(self.state, State::Idle) {
                    info!("❌ Cannot handle speech turn - broker not idle (state: {:?})", self.state);
                    return vec![];
                }
                
                let turn_id = self.start_new_tracked_turn();
                self.last_audio_time = Instant::now();
                info!("🔊 Updated last_audio_time for completed speech turn");
                
                info!("🔄 Broker state transition: Idle -> CollectingSpeech (turn_id: {})", turn_id);
                self.state = State::CollectingSpeech { start: t_start, turn_id };
                let mut messages = vec![];
                
                // Console output for speech turn
                println!("🎤 SPEECH TURN {} | PCM: {} KB | Frames: {}", 
                    turn_id, 
                    pcm.len() / 1024,
                    self.recent_frames.len()
                );
                
                info!("🎯 Starting speech turn {} with {} recent frames", turn_id, self.recent_frames.len());
                
                let start_input = serde_json::json!({ "activityStart": {} });
                info!("📤 Sending activityStart to Gemini for turn_id {}", turn_id);
                messages.push(WsOut::RealtimeInput(start_input));

                // Send audio data
                let audio_size_kb = pcm.len() / 1024;
                let audio_input = serde_json::json!({
                    "audio": {
                        "data": base64::engine::general_purpose::STANDARD.encode(&pcm),
                        "mimeType": "audio/pcm;rate=16000"
                    }
                });
                info!("🎤 Sending audio chunk to Gemini for turn_id {}: {} KB", turn_id, audio_size_kb);
                messages.push(WsOut::RealtimeInput(audio_input));

                let recent_frame_cutoff = Instant::now() - Duration::from_secs(1);
                let frames_to_send: Vec<_> = self.recent_frames.iter()
                    .filter(|f| f.timestamp > recent_frame_cutoff)
                    .take(MAX_FRAMES_PER_TURN)
                    .collect();

                info!("📹 Sending {} video frames to Gemini for turn_id {}", frames_to_send.len(), turn_id);
                for (i, frame) in frames_to_send.iter().enumerate() {
                    let frame_size_kb = frame.jpeg.len() / 1024;
                    let video_input = serde_json::json!({
                        "video": {
                            "data": base64::engine::general_purpose::STANDARD.encode(&frame.jpeg),
                            "mimeType": "image/jpeg"
                        }
                    });
                    debug!("📸 Frame {}: {} KB", i + 1, frame_size_kb);
                    messages.push(WsOut::RealtimeInput(video_input));
                }

                self.frames_sent_in_turn = self.recent_frames.len();

                info!("📤 Sending activityEnd to Gemini for turn_id {}", turn_id);
                messages.push(WsOut::RealtimeInput(serde_json::json!({
                    "activityEnd": {}
                })));

                messages
            }
            _ => vec![]
        }
    }
    
    fn handle_streaming_audio(&mut self, bytes: Vec<u8>, is_start: bool, is_end: bool) -> Vec<WsOut> {
        let mut messages = vec![];
        
        if is_start {
            if !matches!(self.state, State::Idle) {
                info!("❌ Cannot start streaming - broker not idle (state: {:?})", self.state);
                return vec![];
            }
            
            let turn_id = self.start_new_tracked_turn();
            info!("🔄 Broker state transition: Idle -> StreamingSpeech (turn_id: {})", turn_id);
            self.state = State::StreamingSpeech { start: Instant::now(), turn_id };
            self.last_audio_time = Instant::now();
            
            println!("🎙️  STREAMING START | Turn ID: {}", turn_id);
            
            info!("📤 Sending activityStart for streaming audio (turn_id: {})", turn_id);
            messages.push(WsOut::RealtimeInput(serde_json::json!({ "activityStart": {} })));
        }
        
        if !bytes.is_empty() {
            if let State::StreamingSpeech { turn_id, .. } = self.state {
                 let audio_input = serde_json::json!({
                    "audio": {
                        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                        "mimeType": "audio/pcm;rate=16000"
                    }
                });
                debug!("🎤 Sending streaming audio chunk for turn_id {}", turn_id);
                messages.push(WsOut::RealtimeInput(audio_input));
            } else {
                debug!("❌ Ignoring audio chunk - not in streaming state or state is inconsistent.");
                return vec![];
            }
        }
        
        if is_end {
            if let State::StreamingSpeech { turn_id, .. } = self.state {
                println!("🎙️  STREAMING END | Turn ID: {}", turn_id);
                info!("📤 Sending activityEnd for streaming audio (turn_id: {})", turn_id);
                messages.push(WsOut::RealtimeInput(serde_json::json!({ "activityEnd": {} })));
                // Note: We don't transition to Idle here or call complete_tracked_turn.
                // That happens when WsIn::GenerationComplete is received.
            } else {
                 info!("❌ Cannot end streaming - not in streaming state (state: {:?}) or state is inconsistent.", self.state);
                 return vec![];
            }
        }
        
        messages
    }

    fn start_video_turn(&mut self) -> Vec<WsOut> {
        let turn_id = self.start_new_tracked_turn();
        info!("🔄 Broker state transition: Idle -> CollectingVideoOnly (turn_id: {})", turn_id);
        self.state = State::CollectingVideoOnly { start: Instant::now(), turn_id };
        let mut messages = vec![];
        
        println!("📹 VIDEO TURN {} | Frames: {}", turn_id, self.recent_frames.len());
        
        info!("🎯 Starting video-only turn {} with {} frames", turn_id, self.recent_frames.len());

        info!("📤 Sending activityStart (video-only) to Gemini for turn_id {}", turn_id);
        messages.push(WsOut::RealtimeInput(serde_json::json!({ "activityStart": {} })));

        let frames_to_send = self.recent_frames.len() - self.frames_sent_in_turn;
        let frames_to_send = frames_to_send.min(MAX_FRAMES_PER_TURN);
        let start_idx = self.recent_frames.len().saturating_sub(frames_to_send);

        // Send video frames
        let frames_slice = &self.recent_frames[start_idx..];
        info!("📹 Sending {} video frames (video-only turn) to Gemini for turn_id {}", frames_slice.len(), turn_id);
        for (i, frame) in frames_slice.iter().enumerate() {
            let frame_size_kb = frame.jpeg.len() / 1024;
            let video_input = serde_json::json!({
                "video": {
                    "data": base64::engine::general_purpose::STANDARD.encode(&frame.jpeg),
                    "mimeType": "image/jpeg"
                }
            });
            debug!("📸 Frame {}: {} KB", i + 1, frame_size_kb);
            messages.push(WsOut::RealtimeInput(video_input));
        }

        self.frames_sent_in_turn = self.recent_frames.len();

        info!("📤 Sending activityEnd (video-only) to Gemini for turn_id {}", turn_id);
        messages.push(WsOut::RealtimeInput(serde_json::json!({
            "activityEnd": {}
        })));

        messages
    }
}