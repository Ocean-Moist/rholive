use rholive::audio_seg::{AudioSegmenter, CloseReason, SegConfig};
use std::error::Error;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

/// Demo program to visualize audio segmentation in real-time
/// Shows how speech is broken into semantic segments as you speak
/// Useful to understand how the aggressive Whisper segmentation works
/// Save PCM audio data to a WAV file
fn save_pcm_to_wav(pcm: &[i16], filename: &str) -> Result<(), Box<dyn Error>> {
    // Create WAV writer
    let file = File::create(filename)?;
    let mut writer = BufWriter::new(file);

    // Create WAV specification
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    // Create WAV writer
    let mut wav_writer = hound::WavWriter::new(&mut writer, spec)?;

    // Write PCM samples
    for sample in pcm {
        wav_writer.write_sample(*sample)?;
    }

    // Finalize file
    wav_writer.finalize()?;

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("🎤 Audio Segmentation Demo");
    println!("This demo visualizes how your speech is broken into semantic segments");
    println!("as you speak. It uses the same segmentation system as the main app.");
    println!();
    println!("Try speaking in sentences or with pauses and observe how segments");
    println!("are created. The segmenter now aggressively detects 'valid clauses'");
    println!("even when you're still speaking.");
    println!();
    println!("Press Ctrl+C to exit\n");

    // Initialize audio capture
    let mut audio = rholive::audio::AudioCapturer::new("segment_test")?;

    // Create whisper model path (use the local model)
    let whisper_model_path = Path::new("./tiny.en-q8.gguf");

    // Check if model exists
    let whisper_path = if whisper_model_path.exists() {
        println!("✅ Using Whisper model at {}", whisper_model_path.display());
        Some(whisper_model_path)
    } else {
        println!(
            "⚠️ Whisper model not found at {}, falling back to VAD-only mode",
            whisper_model_path.display()
        );
        None
    };

    // Create segmenter configuration with aggressive settings
    let seg_config = SegConfig {
        open_voiced_frames: 4,    // 80ms of speech to open
        close_silence_ms: 300,    // 300ms silence to close
        max_turn_ms: 5000,        // 5 seconds max turn
        whisper_gate: true,       // Use semantic gating
        clause_tokens: 8,         // 8 tokens is roughly a short phrase
        whisper_interval_ms: 300, // Run Whisper every 300ms
    };

    // Create the segmenter
    let mut segmenter = AudioSegmenter::new(seg_config, whisper_path.as_deref())?;

    println!("🔊 Recording started - speak now!\n");

    // Variables for UI display
    let mut is_capturing = false;
    let mut segment_start_time: Option<Instant> = None;
    let mut segments_completed = 0;

    // Main capture loop
    loop {
        // Create a buffer for 100ms of 16kHz mono audio (1600 samples)
        let mut buffer = [0i16; 1600];
        let mut buffer_u8 = rholive::audio_seg::i16_to_u8_mut(&mut buffer);

        // Read audio data
        if let Err(e) = audio.read(&mut buffer_u8) {
            eprintln!("Error reading audio: {}", e);
            thread::sleep(Duration::from_millis(100));
            continue;
        }

        // Process the buffer
        if let Some(turn) = segmenter.push_chunk(&buffer) {
            // A segment was completed!
            segments_completed += 1;

            // Calculate duration in seconds
            let duration_sec = turn.pcm.len() as f32 / 16000.0;

            // Get the transcribed text
            let text = turn.partial_text.as_deref().unwrap_or("<no transcription>");

            // Get the reason for closing
            let reason = match turn.close_reason {
                CloseReason::Silence => "silence",
                CloseReason::MaxLength => "max length",
                CloseReason::WhisperClause => "semantic clause",
            };

            println!(
                "\n🔴 SEGMENT #{} COMPLETED - Reason: {}",
                segments_completed, reason
            );
            println!("📝 Transcription: \"{}\"", text);
            println!(
                "⏱️ Duration: {:.2} seconds ({} samples)",
                duration_sec,
                turn.pcm.len()
            );

            // Save audio to WAV file
            let filename = format!(
                "segment_{:03}_{}.wav",
                segments_completed,
                reason.replace(" ", "_")
            );
            if let Err(e) = save_pcm_to_wav(&turn.pcm, &filename) {
                eprintln!("Error saving WAV file: {}", e);
            } else {
                println!("💾 Saved audio to: {}", filename);
            }

            println!("-----------------------------------------------------------");

            // Reset UI state
            is_capturing = false;
            segment_start_time = None;
        } else {
            // No completed segment, check if capturing state changed
            if !is_capturing && segmenter.is_capturing() {
                is_capturing = true;
                segment_start_time = Some(Instant::now());
                print!("\r🟢 Capturing started...                                           ");
                io::stdout().flush().unwrap();
            } else if is_capturing && !segmenter.is_capturing() {
                is_capturing = false;
                print!("\r⚪ Not capturing (waiting for speech)                              ");
                io::stdout().flush().unwrap();
            }

            // Update the buffer display if capturing
            if is_capturing {
                let elapsed = segment_start_time.unwrap().elapsed();
                let seconds = elapsed.as_secs_f32();
                let buffer_seconds = segmenter.buffer_duration();
                let buffer_samples = segmenter.buffer_samples();

                print!(
                    "\r🟢 Capturing: {:.1}s elapsed, {:.1}s audio ({} samples)           ",
                    seconds, buffer_seconds, buffer_samples
                );
                io::stdout().flush().unwrap();
            }
        }

        // Small delay to avoid consuming too much CPU
        thread::sleep(Duration::from_millis(10));
    }
}
