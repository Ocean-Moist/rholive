//! Media input capture module

pub mod audio;
pub mod video;

pub use audio::{spawn_audio_capture, spawn_audio_capture_with_source, AudioSource};
pub use video::spawn_video_capture;