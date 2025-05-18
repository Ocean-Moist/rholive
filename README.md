# rholive
## realtime assistant which has access to system audio, screen, microphone etc

good for interviews, sales calls, etc. 

uses xcap (screen recording), libpulse_simple_binding (sys audio + mic), gemini live api via websockets, and egui_window_glfw_passthrough and glow (overlay rendering).

### Gemini API Wrapper

The `gemini` module defines an async `GeminiClient` for connecting to the
Gemini Live API via WebSocket. Messages are serialized with Serde so the
rest of the application can send setup, realtime input and read responses
easily.

## future features/ideation (not in scop rn)

async send to better model like openai's o3 for hard tasks/interview questions.
