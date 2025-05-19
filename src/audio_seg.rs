//! Audio segmentation module
//!
//! This module provides semantic audio segmentation using WebRTC VAD and Whisper ASR.
//! It processes incoming audio chunks to detect speech boundaries and uses Whisper
//! for semantic segmentation based on the content of speech.

use crate::gemini_client;
use std::path::Path;
use std::time::Instant;
use tracing::{debug, error, info, span, warn, Level};
use webrtc_vad::{SampleRate, Vad, VadMode};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperState};

/// Error type for audio segmentation operations
#[derive(Debug, thiserror::Error)]
pub enum SegmentationError {
    #[error("VAD error: {0}")]
    VadError(String),

    #[error("Whisper error: {0}")]
    WhisperError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}

pub type Result<T> = std::result::Result<T, SegmentationError>;

/// Configuration for audio segmentation
#[derive(Debug, Clone)]
pub struct SegConfig {
    /// Number of voiced 20ms frames required to open a turn
    pub open_voiced_frames: usize,

    /// Milliseconds of silence required to close a turn
    pub close_silence_ms: u64,

    /// Maximum duration of a turn in milliseconds
    pub max_turn_ms: u64,

    /// Whether to use Whisper for semantic gating
    pub whisper_gate: bool,

    /// Minimum number of tokens in a clause to trigger early closure
    pub clause_tokens: usize,

    /// Interval between Whisper runs in milliseconds
    pub whisper_interval_ms: u64,
}

impl Default for SegConfig {
    fn default() -> Self {
        Self {
            open_voiced_frames: 6,    // 120ms of speech to open
            close_silence_ms: 400,    // 400ms silence to close
            max_turn_ms: 8000,        // 8 seconds max turn
            whisper_gate: true,       // Use semantic gating
            clause_tokens: 12,        // 12 tokens is roughly a short sentence
            whisper_interval_ms: 500, // Run Whisper every 500ms
        }
    }
}

/// A completed speech segment/turn with pcm data and optional transcription
#[derive(Debug, Clone)]
pub struct SegmentedTurn {
    /// The PCM audio data (16-bit mono at 16kHz)
    pub pcm: Vec<i16>,

    /// Optional partial transcription from Whisper
    pub partial_text: Option<String>,

    /// The reason this turn was closed
    pub close_reason: CloseReason,
}

/// Reason a turn was closed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// Closed due to silence threshold
    Silence,

    /// Closed due to maximum turn length
    MaxLength,

    /// Closed due to semantic boundary detection
    WhisperClause,
}

/// Audio segmenter that processes incoming audio chunks and produces turns
pub struct AudioSegmenter {
    /// WebRTC Voice Activity Detector
    vad: Vad,

    /// Configuration
    cfg: SegConfig,

    /// Whisper context (loaded model)
    whisper_ctx: Option<WhisperContext>,

    /// Current Whisper state for incremental decoding (owned by the context)
    whisper_state: Option<Box<WhisperState>>,

    /// Buffer of audio samples for the current turn
    buffer: Vec<i16>,

    /// Counter for voiced frames in the current segment
    voiced_frames: usize,

    /// Counter for silent frames in the current segment (in ms)
    silent_ms: u64,

    /// Timestamp when the current turn was opened
    opened_at: Option<Instant>,

    /// Timestamp of the last Whisper run
    last_whisper_run: Option<Instant>,
}

impl AudioSegmenter {
    /// Create a new AudioSegmenter with the given configuration and optional Whisper model
    pub fn new(cfg: SegConfig, whisper_path: Option<&Path>) -> Result<Self> {
        // 16 kHz, very aggressive mode for more sensitive voice detection
        let vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::VeryAggressive);

        // Try to load Whisper model if path is provided and whisper_gate is enabled
        let (whisper_ctx, whisper_state) = if cfg.whisper_gate && whisper_path.is_some() {
            let path = whisper_path.unwrap();
            info!("Loading Whisper model from {}", path.display());

            match WhisperContext::new_with_params(path.to_str().unwrap(), Default::default()) {
                Ok(ctx) => {
                    // Create initial state
                    match ctx.create_state() {
                        Ok(state) => (Some(ctx), Some(Box::new(state))),
                        Err(e) => {
                            warn!("Failed to create Whisper state: {}", e);
                            (Some(ctx), None)
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to load Whisper model: {}. Falling back to VAD-only mode.",
                        e
                    );
                    (None, None)
                }
            }
        } else {
            // Whisper not enabled or no path provided
            (None, None)
        };

        Ok(Self {
            vad,
            cfg,
            whisper_ctx,
            whisper_state,
            buffer: Vec::with_capacity(16000 * 8), // Pre-allocate buffer for 8 seconds
            voiced_frames: 0,
            silent_ms: 0,
            opened_at: None,
            last_whisper_run: None,
        })
    }

    /// Process a chunk of PCM audio and potentially return a completed turn
    ///
    /// Each chunk should be 100ms of 16kHz PCM audio (1600 samples).
    /// Returns Some(SegmentedTurn) when a turn is complete.
    pub fn push_chunk(&mut self, pcm_100ms: &[i16]) -> Option<SegmentedTurn> {
        let _span = span!(Level::DEBUG, "segment.push_chunk").entered();

        // Ensure the chunk is the right size
        if pcm_100ms.len() != 1600 {
            warn!(
                "Expected 100ms chunk (1600 samples), got {} samples",
                pcm_100ms.len()
            );
            // Try to process anyway if we have at least a few samples
            if pcm_100ms.len() < 20 {
                return None;
            }
        }

        // Process through WebRTC VAD
        let mut is_voiced = false;

        // Split into 20ms chunks (320 samples each) and process with VAD
        for chunk in pcm_100ms.chunks(320) {
            if chunk.len() == 320 {
                // Only process full chunks
                match self.vad.is_voice_segment(chunk) {
                    Ok(voiced) => {
                        if voiced {
                            is_voiced = true;
                            self.voiced_frames += 1;
                            self.silent_ms = 0; // Reset silence counter when voice detected
                        }
                    }
                    Err(_) => {
                        error!("VAD error: invalid frame length {}", chunk.len());
                    }
                }
            }
        }

        // If not voiced, increment silence counter
        if !is_voiced {
            // 100ms chunk
            self.silent_ms += 100;
        }

        // Check if we need to open a new turn
        if self.opened_at.is_none() && self.voiced_frames >= self.cfg.open_voiced_frames {
            debug!(
                "Opening new turn after {} voiced frames",
                self.voiced_frames
            );
            self.opened_at = Some(Instant::now());
            self.buffer.clear();
            // Reset whisper state if using incremental decoding
            if let (Some(ctx), Some(_)) = (&self.whisper_ctx, &self.whisper_state) {
                match ctx.create_state() {
                    Ok(state) => self.whisper_state = Some(Box::new(state)),
                    Err(e) => error!("Failed to reset Whisper state: {}", e),
                }
            }
            self.last_whisper_run = None;
        }

        // If turn is open, add samples to buffer
        if let Some(opened_at) = self.opened_at {
            // Add samples to buffer
            self.buffer.extend_from_slice(pcm_100ms);

            // Check for turn closure conditions

            // 1. Silence threshold
            if self.silent_ms >= self.cfg.close_silence_ms {
                debug!("Closing turn due to silence: {}ms", self.silent_ms);
                return self.finalize_turn(CloseReason::Silence);
            }

            // 2. Maximum turn length
            let turn_duration = opened_at.elapsed().as_millis() as u64;
            if turn_duration >= self.cfg.max_turn_ms {
                debug!("Closing turn due to max length: {}ms", turn_duration);
                return self.finalize_turn(CloseReason::MaxLength);
            }

            // 3. Semantic boundary (if Whisper is enabled)
            if self.cfg.whisper_gate
                && self.whisper_ctx.is_some()
                && self.whisper_state.is_some()
                && is_voiced
            {
                // Only run Whisper periodically to avoid CPU overload
                let should_run_whisper = match self.last_whisper_run {
                    Some(last_run) => {
                        last_run.elapsed().as_millis() as u64 >= self.cfg.whisper_interval_ms
                    }
                    None => true,
                };

                if should_run_whisper {
                    self.last_whisper_run = Some(Instant::now());

                    // Get partial transcription from Whisper
                    match self.run_whisper() {
                        Ok(Some(text)) => {
                            debug!("Whisper partial: '{}'", text);

                            // If we have enough audio and any reasonable text...
                            if !text.trim().is_empty() {
                                // Check for valid clauses or token counts
                                let is_clause = is_valid_clause(&text);
                                let has_enough_tokens =
                                    token_count(&text) >= self.cfg.clause_tokens;
                                let buffer_duration = self.buffer.len() as f32 / 16000.0;
                                let has_enough_audio = buffer_duration >= 1.0; // At least 1 second

                                if (is_clause || has_enough_tokens) && has_enough_audio {
                                    debug!(
                                        "Closing turn due to semantic clause detected: '{}'",
                                        text
                                    );
                                    // IMPORTANT: Reset silence counter since we're explicitly not waiting for silence
                                    self.silent_ms = 0;
                                    return self.finalize_turn_with_text(
                                        CloseReason::WhisperClause,
                                        Some(text),
                                    );
                                }
                            }
                        }
                        Ok(None) => {
                            // Even without transcription, segment if we have enough audio
                            let buffer_duration = self.buffer.len() as f32 / 16000.0;
                            if buffer_duration >= 2.5 && turn_duration >= self.cfg.max_turn_ms / 2 {
                                debug!("Closing turn due to sufficient audio without transcription ({:.1}s)", buffer_duration);
                                self.silent_ms = 0;
                                return self.finalize_turn(CloseReason::WhisperClause);
                            }
                        }
                        Err(e) => error!("Whisper error: {}", e),
                    }
                }
            }
        }

        None
    }

    /// Run Whisper on the current buffer and return partial transcription
    fn run_whisper(&mut self) -> Result<Option<String>> {
        if let (Some(_ctx), Some(state)) = (&self.whisper_ctx, &mut self.whisper_state) {
            // Set up parameters for more aggressive recognition
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

            // English-only, single segment, no special tokens
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_token_timestamps(false);
            params.set_language(Some("en"));
            params.set_translate(false);
            params.set_no_context(true);
            params.set_single_segment(true);

            // Much more permissive settings to accept more transcriptions
            params.set_duration_ms(0); // No minimum duration
            params.set_entropy_thold(10.0); // Very high entropy threshold (accept almost anything)
            params.set_logprob_thold(-5.0); // Very permissive logprob threshold (accept low confidence)
            params.set_no_speech_thold(0.1); // Very low threshold for speech detection

            // Run inference on the buffer
            // Convert i16 samples to f32 samples for Whisper
            let f32_samples: Vec<f32> = self.buffer.iter().map(|&s| s as f32 / 32768.0).collect();

            // Run inference
            match state.full(params, &f32_samples) {
                Ok(n_segments) => {
                    // Check if we have any segments
                    if n_segments > 0 {
                        // Get the text from the first segment
                        return match state.full_get_segment_text(0) {
                            Ok(text) => {
                                Ok(Some(text))
                            }
                            Err(e) => {
                                error!("Failed to get segment text: {}", e);
                                Err(SegmentationError::WhisperError(e.to_string()))
                            }
                        }
                    }

                    Ok(None)
                }
                Err(e) => {
                    error!("Whisper inference error: {}", e);
                    Err(SegmentationError::WhisperError(e.to_string()))
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Finalize the current turn and reset state
    fn finalize_turn(&mut self, reason: CloseReason) -> Option<SegmentedTurn> {
        // Try one last Whisper pass to capture any text before finalizing
        let mut final_text = None;

        if self.whisper_ctx.is_some() && self.whisper_state.is_some() && !self.buffer.is_empty() {
            debug!("Running final Whisper pass before closing turn");
            if let Ok(Some(text)) = self.run_whisper() {
                debug!("Final transcription: '{}'", text);
                final_text = Some(text);
            }
        }

        self.finalize_turn_with_text(reason, final_text)
    }

    /// Finalize the current turn with optional text and reset state
    fn finalize_turn_with_text(
        &mut self,
        reason: CloseReason,
        text: Option<String>,
    ) -> Option<SegmentedTurn> {
        if self.opened_at.is_none() || self.buffer.is_empty() {
            // No open turn or empty buffer
            self.reset_state();
            return None;
        }

        // Create the turn
        let turn = SegmentedTurn {
            pcm: std::mem::take(&mut self.buffer),
            partial_text: text,
            close_reason: reason,
        };

        // Reset state
        self.reset_state();

        Some(turn)
    }

    /// Reset internal state
    fn reset_state(&mut self) {
        self.opened_at = None;
        self.voiced_frames = 0;
        self.silent_ms = 0;
        self.last_whisper_run = None;
        self.buffer.clear();
    }

    /// Check if a turn is currently open/being captured
    pub fn is_capturing(&self) -> bool {
        self.opened_at.is_some() && !self.buffer.is_empty()
    }

    /// Get the number of samples in the current buffer
    pub fn buffer_samples(&self) -> usize {
        self.buffer.len()
    }

    /// Get the current buffer duration in seconds
    pub fn buffer_duration(&self) -> f32 {
        self.buffer.len() as f32 / 16000.0 // 16kHz sample rate
    }
}

/// Check if a string ends with a clause boundary marker
/// More permissive to encourage shorter segments
fn is_clause_boundary(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }

    let trimmed = text.trim();

    // Check for common sentence-ending punctuation
    let has_ending_punct = matches!(
        trimmed.chars().last().unwrap_or(' '),
        '.' | '?' | '!' | ';' | ':' | ',' | '-'
    );

    // Also check for common complete phrases or conjunctions that might indicate
    // a good break point even without punctuation
    let lower = trimmed.to_lowercase();
    let has_phrase_ending = lower.ends_with(" and")
        || lower.ends_with(" but")
        || lower.ends_with(" or")
        || lower.ends_with(" so")
        || lower.ends_with(" then")
        || lower.contains(" because ");

    has_ending_punct || has_phrase_ending
}

/// Determine if text contains a valid semantic clause that can be sent
/// This is much more aggressive than is_clause_boundary, actively looking
/// for any potential meaningful content to send immediately
fn is_valid_clause(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }

    let trimmed = text.trim();

    // If it's very short, wait for more content unless it's a clear statement
    if token_count(trimmed) < 3 && !trimmed.ends_with('?') && !trimmed.ends_with('!') {
        return false;
    }

    // Always recognize questions as valid clauses
    if trimmed.contains('?') {
        return true;
    }

    // Check for sentence-ending punctuation anywhere in the text
    let has_punct = trimmed.contains('.') || trimmed.contains('!') || trimmed.contains(';');

    // Common conjunctions that mark the end of a thought
    let contains_conjunctions = trimmed.contains(" and ")
        || trimmed.contains(" but ")
        || trimmed.contains(" or ")
        || trimmed.contains(" because ")
        || trimmed.contains(" so ")
        || trimmed.contains(" then ");

    // Noticeable pauses in speech that Whisper often marks with commas
    let contains_pauses = trimmed.contains(", ") || trimmed.contains(" - ");

    // Complete sentence detector - look for subject-verb-object patterns
    // This is a simple heuristic to find what might be complete thoughts
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let word_count = words.len();

    // If we have 5+ words and no other triggers, it's probably worth sending
    if word_count >= 5 {
        return true;
    }

    // Look for typical sentence patterns in shorter phrases
    let contains_noun_verb_structure = word_count >= 3 
        && !words[0].starts_with(|c: char| c.is_lowercase()) &&  // First word capitalized 
         trimmed.contains(' '); // At least one space (2+ words)

    // Use any of these indicators to trigger a valid clause
    has_punct
        || contains_conjunctions
        || contains_pauses
        || contains_noun_verb_structure
        || is_clause_boundary(text)
}

/// Very crude token count estimation (space-separated words)
fn token_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Convert i16 PCM samples to u8 bytes (for sending to audio APIs)
pub fn i16_slice_to_u8(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Convert u8 bytes to i16 PCM samples (for processing)
pub fn u8_to_i16_slice(bytes: &[u8]) -> Vec<i16> {
    let mut samples = Vec::with_capacity(bytes.len() / 2);

    for chunk in bytes.chunks_exact(2) {
        if chunk.len() == 2 {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            samples.push(sample);
        }
    }

    samples
}

/// Write i16 PCM samples to a mutable u8 slice
pub fn i16_to_u8_mut(samples: &mut [i16]) -> &mut [u8] {
    unsafe { std::slice::from_raw_parts_mut(samples.as_mut_ptr() as *mut u8, samples.len() * 2) }
}

/// Utility to send a segmented turn to Gemini
///
/// This handles splitting large turns into multiple messages while
/// ensuring the proper activity markers are set.
pub async fn send_turn_to_gemini(
    turn: &SegmentedTurn,
    gemini_client: &mut gemini_client::GeminiClient,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pcm_bytes = i16_slice_to_u8(&turn.pcm);
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

    // End marker - set activity_end=true (but NOT audio_stream_end)
    // audio_stream_end should only be true when closing the entire session
    gemini_client
        .send_audio_with_activity(&[], false, true, false)
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clause_boundary_detection() {
        assert!(is_clause_boundary("This is a test."));
        assert!(is_clause_boundary("Is this a test?"));
        assert!(is_clause_boundary("This is a test!"));
        assert!(is_clause_boundary("First part; second part"));
        assert!(is_clause_boundary("First item: second item"));

        assert!(!is_clause_boundary("This is a test"));
        assert!(!is_clause_boundary(""));
    }

    #[test]
    fn test_valid_clause_detection() {
        // These should all be valid clauses
        assert!(is_valid_clause("Is this a question?"));
        assert!(is_valid_clause("This is a complete sentence."));
        assert!(is_valid_clause(
            "I'm thinking about something, and then I'll decide"
        ));
        assert!(is_valid_clause(
            "First I'll go to the store, then I'll go home"
        ));
        assert!(is_valid_clause("Please help me understand this concept"));
        assert!(is_valid_clause("The weather is nice today"));

        // Very short phrases should not be valid clauses unless they're complete
        assert!(!is_valid_clause("Um"));
        assert!(!is_valid_clause("I am"));
        assert!(!is_valid_clause(""));

        // But short questions or exclamations are valid
        assert!(is_valid_clause("Why?"));
        assert!(is_valid_clause("Go now!"));
    }

    /* <<<<<<<<<<<<<<  ✨ Windsurf Command ⭐ >>>>>>>>>>>>>>>> */
    /// Test that the token count function accurately counts space-separated words
    ///
    /// The purpose of this function is to demonstrate that the token count
    /// function is working correctly. It should return the number of words
    /// in each string, and 0 for an empty string.
    /* <<<<<<<<<<  9dfab825-6146-4cea-9d57-c9b0d7e46e86  >>>>>>>>>>> */
    #[test]
    fn test_token_count() {
        assert_eq!(token_count("This is a test"), 4);
        assert_eq!(token_count(""), 0);
        assert_eq!(token_count("One"), 1);
        assert_eq!(token_count("One two three four five six"), 6);
    }

    #[test]
    fn test_i16_u8_conversion() {
        let samples = vec![0i16, 100, -100, i16::MAX, i16::MIN];
        let bytes = i16_slice_to_u8(&samples);
        let samples2 = u8_to_i16_slice(&bytes);
        assert_eq!(samples, samples2);
    }
}
