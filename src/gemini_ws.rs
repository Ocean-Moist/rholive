use crate::events::{WsOut, WsIn};
use crate::gemini_client::GeminiClient;
use crate::gemini::ApiResponse;
use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub async fn run(
    api_key: &str,
    mut rx_ws: UnboundedReceiver<WsOut>,
    tx_evt: UnboundedSender<WsIn>,
) -> Result<()> {
    use crate::gemini::GeminiClientConfig;
    
    let mut config = GeminiClientConfig::default();
    config.system_instruction = Some(
        "\
            describe what you see on the screen, if it hasn't changed, respond with '<nothing>' (no quotes). ignore audio. do not repeat yourself.
        \
        ".to_string()
    );
    
    let mut client = GeminiClient::from_api_key(api_key, Some(config));
    
    client.connect().await?;
    client.setup().await?;
    
    let mut response_rx = client.subscribe();
    
    tokio::spawn(async move {
        while let Some(msg) = rx_ws.recv().await {
            use tracing::info;
            info!("📨 Received WsOut message for transmission to Gemini");
            if let Err(e) = handle_outgoing(&mut client, msg).await {
                tracing::error!("❌ Error sending to Gemini: {}", e);
            }
        }
    });
    
    while let Some(response) = response_rx.recv().await {
        match response {
            Ok(api_resp) => {
                if let Err(e) = handle_incoming(api_resp, &tx_evt) {
                    eprintln!("Error handling Gemini response: {}", e);
                }
            }
            Err(e) => {
                tx_evt.send(WsIn::Error(e.to_string()))?;
            }
        }
    }
    
    Ok(())
}

async fn handle_outgoing(client: &mut GeminiClient, msg: WsOut) -> Result<()> {
    use tracing::{debug, info};
    
    match msg {
        WsOut::Setup(_json) => {
            debug!("🔧 Handling Setup message (skipped)");
            Ok(())
        }
        WsOut::RealtimeInput(json) => {
            if json.get("video").is_some() {
                info!("📹 Sending video frame to Gemini client");
            } else if json.get("audio").is_some() {
                info!("🎵 Sending audio chunk to Gemini client");
            } else if json.get("activityStart").is_some() {
                info!("🎬 Sending activityStart to Gemini client");
            } else if json.get("activityEnd").is_some() {
                info!("🎬 Sending activityEnd to Gemini client");
            } else {
                info!("📨 Sending other realtime input to Gemini client");
            }
            client.send_realtime_input(json).await?;
            debug!("✅ Realtime input sent successfully");
            Ok(())
        }
        WsOut::ClientContent(json) => {
            info!("💬 Sending client content to Gemini client");
            client.send_client_content(json).await?;
            debug!("✅ Client content sent successfully");
            Ok(())
        }
    }
}

fn handle_incoming(resp: ApiResponse, tx: &UnboundedSender<WsIn>) -> Result<()> {
    use tracing::info;
    
    match resp {
        ApiResponse::TextResponse { text, is_complete } => {
            if is_complete {
                info!("📥 Received complete text response from Gemini: {}", text.chars().take(100).collect::<String>());
            } else {
                info!("📥 Received partial text response from Gemini: {}", text.chars().take(50).collect::<String>());
            }
            tx.send(WsIn::Text {
                content: text,
                is_final: is_complete,
            })?;
            if is_complete {
                info!("✅ Gemini generation complete");
                tx.send(WsIn::GenerationComplete)?;
            }
        }
        ApiResponse::OutputTranscription(transcript) => {
            info!("📥 Received output transcription from Gemini: {}", transcript.text);
            tx.send(WsIn::Text {
                content: transcript.text,
                is_final: transcript.is_final,
            })?;
        }
        ApiResponse::ConnectionClosed | ApiResponse::GoAway => {
            info!("📥 Gemini connection closing");
            tx.send(WsIn::GenerationComplete)?;
        }
        ApiResponse::GenerationComplete => {
            info!("✅ Forwarding GenerationComplete to broker");
            tx.send(WsIn::GenerationComplete)?;
        }
        _ => {}
    }
    
    Ok(())
}