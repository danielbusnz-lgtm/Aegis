use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Streaming Speech-to-Text via Deepgram's WebSocket endpoint.
///
/// Audio chunks (i16 PCM, little-endian) arrive via the channel; the
/// transcript builds up from interim/final segments and is returned when
/// the audio sender is dropped (end of recording).
pub struct SttDeepgram {
    pub api_key: String,
}

impl SttDeepgram {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("DEEPGRAM_API_KEY")?;
        Ok(Self { api_key })
    }

    /// Open a WebSocket session, pump audio chunks from `audio_rx`, return
    /// the final transcript when the audio channel closes.
    ///
    /// The audio is expected to be `linear16` PCM at the given sample rate
    /// and channel count.
    pub async fn transcribe_stream(
        &self,
        sample_rate: u32,
        channels: u16,
        mut audio_rx: UnboundedReceiver<Vec<i16>>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Build the WSS URL with query params. Deepgram expects the audio
        // format declared here to match what we send. interim_results=true
        // lets us return the latest partial transcript the moment the user
        // releases, without waiting for Deepgram's final commit.
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model=nova-3&language=en\
             &encoding=linear16&sample_rate={}&channels={}\
             &punctuate=true&interim_results=true&smart_format=true",
            sample_rate, channels
        );

        // Build the WS request via tungstenite's IntoClientRequest, then
        // attach the Authorization header. tokio-tungstenite auto-fills the
        // mandatory handshake headers (Sec-WebSocket-Key, Upgrade, etc.).
        let mut request = url.into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Token {}", self.api_key).parse()?);

        let (ws_stream, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut write, mut read) = ws_stream.split();

        // Oneshot signal: fires the moment audio_rx closes (user released
        // the hotkey). The read loop uses this to return immediately with
        // whatever transcript it has, instead of waiting for is_final.
        let (release_tx, mut release_rx) = tokio::sync::oneshot::channel::<()>();

        // Task 1: pump audio chunks into WS. When audio_rx closes, fire
        // the release signal, then close the WS in the background.
        let _send_task = tokio::spawn(async move {
            while let Some(samples) = audio_rx.recv().await {
                let mut bytes = Vec::with_capacity(samples.len() * 2);
                for s in samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                if write.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
            // Audio sender dropped. Signal release BEFORE doing any cleanup
            // so the read loop returns ASAP.
            let _ = release_tx.send(());
            // Close the WS in the background — we don't wait for Deepgram's
            // ack here because we've already returned from the parent fn.
            let _ = write
                .send(Message::Text(
                    "{\"type\":\"CloseStream\"}".to_string().into(),
                ))
                .await;
            let _ = write.close().await;
        });

        // Task 2: read transcripts. Track committed is_final segments
        // separately from the latest interim guess. On release signal,
        // return finalized + latest_interim immediately.
        let mut finalized = String::new();
        let mut latest_interim = String::new();

        loop {
            tokio::select! {
                biased;
                _ = &mut release_rx => {
                    return Ok(merge(finalized, latest_interim));
                }
                msg = read.next() => {
                    let Some(Ok(Message::Text(text))) = msg else {
                        break;
                    };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    if event["type"] != "Results" {
                        continue;
                    }
                    let Some(t) = event["channel"]["alternatives"][0]["transcript"].as_str() else {
                        continue;
                    };
                    if event["is_final"].as_bool().unwrap_or(false) {
                        if !t.is_empty() {
                            if !finalized.is_empty() {
                                finalized.push(' ');
                            }
                            finalized.push_str(t);
                        }
                        latest_interim.clear();
                    } else {
                        latest_interim = t.to_string();
                    }
                }
            }
        }

        Ok(merge(finalized, latest_interim))
    }
}

fn merge(finalized: String, latest_interim: String) -> String {
    if latest_interim.is_empty() {
        return finalized;
    }
    if finalized.is_empty() {
        return latest_interim;
    }
    format!("{} {}", finalized, latest_interim)
}
