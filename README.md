# rholive
## realtime assistant which has access to system audio, screen, microphone etc

good for interviews, sales calls, etc. 

uses xcap (screen recording), libpulse_simple_binding (sys audio + mic), gemini live api via websockets, and egui_window_glfw_passthrough and glow (overlay rendering).

## Features

- Transparent overlay UI with a modern glass-like design
- Real-time audio capture and processing
- Screen recording capability
- Integration with Google's Gemini Live API
- Mute/unmute functionality

## Usage

1. Set up your Gemini API key:
   ```
   export GEMINI_API_KEY=your_api_key_here
   ```

2. Run with UI and screen capture:
   ```
   cargo run --features "ui capture"
   ```

3. Run with UI only (no screen capture):
   ```
   cargo run
   ```

The glass-like UI overlay will appear on your screen with a mute/unmute button and a display area for AI responses.

### Gemini API Wrapper

The `gemini` module defines an async `GeminiClient` for connecting to the
Gemini Live API via WebSocket. Messages are serialized with Serde so the
rest of the application can send setup, realtime input and read responses
easily.

## future features/ideation (not in scop rn)

async send to better model like openai's o3 for hard tasks/interview questions.
