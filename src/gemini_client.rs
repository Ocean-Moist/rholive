//! Redesigned Gemini Live API client with proper WebSocket handling
//!
//! This module implements a WebSocket client for the Gemini Live API using
//! a split sink/stream approach for concurrent reading and writing.

use crate::gemini::{
    ApiResponse, BidiGenerateContentSetup, ClientMessage, Content, GeminiClientConfig, GeminiError,
    GenerationConfig, Part, RealtimeAudio, RealtimeInput, RealtimeVideo, Result, ServerMessage,
    Transcript,
};

use base64::engine::general_purpose;
use base64::Engine; // Add this trait to use encode/decode methods
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info};

use std::sync::Arc;
use std::time::Duration;

/// Type alias for the WebSocket split sink, wrapped in Arc<Mutex<>>
type WsSink = Arc<
    Mutex<
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
    >,
>;

/// Type alias for the WebSocket split stream
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Connection state of the Gemini client
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Disconnected,
    Connected,
    SetupComplete,
}

/// Redesigned Gemini Live API client with split WebSocket handling
pub struct GeminiClient {
    config: GeminiClientConfig,
    state: ConnectionState,
    session_token: Option<String>,

    // Direct reference to the WebSocket write half for sending messages
    ws_writer: Option<WsSink>,

    // Channel for receiving messages from the WebSocket
    response_rx: mpsc::Receiver<Result<ApiResponse>>,

    // Task handles to keep background tasks alive
    _rx_task: Option<JoinHandle<()>>,
    _tx_task: Option<JoinHandle<()>>,
}

impl GeminiClient {
    /// Create a new Gemini client with the given configuration.
    pub fn new(config: GeminiClientConfig) -> Self {
        // Create dummy channel until connect() is called
        let (_, response_rx) = mpsc::channel(100);

        Self {
            config,
            state: ConnectionState::Disconnected,
            session_token: None,
            ws_writer: None,
            response_rx,
            _rx_task: None,
            _tx_task: None,
        }
    }

    /// Create a new Gemini client from an API key and optional configuration.
    pub fn from_api_key(api_key: &str, config: Option<GeminiClientConfig>) -> Self {
        let mut config = config.unwrap_or_default();
        config.url = format!(
            "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key={}",
            api_key
        );
        Self::new(config)
    }

    /// Get a receiver to subscribe to responses without holding the client mutex
    pub fn subscribe(&mut self) -> mpsc::Receiver<Result<ApiResponse>> {
        // Replace self.response_rx with a fresh dummy so we keep ownership
        let (dummy_tx, new_rx) = mpsc::channel(1);
        let old_rx = std::mem::replace(&mut self.response_rx, new_rx);
        // We don't care about dummy_tx – it just satisfies the type system
        let _ = dummy_tx;
        old_rx
    }

    /// Connect to the Live API endpoint and set up the session.
    pub async fn connect_and_setup(&mut self) -> Result<()> {
        self.connect().await?;
        self.setup().await
    }

    /// Connect to the Live API endpoint.
    pub async fn connect(&mut self) -> Result<()> {
        if self.state != ConnectionState::Disconnected {
            return Ok(());
        }

        info!("Connecting to Gemini API at {}", self.config.url);

        // Connect to the WebSocket
        let (ws_stream, resp) = connect_async(&self.config.url)
            .await
            .map_err(GeminiError::WebSocket)?;

        debug!("WebSocket connection response: {:?}", resp);

        // Split the WebSocket into separate sink (write) and stream (read) halves
        let (sink, stream) = ws_stream.split();

        // Wrap the sink in Arc<Mutex<>> to safely share it
        let sink_shared: WsSink = Arc::new(Mutex::new(sink));

        // Store the sink for later use in send()
        self.ws_writer = Some(sink_shared.clone());

        // ------ Set up the inbound message channel ------
        let (response_tx, new_response_rx) = mpsc::channel::<Result<ApiResponse>>(100);

        // Spawn a task to handle inbound messages
        let rx_task = tokio::spawn(async move {
            info!("Inbound message task started");

            // Process incoming messages from the WebSocket
            let mut stream = stream;

            while let Some(message_result) = stream.next().await {
                match message_result {
                    Ok(Message::Text(text)) => {
                        crate::tdbg!("⬅ websocket message received");
                        debug!("Received text message: {}", text);

                        // Parse and handle the server message
                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(server_message) => {
                                // Handle the server message based on its type
                                match server_message {
                                    ServerMessage::SetupComplete { .. } => {
                                        if let Err(_) =
                                            response_tx.send(Ok(ApiResponse::SetupComplete)).await
                                        {
                                            error!("Failed to send SetupComplete response");
                                            break;
                                        }
                                    }
                                    ServerMessage::ServerContent { server_content } => {
                                        // Process model content, transcriptions, etc.
                                        if let Err(_) =
                                            handle_server_content(server_content, &response_tx)
                                                .await
                                        {
                                            error!("Failed to handle server content");
                                            break;
                                        }
                                    }
                                    ServerMessage::ToolCall { tool_call } => {
                                        if let Err(_) = response_tx
                                            .send(Ok(ApiResponse::ToolCall(tool_call)))
                                            .await
                                        {
                                            error!("Failed to send ToolCall response");
                                            break;
                                        }
                                    }
                                    ServerMessage::ToolCallCancellation {
                                        tool_call_cancellation,
                                    } => {
                                        let id = tool_call_cancellation["id"]
                                            .as_str()
                                            .unwrap_or("unknown")
                                            .to_string();

                                        if let Err(_) = response_tx
                                            .send(Ok(ApiResponse::ToolCallCancellation(id)))
                                            .await
                                        {
                                            error!("Failed to send ToolCallCancellation response");
                                            break;
                                        }
                                    }
                                    ServerMessage::GoAway { .. } => {
                                        if let Err(_) =
                                            response_tx.send(Ok(ApiResponse::GoAway)).await
                                        {
                                            error!("Failed to send GoAway response");
                                            break;
                                        }
                                    }
                                    ServerMessage::SessionResumptionUpdate {
                                        session_resumption_update,
                                    } => {
                                        let handle = session_resumption_update["newHandle"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string();

                                        if let Err(_) = response_tx
                                            .send(Ok(ApiResponse::SessionResumptionUpdate(handle)))
                                            .await
                                        {
                                            error!(
                                                "Failed to send SessionResumptionUpdate response"
                                            );
                                            break;
                                        }
                                    }
                                }
                                crate::tdbg!("✅ websocket message processed");
                            }
                            Err(e) => {
                                error!("Failed to parse server message: {:?}", e);
                                error!("Raw message: {}", text);

                                if let Err(_) =
                                    response_tx.send(Err(GeminiError::Serialization(e))).await
                                {
                                    error!("Failed to send parsing error");
                                    break;
                                }
                                crate::tdbg!("✅ websocket message processing failed");
                            }
                        }
                    }
                    Ok(Message::Binary(bytes)) => {
                        crate::tdbg!("⬅ websocket binary message received");
                        // Try to decode binary message as UTF-8 to see error content
                        if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                            debug!("Received binary message (decoded): {}", text);

                            // Try to parse it as a ServerMessage - binary messages can be valid responses
                            match serde_json::from_str::<ServerMessage>(&text) {
                                Ok(server_message) => {
                                    // Handle the server message based on its type
                                    match server_message {
                                        ServerMessage::SetupComplete { .. } => {
                                            if let Err(_) = response_tx
                                                .send(Ok(ApiResponse::SetupComplete))
                                                .await
                                            {
                                                error!("Failed to send SetupComplete response");
                                                break;
                                            }
                                        }
                                        ServerMessage::ServerContent { server_content } => {
                                            if let Err(_) =
                                                handle_server_content(server_content, &response_tx)
                                                    .await
                                            {
                                                error!("Failed to handle server content");
                                                break;
                                            }
                                        }
                                        ServerMessage::ToolCall { tool_call } => {
                                            if let Err(_) = response_tx
                                                .send(Ok(ApiResponse::ToolCall(tool_call)))
                                                .await
                                            {
                                                error!("Failed to send ToolCall response");
                                                break;
                                            }
                                        }
                                        ServerMessage::ToolCallCancellation {
                                            tool_call_cancellation,
                                        } => {
                                            let id = tool_call_cancellation["id"]
                                                .as_str()
                                                .unwrap_or("unknown")
                                                .to_string();

                                            if let Err(_) = response_tx
                                                .send(Ok(ApiResponse::ToolCallCancellation(id)))
                                                .await
                                            {
                                                error!(
                                                    "Failed to send ToolCallCancellation response"
                                                );
                                                break;
                                            }
                                        }
                                        ServerMessage::GoAway { .. } => {
                                            if let Err(_) =
                                                response_tx.send(Ok(ApiResponse::GoAway)).await
                                            {
                                                error!("Failed to send GoAway response");
                                                break;
                                            }
                                        }
                                        ServerMessage::SessionResumptionUpdate {
                                            session_resumption_update,
                                        } => {
                                            let handle = session_resumption_update["newHandle"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string();

                                            if let Err(_) = response_tx
                                                .send(Ok(ApiResponse::SessionResumptionUpdate(
                                                    handle,
                                                )))
                                                .await
                                            {
                                                error!("Failed to send SessionResumptionUpdate response");
                                                break;
                                            }
                                        }
                                    }
                                    crate::tdbg!("✅ websocket binary message processed");
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to parse binary message as server message: {:?}",
                                        e
                                    );
                                    error!("Raw message: {}", text);
                                    crate::tdbg!("✅ websocket binary message processing failed");
                                }
                            }
                        } else {
                            debug!("Received binary message ({} bytes)", bytes.len());
                            crate::tdbg!("✅ websocket binary message skipped (not text)");
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        if let Some(close_frame) = &frame {
                            error!(
                                "WebSocket closed with code {:?} and reason: {}",
                                close_frame.code, close_frame.reason
                            );

                            // Log detailed analysis for common close reasons
                            if close_frame.reason.contains("Invalid") {
                                error!("CRITICAL: Server rejected a request with INVALID_ARGUMENT, check for:");
                                error!(
                                    "1. Mixing audio data with activity flags in the same frame"
                                );
                                error!("2. Using 'activityControl' instead of newer 'automaticActivityDetection'");
                                error!("3. Sending activity signals in automatic detection mode");
                            } else if close_frame.reason.contains("Explicit activity control") {
                                error!(
                                    "CRITICAL: Server rejected explicit activity control markers!"
                                );
                                error!(
                                    "Make sure automaticActivityDetection.disabled is set to true"
                                );
                            }
                        } else {
                            info!("WebSocket closed without details");
                        }

                        // Notify that the connection is closed (for error handling)
                        if let Err(_) = response_tx.send(Err(GeminiError::ConnectionClosed)).await {
                            error!("Failed to send connection closed notification");
                        }

                        // Send a special ApiResponse message to tell main client to clean up writer
                        // This is processed in next_response() and stream_responses() to clear state
                        if let Err(_) = response_tx.send(Ok(ApiResponse::ConnectionClosed)).await {
                            error!("Failed to send connection closed notification for cleanup");
                        }

                        break;
                    }
                    Ok(_) => {
                        // Ignore other message types (ping/pong)
                    }
                    Err(e) => {
                        error!("WebSocket error: {:?}", e);

                        if let Err(_) = response_tx.send(Err(GeminiError::WebSocket(e))).await {
                            error!("Failed to send WebSocket error");
                        }

                        break;
                    }
                }
            }

            info!("Inbound message task terminated");
        });

        // Store the response channel and task handles in the client
        self.response_rx = new_response_rx;
        self._rx_task = Some(rx_task);

        // Update the client state
        self.state = ConnectionState::Connected;
        info!("Connected to Gemini API");

        Ok(())
    }

    /// Initialize a session by sending the setup message.
    pub async fn setup(&mut self) -> Result<()> {
        if self.state == ConnectionState::Disconnected {
            error!("Cannot setup session: Connection is closed");
            return Err(GeminiError::ConnectionClosed);
        }

        if self.state == ConnectionState::SetupComplete {
            info!("Session already set up");
            return Ok(());
        }

        info!("Setting up Gemini session");

        // Create the setup message
        let mut setup = BidiGenerateContentSetup {
            model: self.config.model.clone(),
            // Convert the system instruction to the proper Content format if provided
            system_instruction: self.config.system_instruction.as_ref().map(|instruction| {
                Content {
                    role: Some("SYSTEM".to_string()),
                    parts: vec![Part {
                        text: Some(instruction.clone()),
                    }],
                }
            }),
            ..Default::default()
        };

        // Set up generation config
        let mut generation_config = GenerationConfig {
            response_modalities: vec![self.config.response_modality.as_str().to_string()],
            temperature: self.config.temperature,
            ..Default::default()
        };

        // Add media resolution if specified
        if let Some(resolution) = self.config.media_resolution {
            generation_config.media_resolution = Some(resolution.as_str().to_string());
        }

        setup.generation_config = Some(generation_config);

        // Create realtime_input_config with correct fields for the Live API
        let mut realtime_config = if let Some(token) = &self.session_token {
            serde_json::json!({
                "sessionResumptionConfig": {
                    "handle": token
                }
            })
        } else {
            serde_json::json!({})
        };

        // Configure for client-side VAD (since we're using Whisper-based segmentation)
        let config_map = realtime_config.as_object_mut().unwrap();

        // Disable automatic activity detection since we're doing client-side VAD
        config_map.insert(
            "automaticActivityDetection".to_string(),
            serde_json::json!({
                "disabled": true
            }),
        );

        // Set activity handling for responsive interruptions
        config_map.insert(
            "activityHandling".to_string(),
            serde_json::json!("START_OF_ACTIVITY_INTERRUPTS"),
        );

        setup.realtime_input_config = Some(realtime_config);

        info!("Sending setup message with model: {}", setup.model);

        // Send the setup message directly using our send method
        let msg = ClientMessage::Setup { setup };
        if let Err(e) = self.send(&msg).await {
            error!("Failed to send setup message: {:?}", e);
            return Err(e);
        }

        info!("Setup message sent, waiting for acknowledgment");

        // Wait for setup complete response with a timeout
        let setup_completed =
            tokio::time::timeout(Duration::from_secs(10), self.wait_for_setup_complete())
                .await
                .map_err(|_| {
                    error!("Timeout waiting for setup complete message");
                    GeminiError::Timeout
                })??;

        if setup_completed {
            self.state = ConnectionState::SetupComplete;
            info!("Gemini session setup complete");
            Ok(())
        } else {
            error!("Failed to complete Gemini session setup");
            Err(GeminiError::SetupNotComplete)
        }
    }

    /// Wait for the setup complete message.
    async fn wait_for_setup_complete(&mut self) -> Result<bool> {
        let mut attempts = 0;
        while attempts < 10 {
            match self.response_rx.recv().await {
                Some(Ok(ApiResponse::SetupComplete)) => {
                    return Ok(true);
                }
                Some(Ok(_)) => {
                    // Ignore other messages
                    attempts += 1;
                    continue;
                }
                Some(Err(e)) => {
                    // Propagate any errors
                    return Err(e);
                }
                None => {
                    // Channel closed
                    return Err(GeminiError::ChannelClosed);
                }
            }
        }
        Ok(false) // Timed out without seeing SetupComplete
    }

    /// Send a client message to the server using the WebSocket writer.
    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        // Check if connection is already closed or writer is cleared
        if self.state == ConnectionState::Disconnected || self.ws_writer.is_none() {
            error!("Cannot send message: Connection is closed");
            return Err(GeminiError::ConnectionClosed);
        }

        // Format the JSON based on message type
        let json = match msg {
            ClientMessage::Setup { setup } => {
                // Format the JSON manually to avoid nesting issues
                let setup_json =
                    serde_json::to_string(setup).map_err(GeminiError::Serialization)?;
                // Remove outer braces and wrap in setup: {...}
                let inner = &setup_json[1..setup_json.len() - 1];
                format!("{{\"setup\":{{{}}}}}", inner)
            }
            ClientMessage::ClientContent { client_content } => {
                format!(
                    "{{\"clientContent\":{}}}",
                    serde_json::to_string(client_content).map_err(GeminiError::Serialization)?
                )
            }
            ClientMessage::RealtimeInput { realtime_input } => {
                format!(
                    "{{\"realtimeInput\":{}}}",
                    serde_json::to_string(realtime_input).map_err(GeminiError::Serialization)?
                )
            }
            ClientMessage::ToolResponse { tool_response } => {
                format!(
                    "{{\"toolResponse\":{}}}",
                    serde_json::to_string(tool_response).map_err(GeminiError::Serialization)?
                )
            }
        };

        // Log message type without the full content to avoid spamming the console
        match msg {
            ClientMessage::Setup { .. } => info!("Sending setup message to Gemini API"),
            ClientMessage::ClientContent { .. } => info!("Sending text content to Gemini API"),
            ClientMessage::RealtimeInput { realtime_input } => {
                if realtime_input.audio.is_some() {
                    if realtime_input.audio_stream_end.unwrap_or(false) {
                        info!("Sending end-of-audio signal to Gemini API");
                    } else {
                        debug!("Sending audio chunk to Gemini API");
                    }
                } else if realtime_input.video.is_some() {
                    debug!("Sending video frame to Gemini API");
                } else if realtime_input.text.is_some() {
                    info!("Sending streaming text to Gemini API");
                }
            }
            ClientMessage::ToolResponse { .. } => info!("Sending tool response to Gemini API"),
        };

        // Use the WebSocket writer directly to send the message
        if let Some(writer) = &self.ws_writer {
            let mut writer_guard = writer.lock().await;
            match writer_guard.send(Message::Text(json.into())).await {
                Ok(_) => {
                    debug!("Message sent successfully");
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to send message: {:?}", e);

                    // If we get a SendAfterClosing error, update our state
                    if e.to_string().contains("SendAfterClosing") {
                        error!("WebSocket is closed - will not try to send more messages");
                        self.state = ConnectionState::Disconnected;
                        // We'll clear the writer after the lock is released
                        drop(writer_guard);
                        self.ws_writer = None; // Prevent future send attempts
                        return Err(GeminiError::WebSocket(e));
                    }

                    Err(GeminiError::WebSocket(e))
                }
            }
        } else {
            error!("WebSocket writer not available (not connected)");
            Err(GeminiError::ConnectionClosed)
        }
    }

    /// Send an audio chunk to the server.
    ///
    /// * `audio_data` - Raw PCM audio bytes
    /// * `activity_start` - Set to true to mark the beginning of speech (user started talking)
    /// * `activity_end` - Set to true to mark the end of speech (user finished talking)
    /// * `is_end` - Set to true to mark the end of the audio stream (used with auto VAD)
    pub async fn send_audio(
        &mut self,
        audio_data: &[u8],
        activity_start: bool,
        activity_end: bool,
        is_end: bool,
    ) -> Result<()> {
        // IMPORTANT: Use the improved send_audio_with_activity method that properly
        // separates audio data and flags into different messages
        self.send_audio_with_activity(audio_data, activity_start, activity_end, is_end)
            .await
    }

    /// Send a video frame to the server.
    pub async fn send_video(&mut self, frame_data: &[u8], mime_type: &str) -> Result<()> {
        // Encode video data as base64
        let data = general_purpose::STANDARD.encode(frame_data);

        let realtime_input = RealtimeInput {
            audio: None,
            video: Some(RealtimeVideo {
                data,
                mime_type: mime_type.to_string(),
            }),
            text: None,
            activity_start: None,
            activity_end: None,
            audio_stream_end: None,
        };

        let msg = ClientMessage::RealtimeInput { realtime_input };
        self.send(&msg).await
    }

    /// Send a text message to the server.
    pub async fn send_text(&mut self, text: &str) -> Result<()> {
        let client_content = serde_json::json!({
            "turns": [{
                "role": "user",
                "parts": [{
                    "text": text
                }]
            }],
            "turnComplete": true
        });

        let msg = ClientMessage::ClientContent { client_content };
        self.send(&msg).await
    }

    /// Send streaming text to the server (e.g. for partial typing).
    pub async fn send_streaming_text(&mut self, text: &str) -> Result<()> {
        let realtime_input = RealtimeInput {
            audio: None,
            video: None,
            text: Some(text.to_string()),
            activity_start: None,
            activity_end: None,
            audio_stream_end: None,
        };

        let msg = ClientMessage::RealtimeInput { realtime_input };
        self.send(&msg).await
    }

    /// Receive the next response from the server.
    pub async fn next_response(&mut self) -> Option<Result<ApiResponse>> {
        let response = self.response_rx.recv().await;

        // Check if this is the special ConnectionClosed message that requires client-side cleanup
        if let Some(Ok(ApiResponse::ConnectionClosed)) = &response {
            info!("Received ConnectionClosed message, clearing WebSocket writer");
            // Clear the writer to prevent further send attempts
            self.ws_writer = None;
            self.state = ConnectionState::Disconnected;
        }

        response
    }

    /// Simplified helper for sending audio with clean parameter names
    ///
    /// When using with client-side VAD (automaticActivityDetection.disabled = true):
    /// - Set activity_start=true to mark the beginning of a user turn
    /// - Send multiple audio chunks with activity_start/end=false
    /// - Set activity_end=true to mark the end of a user turn
    /// - Set audio_stream_end=true when finished with entire session
    ///
    /// CRITICAL: Never mix audio data with activity flags in the same frame
    /// The API treats all fields in RealtimeInput as a "oneof" union
    pub async fn send_audio_with_activity(
        &mut self,
        audio_data: &[u8],
        activity_start: bool,
        activity_end: bool,
        audio_stream_end: bool,
    ) -> Result<()> {
        // Log what we're doing to help with debugging
        if cfg!(debug_assertions) {
            debug!(
                "send_audio_with_activity: data_len={}, start={}, end={}, stream_end={}",
                audio_data.len(),
                activity_start,
                activity_end,
                audio_stream_end
            );
        }

        // Send activity_start flag in its own frame if requested
        if activity_start {
            let flag_only = RealtimeInput {
                audio: None,
                video: None,
                text: None,
                activity_start: Some(serde_json::json!({})),
                activity_end: None,
                audio_stream_end: None,
            };
            let msg = ClientMessage::RealtimeInput {
                realtime_input: flag_only,
            };
            self.send(&msg).await?;
        }

        // If we have audio data, send it in its own clean frame (NO FLAGS)
        if !audio_data.is_empty() {
            // Send audio data only (no flags) in a clean frame
            let data = general_purpose::STANDARD.encode(audio_data);
            let audio_only = RealtimeInput {
                audio: Some(RealtimeAudio {
                    data,
                    mime_type: "audio/pcm;rate=16000".to_string(),
                }),
                video: None,
                text: None,
                activity_start: None,   // IMPORTANT: No flags with audio data
                activity_end: None,     // IMPORTANT: No flags with audio data
                audio_stream_end: None, // IMPORTANT: No flags with audio data
            };

            let msg = ClientMessage::RealtimeInput {
                realtime_input: audio_only,
            };
            self.send(&msg).await?;
        }

        // Send activity_end or audio_stream_end flags in their own frame if requested
        if activity_end || audio_stream_end {
            let flag_only = RealtimeInput {
                audio: None,
                video: None,
                text: None,
                activity_start: None,
                activity_end: if activity_end {
                    Some(serde_json::json!({}))
                } else {
                    None
                },
                audio_stream_end: if audio_stream_end { Some(true) } else { None },
            };
            let msg = ClientMessage::RealtimeInput {
                realtime_input: flag_only,
            };
            self.send(&msg).await?;
        }

        Ok(())
    }

    /// Stream responses until a condition is met.
    pub async fn stream_responses<F>(&mut self, mut callback: F) -> Result<()>
    where
        F: FnMut(&ApiResponse) -> bool,
    {
        while let Some(response) = self.response_rx.recv().await {
            match &response {
                Ok(ApiResponse::ConnectionClosed) => {
                    info!("Received ConnectionClosed message in stream, clearing WebSocket writer");
                    // Clear the writer to prevent further send attempts
                    self.ws_writer = None;
                    self.state = ConnectionState::Disconnected;

                    // Call the callback with this special message
                    let should_stop = callback(&ApiResponse::ConnectionClosed);
                    if should_stop {
                        break;
                    }
                }
                Ok(resp) => {
                    let should_stop = callback(resp);
                    if should_stop {
                        break;
                    }
                }
                Err(e) => {
                    return Err(e.clone()); // Now this is safe with our manual Clone implementation
                }
            }
        }

        Ok(())
    }

    /// Store a session resumption token for later reconnection.
    pub fn set_session_token(&mut self, token: String) {
        self.session_token = Some(token);
    }

    /// Get the current connection state.
    pub fn state(&self) -> &'static str {
        match self.state {
            ConnectionState::Disconnected => "Disconnected",
            ConnectionState::Connected => "Connected",
            ConnectionState::SetupComplete => "SetupComplete",
        }
    }
}

/// Process server content messages which can contain different types of data.
async fn handle_server_content(
    content: serde_json::Value,
    response_tx: &mpsc::Sender<Result<ApiResponse>>,
) -> Result<()> {
    // Check for input transcription (from audio we sent)
    if let Some(input_transcription) = content.get("inputTranscription") {
        // Safely extract text, providing a default if missing
        let text = match input_transcription.get("text").and_then(|t| t.as_str()) {
            Some(t) => t.to_string(),
            None => {
                tracing::warn!(
                    "Received input transcription without text field: {:?}",
                    input_transcription
                );
                String::new() // Empty string as fallback
            }
        };

        // Safely extract isFinal flag
        let is_final = input_transcription
            .get("isFinal")
            .and_then(|f| f.as_bool())
            .unwrap_or(false);

        // Only send if we have actual text content
        if !text.is_empty() {
            response_tx
                .send(Ok(ApiResponse::InputTranscription(Transcript {
                    text,
                    is_final,
                })))
                .await
                .map_err(|_| {
                    tracing::error!("Failed to send input transcription via channel");
                    GeminiError::ChannelClosed
                })?;
        }
    }

    // Check for output transcription (text of model's speech)
    if let Some(output_transcription) = content.get("outputTranscription") {
        // Safely extract text, providing a default if missing
        let text = match output_transcription.get("text").and_then(|t| t.as_str()) {
            Some(t) => t.to_string(),
            None => {
                tracing::warn!(
                    "Received output transcription without text field: {:?}",
                    output_transcription
                );
                String::new() // Empty string as fallback
            }
        };

        // Safely extract isFinal flag
        let is_final = output_transcription
            .get("isFinal")
            .and_then(|f| f.as_bool())
            .unwrap_or(false);

        // Only send if we have actual text content
        if !text.is_empty() {
            response_tx
                .send(Ok(ApiResponse::OutputTranscription(Transcript {
                    text,
                    is_final,
                })))
                .await
                .map_err(|_| {
                    tracing::error!("Failed to send output transcription via channel");
                    GeminiError::ChannelClosed
                })?;
        }
    }

    // Check for generationComplete flag
    if let Some(generation_complete) = content.get("generationComplete").and_then(|g| g.as_bool()) {
        if generation_complete {
            tracing::info!("Generation complete received from Gemini");
            response_tx
                .send(Ok(ApiResponse::GenerationComplete))
                .await
                .map_err(|_| {
                    tracing::error!("Failed to send GenerationComplete via channel");
                    GeminiError::ChannelClosed
                })?;
        }
    }

    // Check for model turn (the actual response)
    if let Some(model_turn) = content.get("modelTurn") {
        // Get parts array, log warning if missing
        let parts = match model_turn.get("parts").and_then(|p| p.as_array()) {
            Some(parts) => parts,
            None => {
                tracing::warn!("Received model turn without parts array: {:?}", model_turn);
                return Ok(()); // Skip processing if no parts
            }
        };

        // Safely extract completion flag
        let is_complete = content
            .get("generationComplete")
            .and_then(|g| g.as_bool())
            .unwrap_or(false);

        // Process each part in the response
        for part in parts {
            // Check for text response
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    response_tx
                        .send(Ok(ApiResponse::TextResponse {
                            text: text.to_string(),
                            is_complete,
                        }))
                        .await
                        .map_err(|_| {
                            tracing::error!("Failed to send text response via channel");
                            GeminiError::ChannelClosed
                        })?;
                }
            }
            // Check for audio response (inline data)
            else if let Some(inline_data) = part.get("inlineData") {
                // Try to extract and decode the base64 data
                match inline_data.get("data").and_then(|d| d.as_str()) {
                    Some(data_str) => {
                        match general_purpose::STANDARD.decode(data_str) {
                            Ok(data) => {
                                // Only send if we have actual data
                                if !data.is_empty() {
                                    response_tx
                                        .send(Ok(ApiResponse::AudioResponse { data, is_complete }))
                                        .await
                                        .map_err(|_| {
                                            tracing::error!(
                                                "Failed to send audio response via channel"
                                            );
                                            GeminiError::ChannelClosed
                                        })?;
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to decode base64 audio data: {:?}", e);
                                // Continue processing other parts even if one fails
                            }
                        }
                    }
                    None => {
                        tracing::warn!(
                            "Received inline data without data field: {:?}",
                            inline_data
                        );
                    }
                }
            }
        }
    }

    Ok(())
}
