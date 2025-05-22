/// Real-time audio segmentation demo
/// Shows how speech is broken into segments as you speak
use rholive::audio_seg::{AudioSegmenter, CloseReason, SegConfig};
use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

/// Save PCM audio data to a WAV file
fn save_pcm_to_wav(pcm: &[i16], filename: &str) -> Result<(), Box<dyn Error>> {
    let file = File::create(filename)?;
    let mut writer = BufWriter::new(file);

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut wav_writer = hound::WavWriter::new(&mut writer, spec)?;

    for sample in pcm {
        wav_writer.write_sample(*sample)?;
    }

    wav_writer.finalize()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    // Enable debug logging to see segmentation activity
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🎤 Real-time Audio Segmentation Demo");
    println!("This demo shows how your speech is segmented in real-time");
    println!("using the new ring buffer architecture.");
    println!();
    println!("Try speaking in sentences with pauses and observe how segments");
    println!("are detected based on silence gaps and semantic boundaries.");
    println!();
    println!("Press Ctrl+C to exit\n");

    // Initialize audio capture
    let mut audio = rholive::audio::AudioCapturer::new("segment_demo")?;

    // Create whisper model path
    let whisper_model_path = Path::new("./tiny.en-q8.gguf");
    let whisper_path = if whisper_model_path.exists() {
        println!("✅ Using Whisper model at {}", whisper_model_path.display());
        Some(whisper_model_path)
    } else {
        println!("⚠️ Whisper model not found, using VAD-only mode");
        None
    };

    // Create segmenter configuration for demo (more responsive)
    let seg_config = SegConfig {
        open_voiced_frames: 4,      // 80ms to open (responsive)
        close_silence_ms: 600,      // 600ms silence to close (reasonable pauses)
        max_turn_ms: 8000,          // 8 seconds max (good for demo)
        min_clause_tokens: 4,       // 4 tokens for clause detection
        asr_poll_ms: 400,           // Poll every 400ms
        ring_capacity: 320_000,     // 20 seconds buffer
        asr_pool_size: 2,           // 2 worker threads
        asr_timeout_ms: 2000,       // 2 second timeout
    };

    let mut segmenter = AudioSegmenter::new(seg_config, whisper_path.as_deref())?;
    
    println!("🔊 Recording started - speak now!\n");

    let mut segments_completed = 0;
    let mut last_status_time = Instant::now();

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

        // Process the buffer through the segmenter
        if let Some(turn) = segmenter.push_chunk(&buffer) {
            // A segment was completed!
            segments_completed += 1;

            let duration_sec = turn.audio.len() as f32 / 16000.0;
            let text = turn.text.as_deref().unwrap_or("<no transcription>");
            
            let reason = match turn.close_reason {
                CloseReason::Silence => "silence",
                CloseReason::MaxLength => "max length", 
                CloseReason::AsrClause => "semantic clause",
            };

            println!("\n🎯 SEGMENT #{} DETECTED - Closed by: {}", segments_completed, reason);
            println!("📝 Text: \"{}\"", text);
            println!("⏱️  Duration: {:.2}s ({} samples)", duration_sec, turn.audio.len());

            // Save audio segment
            let filename = format!("demo_segment_{:03}_{:.1}s_{}.wav", 
                                 segments_completed, duration_sec, reason.replace(" ", "_"));
            match save_pcm_to_wav(&turn.audio, &filename) {
                Ok(_) => println!("💾 Saved: {}", filename),
                Err(e) => eprintln!("❌ Save error: {}", e),
            }

            println!("{}", "─".repeat(60));
            last_status_time = Instant::now();
        }

        // Show periodic status to indicate the system is running
        if last_status_time.elapsed() > Duration::from_secs(5) {
            println!("🔍 Listening... (speak to see segmentation)");
            last_status_time = Instant::now();
        }

        // Small delay to prevent busy waiting
        thread::sleep(Duration::from_millis(10));
    }
}