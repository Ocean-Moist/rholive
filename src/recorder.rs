//! Turn recorder for testing - saves frames and audio to filesystem

use crate::media_event::{Outgoing, WsOutbound};
use base64::Engine;
use chrono::Local;
use std::fs::{self, File};
use std::io::{Write, BufWriter};
use std::path::PathBuf;
use tracing::{debug, info, error};

pub struct TurnRecorder {
    enabled: bool,
    base: PathBuf,           // ./recordings/20250603_153055/
    cur_dir: Option<PathBuf>,
    cur_audio: Option<BufWriter<File>>,
    pending_audio_close_for_turn: bool, // Delay directory closure until after activityEnd
}

impl TurnRecorder {
    pub fn new(enabled: bool) -> Self {
        let ts = Local::now().format("%Y%m%d_%H%M%S").to_string();
        let base = PathBuf::from("recordings").join(ts);
        
        if enabled {
            if let Err(e) = fs::create_dir_all(&base) {
                error!("Failed to create recordings directory: {}", e);
            } else {
                info!("Recording enabled, saving to: {:?}", base);
            }
        }
        
        Self {
            enabled,
            base,
            cur_dir: None,
            cur_audio: None,
            pending_audio_close_for_turn: false,
        }
    }

    pub fn on_outgoing(&mut self, o: &Outgoing) {
        if !self.enabled {
            return;
        }
        
        match o {
            Outgoing::ActivityStart(turn_id) => {
                // One directory per turn
                let dir = self.base.join(format!(
                    "turn_{:03}_{}", 
                    turn_id,
                    Local::now().format("%H%M%S%.3f")
                ));
                
                if let Err(e) = fs::create_dir_all(&dir) {
                    error!("Failed to create turn directory: {}", e);
                    return;
                }
                
                debug!("Starting recording for turn {} in {:?}", turn_id, dir);
                self.cur_dir = Some(dir.clone());
                self.pending_audio_close_for_turn = false; // Reset flag for new turn
                
                // Open audio writer
                match File::create(dir.join("audio.pcm")) {
                    Ok(file) => {
                        self.cur_audio = Some(BufWriter::new(file));
                    }
                    Err(e) => {
                        error!("Failed to create audio file: {}", e);
                    }
                }
            }
            
            Outgoing::AudioChunk(pcm, _turn_id) => {
                if let Some(writer) = self.cur_audio.as_mut() {
                    if let Err(e) = writer.write_all(pcm) {
                        error!("Failed to write audio chunk: {}", e);
                    }
                }
            }
            
            Outgoing::ActivityEnd(_turn_id) => {
                // Flush and close audio file
                if let Some(writer) = self.cur_audio.take() {
                    if let Err(e) = writer.into_inner() {
                        error!("Failed to flush audio writer: {:?}", e);
                    } else {
                        debug!("Closed audio file for turn");
                    }
                }
                // Don't close directory yet - wait for activityEnd WebSocket message
                // This allows forced frames at end of audio turn to be saved
                self.pending_audio_close_for_turn = true;
            }
            
            // Save video frames immediately when we see them
            Outgoing::VideoFrame(jpeg, _turn_id) => {
                if let Some(dir) = &self.cur_dir {
                    let ts = Local::now().format("%H%M%S%.3f");
                    let path = dir.join(format!("frame_{}.jpg", ts));
                    
                    match File::create(&path) {
                        Ok(mut file) => {
                            if let Err(e) = file.write_all(jpeg) {
                                error!("Failed to write frame: {}", e);
                            } else {
                                debug!("Saved frame to {:?}", path);
                            }
                        }
                        Err(e) => {
                            error!("Failed to create frame file: {}", e);
                        }
                    }
                } else {
                    debug!("VideoFrame received but no turn directory is open");
                }
            }
        }
    }

    pub fn on_ws(&mut self, msg: &WsOutbound) {
        if !self.enabled {
            return;
        }
        
        match msg {
            WsOutbound::Json(json) => {
                // Handle activityStart for video-only turns
                if json.get("activityStart").is_some() && self.cur_dir.is_none() {
                    // Create a directory for this turn
                    static mut VIDEO_TURN_COUNTER: u64 = 1000; // Start at 1000 to distinguish from audio turns
                    let turn_id = unsafe {
                        let id = VIDEO_TURN_COUNTER;
                        VIDEO_TURN_COUNTER += 1;
                        id
                    };
                    
                    let dir = self.base.join(format!(
                        "turn_v{:03}_{}", 
                        turn_id,
                        Local::now().format("%H%M%S%.3f")
                    ));
                    
                    if let Err(e) = fs::create_dir_all(&dir) {
                        error!("Failed to create video turn directory: {}", e);
                        return;
                    }
                    
                    debug!("Starting recording for video turn {} in {:?}", turn_id, dir);
                    self.cur_dir = Some(dir);
                }
                
                // Check if this is a video frame
                if let Some(video) = json.get("video") {
                    if let Some(data_b64) = video.get("data").and_then(|d| d.as_str()) {
                        if let Some(dir) = &self.cur_dir {
                            // Decode base64 JPEG data
                            match base64::engine::general_purpose::STANDARD.decode(data_b64) {
                                Ok(bytes) => {
                                    let ts = Local::now().format("%H%M%S%.3f");
                                    let path = dir.join(format!("frame_{}.jpg", ts));
                                    
                                    match File::create(&path) {
                                        Ok(mut file) => {
                                            if let Err(e) = file.write_all(&bytes) {
                                                error!("Failed to write frame: {}", e);
                                            } else {
                                                debug!("Saved frame to {:?}", path);
                                            }
                                        }
                                        Err(e) => {
                                            error!("Failed to create frame file: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to decode frame base64: {}", e);
                                }
                            }
                        } else {
                            debug!("Video frame received but no turn directory is open");
                        }
                    }
                }
                
                // Handle activityEnd
                if json.get("activityEnd").is_some() && self.pending_audio_close_for_turn {
                    // This is the end of an audio turn, close the directory
                    debug!("Closing audio turn directory after activityEnd");
                    self.cur_dir = None;
                    self.pending_audio_close_for_turn = false;
                } else if json.get("activityEnd").is_some() && self.cur_dir.is_some() {
                    // This is the end of a video turn
                    debug!("Closing video turn directory after activityEnd");
                    self.cur_dir = None;
                }
            }
            _ => {} // Ignore other types of messages
        }
    }
}

/// Helper to add WAV header to raw PCM data
pub fn add_wav_header(pcm_data: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let bits_per_sample = 16u16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = pcm_data.len() as u32;
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + pcm_data.len());
    
    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    
    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes());  // PCM format
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    
    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm_data);
    
    wav
}