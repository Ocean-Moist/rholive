# Implementation Plan

## Overview

This project aims to implement a realtime assistant with system audio, screen capture, microphone input, and overlay UI. It will integrate with Google's Gemini Live API via WebSockets.

## Next Steps

1. **Set up project dependencies**
   - Add crates for async runtime (`tokio`), WebSocket client (`tokio-tungstenite`), serialization (`serde`/`serde_json`), and logging (`tracing`).
   - Include bindings for audio (`libpulse_simple_binding`), screen capture (`xcap`), and UI overlay (`egui_window_glfw_passthrough` with `glow`).
   - Create modules for API messaging following the schema in `GEMINI_LIVE_API.md`.

2. **Implement Gemini Live API client**
   - Establish a persistent WebSocket connection to the Live API endpoint.
   - Send the initial `BidiGenerateContentSetup` message with desired model and config.
   - Handle `SetupComplete` before streaming audio or video inputs.
   - Stream microphone audio and video frames using `realtimeInput` messages.
   - Parse `serverContent`, `toolCall`, and other server messages.

3. **Audio and Screen Capture**
   - Use `libpulse_simple_binding` to capture system audio and microphone data in 16 kHz PCM.
   - Use the `xcap` crate to capture periodic screenshots or frames for video input.
   - Buffer captured data and send in small chunks (~100ms audio) for lower latency.

4. **Overlay UI**
   - Build an overlay window using `egui` with GLFW passthrough and `glow` for rendering.
   - Display transcription, assistant responses, and basic controls (mute/unmute, start/stop capture).
   - Show visual cues (e.g., when the assistant is listening or speaking).

5. **Concurrency and Event Handling**
   - Run an async task for capturing and sending audio/video.
   - Run another task for reading WebSocket messages and updating the UI.
   - Use channels (`tokio::sync::mpsc`) to pass events between tasks.

6. **Testing and Iteration**
   - Begin with text-only streaming to validate message handling.
   - Add audio input/output once text flow works.
   - Incorporate video capture last due to higher complexity.
   - Use small manual tests to verify UI overlay and system integration.

## Long Term Ideas

- Optionally route difficult requests to more advanced models (e.g., OpenAI `o3`) after initial response.
- Provide settings UI to select audio devices, frame rates, and enable/disable video.
- Support session resumption tokens for reconnecting after disconnects.

This plan should guide initial development toward a functional realtime assistant using the Gemini Live API.