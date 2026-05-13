use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
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
        interim_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
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

        // Task 1: pump audio chunks into WS. When audio_rx closes, send
        // Finalize so Deepgram commits any pending audio as an is_final
        // event. Fire the release signal so the read loop enters its
        // "wait for is_final" phase. Then send CloseStream + close.
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
            // Audio EOS. Tell Deepgram to commit pending audio as is_final.
            let _ = write
                .send(Message::Text("{\"type\":\"Finalize\"}".to_string().into()))
                .await;
            // Signal the read loop to switch from live to "await is_final".
            let _ = release_tx.send(());
            // CRITICAL: do NOT send CloseStream or close the WS yet, or
            // Deepgram will hang up before emitting the is_final event we
            // just asked for via Finalize. Wait until the read loop has
            // had time to receive the response, then clean up.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let _ = write
                .send(Message::Text(
                    "{\"type\":\"CloseStream\"}".to_string().into(),
                ))
                .await;
            let _ = write.close().await;
        });

        // Task 2: read transcripts. Track committed is_final segments
        // separately from the latest interim guess.
        //
        // Phase 1 (recording): race release signal against incoming frames.
        // Phase 2 (await final): after release, send_task issued a Finalize
        // to Deepgram. Keep processing frames until we see an `is_final`
        // covering the tail audio, OR until POST_RELEASE_TIMEOUT_MS as a
        // safety net.
        let mut finalized = String::new();
        let mut latest_interim = String::new();

        // Track what we last broadcast so we only ping the interim_tx on
        // actual changes (avoids spamming the speculative watchdog).
        let mut last_broadcast = String::new();

        // Phase 1: live forwarding.
        loop {
            tokio::select! {
                biased;
                _ = &mut release_rx => break,
                msg = read.next() => {
                    match process_frame(msg, &mut finalized, &mut latest_interim) {
                        FrameOutcome::Continue | FrameOutcome::GotFinal => {
                            // Broadcast the running merged transcript whenever
                            // it changes, so the speculative watchdog (if any)
                            // can detect stability.
                            if let Some(ref tx) = interim_tx {
                                let current = merge_ref(&finalized, &latest_interim);
                                if current != last_broadcast {
                                    last_broadcast = current.clone();
                                    let _ = tx.send(current);
                                }
                            }
                        }
                        FrameOutcome::WsClosed => {
                            // WS died mid-stream — bail with what we have.
                            let result = merge(finalized, latest_interim);
                            eprintln!("[deepgram-debug] WS closed mid-stream, returning: {:?}", result);
                            return Ok(result);
                        }
                    }
                }
            }
        }

        // Phase 2: wait for Deepgram's is_final event for the tail audio.
        // send_task already issued the Finalize message; Deepgram should
        // emit an is_final within ~100-300ms.
        eprintln!(
            "[deepgram-debug] released, awaiting is_final (timeout {}ms)...",
            POST_RELEASE_TIMEOUT_MS
        );
        let timeout = tokio::time::sleep(Duration::from_millis(POST_RELEASE_TIMEOUT_MS));
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                biased;
                _ = &mut timeout => {
                    eprintln!("[deepgram-debug] is_final timeout reached");
                    break;
                }
                msg = read.next() => {
                    match process_frame(msg, &mut finalized, &mut latest_interim) {
                        FrameOutcome::GotFinal => {
                            // Deepgram committed the tail — we have everything.
                            break;
                        }
                        FrameOutcome::Continue => {}
                        FrameOutcome::WsClosed => break,
                    }
                }
            }
        }

        let result = merge(finalized, latest_interim);
        eprintln!("[deepgram-debug] returning: {:?}", result);
        Ok(result)
    }
}

/// Max time to wait after release for Deepgram's is_final response.
/// Deepgram normally answers within 100-300ms of the Finalize message.
/// If something goes wrong (network blip, Deepgram delay), we still return
/// after this timeout with whatever interim we have, so the user isn't
/// stuck.
const POST_RELEASE_TIMEOUT_MS: u64 = 800;

/// What happened when we processed a frame.
enum FrameOutcome {
    /// Frame processed (interim or non-Results), keep looping.
    Continue,
    /// Frame was an is_final event — caller may want to stop waiting.
    GotFinal,
    /// The WS stream ended — caller must stop.
    WsClosed,
}

/// Process one Deepgram WebSocket frame. Updates `finalized` and
/// `latest_interim` in place, and reports what happened.
fn process_frame(
    msg: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    finalized: &mut String,
    latest_interim: &mut String,
) -> FrameOutcome {
    let Some(Ok(Message::Text(text))) = msg else {
        return FrameOutcome::WsClosed;
    };
    let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) else {
        return FrameOutcome::Continue;
    };
    if event["type"] != "Results" {
        return FrameOutcome::Continue;
    }
    let Some(t) = event["channel"]["alternatives"][0]["transcript"].as_str() else {
        return FrameOutcome::Continue;
    };
    let is_final = event["is_final"].as_bool().unwrap_or(false);
    eprintln!(
        "[deepgram-debug] {} → {:?}",
        if is_final { "FINAL" } else { "interim" },
        t
    );
    if is_final {
        if !t.is_empty() {
            if !finalized.is_empty() {
                finalized.push(' ');
            }
            finalized.push_str(t);
        }
        latest_interim.clear();
        FrameOutcome::GotFinal
    } else {
        *latest_interim = t.to_string();
        FrameOutcome::Continue
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

/// Same as `merge` but borrows its inputs — used by the running-transcript
/// broadcast inside the read loop where we don't want to consume state.
fn merge_ref(finalized: &str, latest_interim: &str) -> String {
    if latest_interim.is_empty() {
        return finalized.to_string();
    }
    if finalized.is_empty() {
        return latest_interim.to_string();
    }
    format!("{} {}", finalized, latest_interim)
}
