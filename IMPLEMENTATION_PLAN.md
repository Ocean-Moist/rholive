# Implementation Plan

This project uses a few key native crates to capture audio/video and render an overlay. The dependencies listed in `Cargo.toml` correspond to the initial setup described in `README.md`.

Dependencies added:
- `xcap` for screen recording
- `libpulse-simple-binding` to access system audio and microphone
- `tungstenite` + `tokio` for WebSocket integration with Gemini Live API
- `serde` and `serde_json` for JSON handling
- `egui_window_glfw_passthrough` and `glow` for the overlay UI
- `egui` as the underlying UI framework

Further updates will expand on application logic once network access is available for retrieving crate metadata.
