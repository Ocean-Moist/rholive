# Audio Segmentation Design

This document details the audio segmentation system used in RhoLive to create natural voice interaction with the Gemini Live API.

## First-Principles Design

The segmentation system is designed based on these first-principles requirements:

| Requirement | Implementation |
|-------------|---------------|
| **Low latency** (sub-word turn-taking) | Whisper runs off-thread and only when needed; main thread stays responsive |
| **Minimal understandable clause** | Segments are closed at semantic boundaries, not just silence |
| **Bandwidth / CPU friendly** | Uses VAD first (cheap), sampling Whisper every few hundred ms |
| **Robust when VAD fails** | Has fallback mechanisms when in noisy environments |
| **Deterministic in test mode** | Boundary heuristics are in pure functions for reliable unit testing |

## Architecture

The audio segmentation system follows this runtime topology:

```
               +---------------+
 Mic 16 kHz →  |  Chunkizer    | -- 100 ms i16 frames -->
               +---------------+
                        |
                        v
           +---------------------------+
           | WebRTC-VAD (4×20 ms hops) |
           +---------------------------+
            | voiced / unvoiced event
            | plus 100 ms frame
            v
   +-----------------------+
   | Segment Manager FSM   |  ← wall-clock
   +-----------------------+
      |        |        |
      |        |        +----- timeout / max_len reached
      |        |
      |        +-- "no close yet" → maybe spawn Whisper job
      |
      +-- "segment closed" → CL-1..N ------>  Storage / Gemini
```

The `AudioSegmenter` owns a mutable buffer of PCM audio, VAD counters, and a Channel to communicate with a lightweight Whisper worker pool. Whisper workers return `(text, clause_ready)` messages that the FSM can use to force-close a segment early when a semantic boundary is detected.

## Finite-State Machine

The segmentation operates as a finite-state machine with three states:

```
┌──────────┐       voiced≥open_frames          valid_clause
│  Idle    │ ───────────────► Capturing ────────────────┐
└──────────┘                                           │
      ▲                                                │
      │  segment closed ◄───────────────┐              │
      └─────────────◄ silence≥close_ms  │              │
                             or         │              │
                   max_turn or clause_ready            │
                                                      ▼
                                                 Flushing
                                             (writes segment,
                                              resets state)
```

## Heuristic for Valid Clause

We use a sophisticated heuristic to determine when a speech segment constitutes a valid clause that can be safely sent for processing:

```rust
fn is_valid_clause(text: &str, min_tokens: usize) -> bool {
    let t = text.trim();
    if t.is_empty() { return false; }

    // Always accept explicit sentence enders
    if t.ends_with(['.','?','!',';']) { return true; }

    // Short Q/A ("why?" "okay!")
    if t.len() <= 20 && t.ends_with(['?','!']) { return true; }

    // Token threshold (≈ words)
    let tokens = t.split_whitespace().count();
    if tokens >= min_tokens { return true; }

    // Speech disfluency commas / conjunctions
    matches!(t.chars().last().unwrap_or(' '), ',' | '-') ||
    t.ends_with(" and") || t.ends_with(" but") ||
    t.contains(" because ")
}
```

This function returns `true` when the text appears to be a semantically complete unit or natural pause point in speech.

## Configuration Parameters

The segmentation can be tuned with these parameters:

| Parameter | Purpose | Default |
|-----------|---------|---------|
| `open_voiced_frames` | Number of voiced frames needed to open a segment | 4 (~80ms) |
| `close_silence_ms` | Silence duration to close a segment | 300ms |
| `max_turn_ms` | Maximum duration of a turn | 5000ms (5s) |
| `min_clause_tokens` | Minimum token count for a valid clause | 8 |
| `whisper_poll_ms` | Interval between Whisper inferences | 300ms |

## Worker Thread Design

The Whisper worker runs in a separate thread and communicates with the main segmenter via channels:
1. Main thread sends audio buffers to the worker
2. Worker performs transcription and clause boundary detection
3. Worker sends results back to the main thread
4. Main thread processes results non-blockingly

This design ensures that the main thread remains responsive even during CPU-intensive speech recognition.

## Benefits Over Previous Implementation

1. **Concurrency**: Whisper runs in a separate thread, keeping the main thread responsive
2. **Resource Efficiency**: Whisper is only executed when a segment is active
3. **Clarity**: Single semantic boundary detection function that's easy to test
4. **State Management**: Explicit state machine for more predictable behavior
5. **Testability**: Pure functions for boundary detection enable reliable unit tests

## Usage

The segmenter is used by feeding it 100ms chunks of 16kHz 16-bit mono audio:

```rust
let config = SegConfig::default();
let segmenter = AudioSegmenter::new(config, Some(Path::new("./whisper-model.gguf")))?;

// Process audio in 100ms chunks (1600 samples at 16kHz)
if let Some(turn) = segmenter.push_chunk(&audio_buffer) {
    // A segment was completed, process it
    println!("Segment closed due to: {:?}", turn.close_reason);
    if let Some(text) = turn.partial_text {
        println!("Transcription: {}", text);
    }
    
    // Send to Gemini for processing
    send_turn_to_gemini(&turn, &mut gemini_client).await?;
}
```