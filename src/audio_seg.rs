//! Real-time audio segmentation v2
//!
//! A redesigned architecture that addresses the fundamental timing issues in v1:
//! - Lock-free ring buffer for audio storage
//! - Decoupled VAD, ASR, and boundary decision pipelines
//! - Bounded worst-case latencies with skip-not-wait back-pressure
//! - Tri-stable state machine: Idle → Recording → Committing → Recording

use std::collections::{BTreeMap, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};
use webrtc_vad::{SampleRate, Vad, VadMode};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
use crate::media_event::Outgoing;

/// Reason why a segment was closed
#[derive(Debug, Clone, PartialEq)]
pub enum CloseReason {
    /// Closed due to silence
    Silence,
    /// Closed due to maximum length
    MaxLength,
    /// Closed due to ASR clause detection
    AsrClause,
}

/// A completed audio segment
#[derive(Debug, Clone)]
pub struct SegmentedTurn {
    pub id: u64,
    pub audio: Vec<i16>,
    pub close_reason: CloseReason,
    pub text: Option<String>,
}

/// Configuration for the segmenter
#[derive(Debug, Clone)]
pub struct SegConfig {
    /// Number of voiced frames to open a segment (4 frames ≈ 80ms)
    pub open_voiced_frames: usize,
    /// Silence duration to automatically close a segment (ms)
    pub close_silence_ms: u64,
    /// Maximum duration of a turn (ms)
    pub max_turn_ms: u64,
    /// Minimum number of tokens for a valid clause
    pub min_clause_tokens: usize,
    /// Interval between ASR polls during a turn (ms)
    pub asr_poll_ms: u64,
    /// Ring buffer capacity in samples (default: 20 seconds at 16kHz)
    pub ring_capacity: usize,
    /// ASR worker pool size
    pub asr_pool_size: usize,
    /// Maximum time to wait for ASR result before emitting without transcript
    pub asr_timeout_ms: u64,
}

impl Default for SegConfig {
    fn default() -> Self {
        Self {
            open_voiced_frames: 6,      // 120ms
            close_silence_ms: 300,      // 300ms
            max_turn_ms: 5000,          // 5 seconds for responsive interaction
            min_clause_tokens: 4,       // 4 tokens minimum
            asr_poll_ms: 250,           // 250ms ASR polling
            ring_capacity: 320_000,     // 20 seconds at 16kHz
            asr_pool_size: 2,           // 2 worker threads
            asr_timeout_ms: 2000,       // 2 second timeout
        }
    }
}

/// Lock-free ring buffer for audio samples
pub struct AudioRingBuffer {
    buffer: Vec<i16>,
    capacity: usize,
    write_pos: AtomicUsize,
    /// Global sample index (monotonically increasing)
    global_idx: AtomicUsize,
}

impl AudioRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0; capacity],
            capacity,
            write_pos: AtomicUsize::new(0),
            global_idx: AtomicUsize::new(0),
        }
    }

    /// Push a frame of samples, returns the global index of the first sample
    pub fn push_frame(&self, samples: &[i16]) -> usize {
        let start_global_idx = self.global_idx.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);
        
        // Copy samples into ring buffer
        for (i, &sample) in samples.iter().enumerate() {
            let pos = (write_pos + i) % self.capacity;
            // SAFETY: We're the only writer, and pos is always in bounds
            unsafe {
                // Use atomic store - this is safe because we're the only writer
                std::ptr::write_volatile(self.buffer.as_ptr().add(pos) as *mut i16, sample);
            }
        }
        
        // Update positions atomically
        let new_write_pos = (write_pos + samples.len()) % self.capacity;
        self.write_pos.store(new_write_pos, Ordering::Release);
        self.global_idx.store(start_global_idx + samples.len(), Ordering::Release);
        
        start_global_idx
    }

    /// Get a snapshot of samples for the given global index range
    /// Returns None if the range is no longer available in the ring
    pub fn get_range(&self, range: Range<usize>) -> Option<Vec<i16>> {
        let current_global = self.global_idx.load(Ordering::Acquire);
        let available_start = current_global.saturating_sub(self.capacity);
        
        // Check if range is still available
        if range.start < available_start || range.end > current_global {
            return None;
        }
        
        let write_pos = self.write_pos.load(Ordering::Acquire);
        let mut result = Vec::with_capacity(range.len());
        
        for global_idx in range {
            let ring_pos = (write_pos + self.capacity - (current_global - global_idx)) % self.capacity;
            // SAFETY: ring_pos is always in bounds due to the availability check above
            unsafe {
                result.push(*self.buffer.get_unchecked(ring_pos));
            }
        }
        
        Some(result)
    }

    /// Get current global index
    pub fn current_global_idx(&self) -> usize {
        self.global_idx.load(Ordering::Acquire)
    }
}

/// Metadata for a 20ms frame
#[derive(Debug, Clone)]
pub struct FrameMeta {
    pub timestamp: Instant,
    pub start_idx: usize,  // Global index in ring
    pub voiced: bool,
}

/// ASR proposal for clause boundary
#[derive(Debug, Clone)]
pub struct AsrProposal {
    pub clause_end_idx: usize,  // Global index
    pub text: String,
    pub confidence: f32,
}

/// Boundary detection event
#[derive(Debug, Clone)]
pub enum BoundaryEvent {
    SilenceClose(usize, usize),        // start_idx, end_idx
    MaxLenClose(usize, usize),         // start_idx, end_idx
    AsrClose(usize, usize, String),    // start_idx, end_idx, text
}

/// A committed segment waiting for emission
#[derive(Debug)]
pub struct SegmentCommit {
    pub id: u64,
    pub range: Range<usize>,
    pub reason: CloseReason,
    pub text: Option<String>,
    pub timestamp: Instant,
}

/// Frame classifier that runs VAD on incoming audio
pub struct FrameClassifier {
    vad: Vad,
    frame_queue: mpsc::Sender<FrameMeta>,
}

impl FrameClassifier {
    pub fn new() -> Result<(Self, mpsc::Receiver<FrameMeta>), Box<dyn std::error::Error>> {
        let vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::VeryAggressive);
        let (tx, rx) = mpsc::channel();
        
        Ok((Self {
            vad,
            frame_queue: tx,
        }, rx))
    }

    /// Classify a 20ms frame (320 samples)
    pub fn classify_frame(&mut self, samples: &[i16], global_idx: usize, timestamp: Instant) -> Result<(), Box<dyn std::error::Error>> {
        if samples.len() != 320 {
            return Err(format!("Expected 320 samples for 20ms frame, got {}", samples.len()).into());
        }

        let voiced = self.vad.is_voice_segment(samples).map_err(|_| "VAD error")?;
        
        let frame_meta = FrameMeta {
            timestamp,
            start_idx: global_idx,
            voiced,
        };

        if let Err(_) = self.frame_queue.send(frame_meta) {
            warn!("Frame queue full, dropping frame");
        }

        Ok(())
    }
}

/// States for the boundary detection FSM
#[derive(Debug, Clone, PartialEq)]
pub enum BoundaryState {
    Idle,
    Recording {
        seg_start_idx: usize,
        last_voice_idx: usize,
        started_at: Instant,
    },
    Committing {
        seg_start_idx: usize,
        last_voice_idx: usize,
        started_at: Instant,
    },
}

/// Finite state machine for boundary detection
pub struct BoundaryFSM {
    config: SegConfig,
    state: BoundaryState,
    voiced_score: f32,
    next_seg_id: u64,
    boundary_events: mpsc::Sender<BoundaryEvent>,
    asr_proposals: mpsc::Receiver<AsrProposal>,
}

impl BoundaryFSM {
    pub fn new(
        config: SegConfig,
        asr_proposals: mpsc::Receiver<AsrProposal>,
    ) -> (Self, mpsc::Receiver<BoundaryEvent>) {
        let (boundary_tx, boundary_rx) = mpsc::channel();
        
        (Self {
            config,
            state: BoundaryState::Idle,
            voiced_score: 0.0,
            next_seg_id: 1,
            boundary_events: boundary_tx,
            asr_proposals,
        }, boundary_rx)
    }

    /// Process a frame and potentially emit boundary events
    pub fn process_frame(&mut self, frame: &FrameMeta, current_global_idx: usize) {
        // Update voiced score with decay
        self.voiced_score = self.voiced_score * 0.75 + if frame.voiced { 1.0 } else { 0.0 };
        
        // Check for ASR proposals
        while let Ok(proposal) = self.asr_proposals.try_recv() {
            self.handle_asr_proposal(proposal, current_global_idx);
        }
        
        let now = Instant::now();
        
        match &self.state {
            BoundaryState::Idle => {
                // Check for opening condition
                if self.voiced_score >= 3.0 { // ~60ms of speech
                    let seg_start_idx = frame.start_idx.saturating_sub(8000); // 500ms pre-roll
                    debug!("Opening segment {} at idx {}", self.next_seg_id, seg_start_idx);
                    
                    self.state = BoundaryState::Recording {
                        seg_start_idx,
                        last_voice_idx: frame.start_idx,
                        started_at: now,
                    };
                }
            }
            
            BoundaryState::Recording { seg_start_idx, last_voice_idx, started_at } => {
                let mut new_last_voice = *last_voice_idx;
                if frame.voiced {
                    new_last_voice = frame.start_idx;
                }
                
                // Check closing conditions
                let elapsed = now.duration_since(*started_at);
                let silence_samples = current_global_idx.saturating_sub(new_last_voice);
                let silence_ms = (silence_samples * 1000) / 16000; // Convert to ms
                
                let close_event = if elapsed.as_millis() as u64 >= self.config.max_turn_ms {
                    Some(BoundaryEvent::MaxLenClose(*seg_start_idx, current_global_idx))
                } else if silence_ms >= self.config.close_silence_ms as usize {
                    Some(BoundaryEvent::SilenceClose(*seg_start_idx, current_global_idx))
                } else {
                    None
                };
                
                if let Some(event) = close_event {
                    debug!("Closing segment {} due to {:?}", self.next_seg_id, event);
                    let _ = self.boundary_events.send(event);
                    self.next_seg_id += 1;
                    self.state = BoundaryState::Idle;
                    self.voiced_score = 0.0;
                } else {
                    // Update state with new voice position
                    self.state = BoundaryState::Recording {
                        seg_start_idx: *seg_start_idx,
                        last_voice_idx: new_last_voice,
                        started_at: *started_at,
                    };
                }
            }
            
            BoundaryState::Committing { .. } => {
                // In committing state, check if we should start a new segment
                if self.voiced_score >= 3.0 {
                    let seg_start_idx = frame.start_idx.saturating_sub(1600); // 100ms pre-roll
                    self.state = BoundaryState::Recording {
                        seg_start_idx,
                        last_voice_idx: frame.start_idx,
                        started_at: now,
                    };
                }
            }
        }
    }

    fn handle_asr_proposal(&mut self, proposal: AsrProposal, current_global_idx: usize) {
        // Only handle ASR proposals if we're in Recording state
        if let BoundaryState::Recording { seg_start_idx, .. } = &self.state {
            // Validate that the proposal is for current segment and represents a valid clause
            if proposal.clause_end_idx > *seg_start_idx && 
               proposal.clause_end_idx < current_global_idx &&
               self.is_valid_clause(&proposal.text) {
                
                debug!("ASR clause detected: '{}' ending at {}", proposal.text, proposal.clause_end_idx);
                let event = BoundaryEvent::AsrClose(*seg_start_idx, proposal.clause_end_idx, proposal.text);
                let _ = self.boundary_events.send(event);
                self.next_seg_id += 1;
                
                // Transition to committing state to handle remaining audio
                self.state = BoundaryState::Committing {
                    seg_start_idx: proposal.clause_end_idx,
                    last_voice_idx: proposal.clause_end_idx,
                    started_at: Instant::now(),
                };
            }
        }
    }

    fn is_valid_clause(&self, text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return false;
        }

        // Always accept explicit sentence enders
        if t.ends_with(['.', '?', '!', ';']) {
            return true;
        }

        // Token threshold (≈ words)
        let tokens = t.split_whitespace().count();
        if tokens >= self.config.min_clause_tokens {
            return true;
        }

        // Speech disfluency markers
        matches!(t.chars().last().unwrap_or(' '), ',' | '-')
            || t.ends_with(" and")
            || t.ends_with(" but")
            || t.contains(" because ")
    }

    pub fn get_current_segment_range(&self) -> Option<Range<usize>> {
        match &self.state {
            BoundaryState::Recording { seg_start_idx, .. } => Some(*seg_start_idx..usize::MAX),
            BoundaryState::Committing { seg_start_idx, .. } => Some(*seg_start_idx..usize::MAX),
            BoundaryState::Idle => None,
        }
    }
    
    pub fn get_state(&self) -> &BoundaryState {
        &self.state
    }
}

/// Request to ASR worker pool
#[derive(Debug)]
struct AsrRequest {
    id: u64,
    audio: Vec<i16>,
    global_range: Range<usize>,
}

/// ASR worker pool for semantic analysis
pub struct AsrWorkerPool {
    workers: Vec<std::thread::JoinHandle<()>>,
    request_tx: mpsc::Sender<AsrRequest>,
    shutdown: Arc<AtomicBool>,
}

impl AsrWorkerPool {
    pub fn new(
        config: &SegConfig,
        whisper_model: Option<&std::path::Path>,
        proposal_tx: mpsc::Sender<AsrProposal>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (request_tx, request_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        
        let mut workers = Vec::new();
        
        if let Some(model_path) = whisper_model {
            let ctx = Arc::new(WhisperContext::new_with_params(
                model_path.to_str().unwrap(),
                WhisperContextParameters::default(),
            )?);
            
            // Use shared receiver for multiple workers
            let request_rx = Arc::new(std::sync::Mutex::new(request_rx));
            
            for worker_id in 0..config.asr_pool_size {
                let ctx_clone = ctx.clone();
                let request_rx_clone = request_rx.clone();
                let proposal_tx_clone = proposal_tx.clone();
                let shutdown_clone = shutdown.clone();
                let min_tokens = config.min_clause_tokens;
                
                let worker = std::thread::spawn(move || {
                    asr_worker_shared(worker_id, request_rx_clone, proposal_tx_clone, ctx_clone, shutdown_clone, min_tokens);
                });
                
                workers.push(worker);
            }
        }
        
        Ok(Self {
            workers,
            request_tx,
            shutdown,
        })
    }

    /// Submit audio for ASR processing (non-blocking)
    pub fn submit(&self, id: u64, audio: Vec<i16>, global_range: Range<usize>) -> bool {
        let request = AsrRequest { id, audio, global_range };
        self.request_tx.send(request).is_ok()
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

impl Drop for AsrWorkerPool {
    fn drop(&mut self) {
        self.shutdown();
        // Don't wait for workers to finish - they'll detect shutdown and exit
    }
}

/// ASR worker function with shared receiver
fn asr_worker_shared(
    worker_id: usize,
    request_rx: Arc<std::sync::Mutex<mpsc::Receiver<AsrRequest>>>,
    proposal_tx: mpsc::Sender<AsrProposal>,
    ctx: Arc<WhisperContext>,
    shutdown: Arc<AtomicBool>,
    min_tokens: usize,
) {
    debug!("ASR worker {} started", worker_id);
    
    while !shutdown.load(Ordering::Acquire) {
        // Wait for request with timeout
        let request = match request_rx.lock().unwrap().recv_timeout(Duration::from_millis(100)) {
            Ok(req) => req,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        
        debug!("Worker {} processing {} samples", worker_id, request.audio.len());
        
        // Create Whisper state
        let mut state = match ctx.create_state() {
            Ok(state) => state,
            Err(e) => {
                error!("Worker {} failed to create Whisper state: {}", worker_id, e);
                continue;
            }
        };
        
        // Set up parameters
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_token_timestamps(true);
        
        // Convert to f32 and ensure minimum length
        let mut audio: Vec<f32> = request.audio.iter().map(|&s| s as f32 / 32768.0).collect();
        if audio.len() < 16080 {
            audio.resize(16080, 0.0);
        }
        
        // Run inference
        if let Err(e) = state.full(params, &audio) {
            error!("Worker {} inference failed: {}", worker_id, e);
            continue;
        }
        
        // Extract clause boundaries
        if let Some(proposal) = extract_clause_boundary(&state, &request.global_range, min_tokens) {
            if let Err(_) = proposal_tx.send(proposal) {
                warn!("Worker {} proposal queue full", worker_id);
            }
        }
    }
    
    debug!("ASR worker {} shutting down", worker_id);
}

/// Extract the first valid clause boundary from Whisper results
fn extract_clause_boundary(
    state: &whisper_rs::WhisperState,
    global_range: &Range<usize>,
    min_tokens: usize,
) -> Option<AsrProposal> {
    let n_segments = state.full_n_segments().unwrap_or(0);
    if n_segments == 0 {
        return None;
    }
    
    let full_text = state.full_get_segment_text(0).unwrap_or_default().to_string();
    if full_text.trim().is_empty() {
        return None;
    }
    
    // Find first valid clause boundary
    if let Ok(n_tokens) = state.full_n_tokens(0) {
        let mut current_text = String::new();
        
        for i in 0..n_tokens {
            if let (Ok(token_text), Ok(token_data)) = 
                (state.full_get_token_text(0, i), state.full_get_token_data(0, i)) {
                
                if !token_text.starts_with('[') {
                    current_text.push_str(&token_text);
                }
                
                if is_valid_clause_simple(&current_text, min_tokens) {
                    // Convert centiseconds to global sample index
                    let time_offset_samples = (token_data.t1 as f32 * 0.01 * 16000.0) as usize;
                    let clause_end_idx = global_range.start + time_offset_samples;
                    
                    if clause_end_idx < global_range.end {
                        return Some(AsrProposal {
                            clause_end_idx,
                            text: current_text.trim().to_string(),
                            confidence: 1.0, // TODO: extract actual confidence
                        });
                    }
                }
            }
        }
    }
    
    None
}

/// Simple clause validation (reused from original)
fn is_valid_clause_simple(text: &str, min_tokens: usize) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }

    // Always accept explicit sentence enders
    if t.ends_with(['.', '?', '!', ';']) {
        return true;
    }

    // Token threshold
    let tokens = t.split_whitespace().count();
    if tokens >= min_tokens {
        return true;
    }

    false

    // // Disfluencies
    // matches!(t.chars().last().unwrap_or(' '), ',' | '-')
    //     || t.ends_with(" and")
    //     || t.ends_with(" but")
    //     || t.contains(" because ")
}

/// Segment emitter that converts commits to final segments
pub struct SegmentEmitter {
    config: SegConfig,
    ring_buffer: Arc<AudioRingBuffer>,
    pending_commits: BTreeMap<u64, SegmentCommit>,
    next_emit_id: u64,
    output_queue: VecDeque<SegmentedTurn>,
}

impl SegmentEmitter {
    pub fn new(config: SegConfig, ring_buffer: Arc<AudioRingBuffer>) -> Self {
        Self {
            config,
            ring_buffer,
            pending_commits: BTreeMap::new(),
            next_emit_id: 1,
            output_queue: VecDeque::new(),
        }
    }

    /// Process a boundary event and create a commit
    pub fn process_boundary_event(&mut self, event: BoundaryEvent, seg_id: u64) {
        let (start_idx, end_idx, reason, text) = match event {
            BoundaryEvent::SilenceClose(start_idx, end_idx) => (start_idx, end_idx, CloseReason::Silence, None),
            BoundaryEvent::MaxLenClose(start_idx, end_idx) => (start_idx, end_idx, CloseReason::MaxLength, None),
            BoundaryEvent::AsrClose(start_idx, end_idx, text) => (start_idx, end_idx, CloseReason::AsrClause, Some(text)),
        };

        let commit = SegmentCommit {
            id: seg_id,
            range: start_idx..end_idx,
            reason,
            text,
            timestamp: Instant::now(),
        };

        self.pending_commits.insert(seg_id, commit);
        self.try_emit_ready_segments();
    }

    /// Add transcript to existing commit
    pub fn add_transcript(&mut self, seg_id: u64, text: String) {
        if let Some(commit) = self.pending_commits.get_mut(&seg_id) {
            if commit.text.is_none() {
                commit.text = Some(text);
                self.try_emit_ready_segments();
            }
        }
    }

    /// Try to emit segments that are ready
    fn try_emit_ready_segments(&mut self) {
        while let Some(commit) = self.pending_commits.get(&self.next_emit_id) {
            // Check if we should wait for transcript
            let should_wait = commit.text.is_none() && 
                commit.reason != CloseReason::AsrClause &&
                commit.timestamp.elapsed().as_millis() < self.config.asr_timeout_ms as u128;

            if should_wait {
                break;
            }

            // Remove from pending and convert to segment
            let commit = self.pending_commits.remove(&self.next_emit_id).unwrap();
            
            if let Some(pcm) = self.ring_buffer.get_range(commit.range) {
                let pcm_len = pcm.len();
                let segment = SegmentedTurn {
                    id: self.next_emit_id,
                    audio: pcm,
                    close_reason: commit.reason,
                    text: commit.text,
                };
                
                self.output_queue.push_back(segment);
                debug!("Emitted segment {} with {} samples", self.next_emit_id, pcm_len);
            } else {
                warn!("Failed to get audio for segment {} - range no longer available", self.next_emit_id);
            }
            
            self.next_emit_id += 1;
        }
    }

    /// Get next ready segment
    pub fn pop_segment(&mut self) -> Option<SegmentedTurn> {
        self.try_emit_ready_segments();
        self.output_queue.pop_front()
    }
}

/// Main v2 audio segmenter
pub struct AudioSegmenter {
    config: SegConfig,
    ring_buffer: Arc<AudioRingBuffer>,
    frame_classifier: FrameClassifier,
    frame_receiver: mpsc::Receiver<FrameMeta>,
    boundary_fsm: BoundaryFSM,
    boundary_receiver: mpsc::Receiver<BoundaryEvent>,
    asr_pool: AsrWorkerPool,
    emitter: SegmentEmitter,
    last_asr_poll: Instant,
    next_asr_id: u64,
    /// Track the last index submitted to ASR to avoid duplicate processing
    last_asr_submit_idx: Option<usize>,
    /// Track the previous FSM state to detect transitions
    prev_fsm_state: Option<BoundaryState>,
    /// Sender for outgoing websocket messages
    outgoing_tx: Option<mpsc::Sender<Outgoing>>,
    /// Global turn ID generator (shared across all producers)
    turn_id_generator: Arc<AtomicU64>,
    /// Current turn ID for this segmenter
    current_turn_id: Option<u64>,
}

impl AudioSegmenter {
    pub fn new(
        config: SegConfig,
        whisper_model: Option<&std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let ring_buffer = Arc::new(AudioRingBuffer::new(config.ring_capacity));
        
        let (frame_classifier, frame_receiver) = FrameClassifier::new()?;
        
        // Create proposal channel for ASR → FSM communication
        let (asr_proposal_tx, asr_proposal_rx) = mpsc::channel();
        let asr_pool = AsrWorkerPool::new(&config, whisper_model, asr_proposal_tx)?;
        let (boundary_fsm, boundary_receiver) = BoundaryFSM::new(config.clone(), asr_proposal_rx);
        
        let emitter = SegmentEmitter::new(config.clone(), ring_buffer.clone());
        
        Ok(Self {
            config,
            ring_buffer,
            frame_classifier,
            frame_receiver,
            boundary_fsm,
            boundary_receiver,
            asr_pool,
            emitter,
            last_asr_poll: Instant::now(),
            next_asr_id: 1,
            last_asr_submit_idx: None,
            prev_fsm_state: None,
            outgoing_tx: None,
            turn_id_generator: Arc::new(AtomicU64::new(0)),
            current_turn_id: None,
        })
    }

    
    /// Set the outgoing websocket message sender and turn ID generator
    pub fn set_outgoing_sender(&mut self, tx: mpsc::Sender<Outgoing>, turn_id_gen: Arc<AtomicU64>) {
        self.outgoing_tx = Some(tx);
        self.turn_id_generator = turn_id_gen;
    }

    /// Process a 20ms chunk (320 samples at 16kHz)
    pub fn push_chunk(&mut self, chunk: &[i16]) -> Option<SegmentedTurn> {
        if chunk.len() != 320 {
            warn!("Expected 320 samples, got {}", chunk.len());
            return None;
        }

        let timestamp = Instant::now();
        let chunk_start_idx = self.ring_buffer.push_frame(chunk);
        
        // Store current FSM state before processing
        let prev_state = self.prev_fsm_state.clone();
        
        // Process the 20ms frame directly for VAD
        let _ = self.frame_classifier.classify_frame(chunk, chunk_start_idx, timestamp);
        
        // Process frame events
        while let Ok(frame_meta) = self.frame_receiver.try_recv() {
            let current_global_idx = self.ring_buffer.current_global_idx();
            self.boundary_fsm.process_frame(&frame_meta, current_global_idx);
        }
        
        // Check for state transitions and emit outgoing events
        let current_state = self.boundary_fsm.get_state();
        
        // Send events via outgoing channel if available
        if let Some(ref tx) = self.outgoing_tx {
            // Check if we just opened a segment (Idle -> Recording)
            if matches!(prev_state, Some(BoundaryState::Idle) | None) && 
               matches!(current_state, BoundaryState::Recording { .. }) {
                // Generate new turn ID
                let turn_id = self.turn_id_generator.fetch_add(1, Ordering::SeqCst);
                self.current_turn_id = Some(turn_id);
                let _ = tx.send(Outgoing::ActivityStart(turn_id));
            }
            
            // Always send the audio chunk if we're recording
            if let Some(turn_id) = self.current_turn_id {
                if matches!(current_state, BoundaryState::Recording { .. } | BoundaryState::Committing { .. }) {
                    let pcm_bytes = i16_slice_to_u8(chunk);
                    let _ = tx.send(Outgoing::AudioChunk(pcm_bytes.to_vec(), turn_id));
                }
            }
        }
        
        // Update previous state
        self.prev_fsm_state = Some(current_state.clone());
        
        // Process boundary events
        while let Ok(boundary_event) = self.boundary_receiver.try_recv() {
            let seg_id = self.next_asr_id;
            self.next_asr_id += 1;
            
            // Emit end event when segment closes
            if let Some(ref tx) = self.outgoing_tx {
                if let Some(turn_id) = self.current_turn_id {
                    let _ = tx.send(Outgoing::ActivityEnd(turn_id));
                }
                self.current_turn_id = None;
            }
            
            self.emitter.process_boundary_event(boundary_event, seg_id);
        }
        
        // Poll ASR if needed
        if timestamp.duration_since(self.last_asr_poll).as_millis() >= self.config.asr_poll_ms as u128 {
            self.poll_asr();
            self.last_asr_poll = timestamp;
        }
        
        // Return any ready segments
        self.emitter.pop_segment()
    }
    
    fn poll_asr(&mut self) {
        if let Some(seg_range) = self.boundary_fsm.get_current_segment_range() {
            let current_idx = self.ring_buffer.current_global_idx();
            let poll_end = current_idx;
            let poll_start = seg_range.start;
            
            // Check if this is a new segment (segment boundary changed)
            let is_new_segment = self.last_asr_submit_idx
                .map(|last_idx| last_idx < poll_start)
                .unwrap_or(true);
            
            if is_new_segment {
                // Reset tracking for new segment
                self.last_asr_submit_idx = Some(poll_start);
            }
            
            // Only submit new audio that hasn't been processed yet
            let actual_start = self.last_asr_submit_idx.unwrap_or(poll_start);
            
            // Only poll if we have enough NEW audio (at least 0.5 seconds of new data)
            if poll_end > actual_start + 8000 {
                if let Some(audio) = self.ring_buffer.get_range(poll_start..poll_end) {
                    let submitted = self.asr_pool.submit(self.next_asr_id, audio, poll_start..poll_end);
                    if submitted {
                        debug!("Submitted ASR request {} for range {}..{} (full segment)", self.next_asr_id, poll_start, poll_end);
                        // Update tracking to avoid reprocessing
                        self.last_asr_submit_idx = Some(poll_end);
                    }
                }
            }
        } else {
            // No active segment, reset tracking
            self.last_asr_submit_idx = None;
        }
    }

    /// Force close current segment
    pub fn force_close(&mut self) -> Option<SegmentedTurn> {
        // Implementation would force FSM to emit current segment
        self.emitter.pop_segment()
    }
}

/// Convert i16 slice to mutable u8 slice for audio capture
pub fn i16_to_u8_mut(buffer: &mut [i16]) -> &mut [u8] {
    unsafe {
        std::slice::from_raw_parts_mut(
            buffer.as_mut_ptr() as *mut u8,
            buffer.len() * 2,
        )
    }
}

/// Convert i16 slice to u8 slice
pub fn i16_slice_to_u8(slice: &[i16]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            slice.as_ptr() as *const u8,
            slice.len() * 2,
        )
    }
}

/// Send completed turn to Gemini
pub async fn send_turn_to_gemini(
    turn: &SegmentedTurn,
    gemini_client: &mut crate::gemini_client::GeminiClient,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pcm_bytes = i16_slice_to_u8(&turn.audio);
    const MAX_WEBSOCKET: usize = 1_000_000; // 1 MiB
    const FIRST_MAX: usize = 256_000; // 0.25 MiB

    // First slice - set activity_start=true
    let first_chunk_size = std::cmp::min(FIRST_MAX, pcm_bytes.len());
    gemini_client
        .send_audio_with_activity(&pcm_bytes[..first_chunk_size], true, false, false)
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

    // Middle slices (if any)
    if pcm_bytes.len() > first_chunk_size {
        for chunk in pcm_bytes[first_chunk_size..].chunks(MAX_WEBSOCKET) {
            gemini_client
                .send_audio_with_activity(chunk, false, false, false)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
        }
    }

    // Final slice - set activity_end=true
    gemini_client
        .send_audio_with_activity(&[], false, false, true)
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

    debug!("Sent turn {} to Gemini: {} samples, reason: {:?}", 
           turn.id, turn.audio.len(), turn.close_reason);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_ring_buffer_basic() {
        let ring = AudioRingBuffer::new(1000);
        
        // Push some data
        let samples1 = vec![1, 2, 3, 4, 5];
        let idx1 = ring.push_frame(&samples1);
        assert_eq!(idx1, 0);
        
        let samples2 = vec![6, 7, 8, 9, 10];
        let idx2 = ring.push_frame(&samples2);
        assert_eq!(idx2, 5);
        
        // Get ranges
        let range1 = ring.get_range(0..5).unwrap();
        assert_eq!(range1, samples1);
        
        let range2 = ring.get_range(5..10).unwrap();
        assert_eq!(range2, samples2);
        
        let combined = ring.get_range(0..10).unwrap();
        assert_eq!(combined, [samples1, samples2].concat());
    }

    #[test]
    fn test_ring_buffer_wraparound() {
        let ring = AudioRingBuffer::new(10);
        
        // Fill beyond capacity
        let samples1 = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let idx1 = ring.push_frame(&samples1);
        assert_eq!(idx1, 0);
        
        let samples2 = vec![9, 10, 11, 12, 13];
        let idx2 = ring.push_frame(&samples2);
        assert_eq!(idx2, 8);
        
        // Should be able to get recent data but not old data
        let recent = ring.get_range(8..13).unwrap();
        assert_eq!(recent, samples2);
        
        // Old data should be unavailable
        assert!(ring.get_range(0..5).is_none());
    }
    
    #[test]
    fn test_clause_validation() {
        assert!(is_valid_clause_simple("This is a sentence.", 4));
        assert!(is_valid_clause_simple("Is this a question?", 4));
        assert!(is_valid_clause_simple("This has enough tokens to pass", 4));
        assert!(!is_valid_clause_simple("Too short", 4));
        assert!(is_valid_clause_simple("I think,", 4));
        assert!(is_valid_clause_simple("Going home and", 4));
    }

    #[test]
    fn test_config_defaults() {
        let config = SegConfig::default();
        
        // Test that defaults are reasonable for real-time operation
        assert!(config.open_voiced_frames >= 3); // At least 60ms
        assert!(config.close_silence_ms >= 200); // At least 200ms silence
        assert!(config.max_turn_ms <= 10000);    // No more than 10s turns
        assert!(config.asr_poll_ms <= 500);      // Poll at least every 500ms
        assert!(config.ring_capacity >= 160000); // At least 10s buffer
    }

    #[test]
    fn test_boundary_fsm_state_transitions() {
        let config = SegConfig::default();
        let (_, asr_rx) = std::sync::mpsc::channel();
        let (mut fsm, _boundary_rx) = BoundaryFSM::new(config, asr_rx);
        
        // Should start in Idle state
        assert_eq!(fsm.state, BoundaryState::Idle);
        
        // Create a voiced frame
        let frame = FrameMeta {
            timestamp: Instant::now(),
            start_idx: 0,
            voiced: true,
        };
        
        // Process multiple voiced frames to trigger opening
        for i in 0..10 {
            let mut test_frame = frame.clone();
            test_frame.start_idx = i * 320; // 20ms apart
            fsm.process_frame(&test_frame, (i + 1) * 320);
        }
        
        // Should transition to Recording state
        assert!(matches!(fsm.state, BoundaryState::Recording { .. }));
    }

    #[test]
    fn test_latency_budget_ring_buffer() {
        let ring = AudioRingBuffer::new(320_000); // 20 second buffer
        let frame_size = 1600; // 100ms
        
        let start = Instant::now();
        
        // Simulate 1 second of audio processing
        for i in 0..10 {
            let samples = vec![i as i16; frame_size];
            ring.push_frame(&samples);
            
            // Each push should be very fast
            let elapsed = start.elapsed();
            assert!(elapsed < Duration::from_millis(1)); // < 1ms per push
        }
        
        // Getting ranges should also be fast
        let get_start = Instant::now();
        let _data = ring.get_range(0..16000); // 1 second of audio
        let get_elapsed = get_start.elapsed();
        assert!(get_elapsed < Duration::from_millis(10)); // < 10ms to extract 1s
    }

    #[test]
    fn test_segment_emitter_ordering() {
        let mut config = SegConfig::default();
        config.asr_timeout_ms = 0; // Don't wait for ASR results in test
        let ring = Arc::new(AudioRingBuffer::new(10000));
        let mut emitter = SegmentEmitter::new(config, ring.clone());
        
        // Add some test audio to ring
        let audio1 = vec![1i16; 1600];
        let audio2 = vec![2i16; 1600]; 
        ring.push_frame(&audio1);
        ring.push_frame(&audio2);
        
        // Create out-of-order boundary events
        // Segment 2: from 1600 to 3200 (next 1600 samples)
        emitter.process_boundary_event(
            BoundaryEvent::SilenceClose(1600, 3200),
            2, // segment 2
        );
        // Segment 1: from 0 to 1600 (first 1600 samples)
        emitter.process_boundary_event(
            BoundaryEvent::SilenceClose(0, 1600),
            1, // segment 1  
        );
        
        // Should emit segment 1 first
        let seg1 = emitter.pop_segment();
        assert!(seg1.is_some(), "Expected segment 1 but got None");
        let seg1 = seg1.unwrap();
        assert_eq!(seg1.audio.len(), 1600);
        
        // Then segment 2
        let seg2 = emitter.pop_segment();
        assert!(seg2.is_some());
        let seg2 = seg2.unwrap();
        assert_eq!(seg2.audio.len(), 1600);
        
        // No more segments
        assert!(emitter.pop_segment().is_none());
    }
}