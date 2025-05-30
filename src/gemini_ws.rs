use crate::events::{WsOut, WsIn};
use crate::gemini_client::GeminiClient;
use crate::gemini::{ApiResponse, ServerMessage};
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
        you are a silent helper meant to assist the user in whatever task they choose. if you see a leetcode problem on the screen, solve it without waiting for them to say anything. if someone they are on call with asks you a question, answer it. you are effectively their second mind, they should not have to do any thinking, they should not have to ask you for anything. you are their brain, they should not have to think, respond to whatever is on screen or whatever someone says like the user would.

            when there is no change or nothing to work, do, or comment on, respond only with '<nothing>' (without quotes).
        \
        ".to_string()
    );
    
    let mut client = GeminiClient::from_api_key(api_key, Some(config));
    
    client.connect().await?;
    client.setup().await?;
    
    let mut response_rx = client.subscribe();
    
    tokio::spawn(async move {
        while let Some(msg) = rx_ws.recv().await {
            if let Err(e) = handle_outgoing(&mut client, msg).await {
                eprintln!("Error sending to Gemini: {}", e);
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
    match msg {
        WsOut::Setup(_json) => {
            Ok(())
        }
        WsOut::RealtimeInput(json) => {
            client.send_realtime_input(json).await?;
            Ok(())
        }
        WsOut::ClientContent(json) => {
            client.send_client_content(json).await?;
            Ok(())
        }
    }
}

fn handle_incoming(resp: ApiResponse, tx: &UnboundedSender<WsIn>) -> Result<()> {
    match resp {
        ApiResponse::TextResponse { text, is_complete } => {
            tx.send(WsIn::Text {
                content: text,
                is_final: is_complete,
            })?;
            if is_complete {
                tx.send(WsIn::GenerationComplete)?;
            }
        }
        ApiResponse::OutputTranscription(transcript) => {
            tx.send(WsIn::Text {
                content: transcript.text,
                is_final: transcript.is_final,
            })?;
        }
        ApiResponse::ConnectionClosed | ApiResponse::GoAway => {
            tx.send(WsIn::GenerationComplete)?;
        }
        _ => {}
    }
    
    Ok(())
}