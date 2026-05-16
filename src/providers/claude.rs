use crate::screenshot::pick_declared_resolution;
use futures_util::StreamExt;

/// A side-effecting action Claude requested via one of the tools in
/// `find_action`. The streaming parser surfaces these in real time so the
/// caller can fire them before the response is finished.
#[derive(Debug, Clone)]
pub enum Action {
    /// `computer` tool, `mouse_move`. Visual overlay moves to (x,y) but
    /// no real input is injected. Used for "where is X" / "show me X".
    Point { x: i64, y: i64 },
    /// `computer` tool, `left_click`. Visual overlay AND system mouse
    /// click at (x,y). Used for "click X" / "press X" / "select X".
    Click { x: i64, y: i64 },
    /// `computer` tool, `type`. Types `text` into the currently focused
    /// field. Used for "type X" / "search for X" / "write X". Embed a
    /// trailing \n if the result should be submitted (Enter).
    Type { text: String },
    /// `open_url` custom tool. URL is whatever Claude emitted; validation
    /// happens at execution time, not here.
    OpenUrl { url: String },
    /// `launch_app` custom tool. App is a desktop-file basename or a
    /// runnable binary name.
    LaunchApp { app: String },
    /// `switch_to_window` custom tool. Target is a window class or title.
    SwitchToWindow { target: String },
}

pub struct Claude {
    pub http: reqwest::Client,
    /// Full URL to POST messages requests to. Either the hosted proxy or
    /// api.anthropic.com depending on which mode we're in.
    pub endpoint: String,
    /// (header_name, header_value) for auth. Either ("x-aegis-device-id", uuid)
    /// when routed through the proxy, or ("x-api-key", anthropic_key) in
    /// direct mode.
    pub auth: (String, String),
}

/// Default endpoint for the hosted proxy. Override at compile time by setting
/// `AEGIS_PROXY_URL` to a different worker URL if you deploy your own.
const PROXY_URL: &str = "https://aegis-proxy.danielbusnz.workers.dev/v1/anthropic/messages";
const DIRECT_URL: &str = "https://api.anthropic.com/v1/messages";

impl Claude {
    /// Initialize from `.env`/environment. Default behavior is to route through
    /// the hosted aegis-proxy on Cloudflare, identified by a per-install UUID.
    /// No API key needed — that's the whole plug-and-play story.
    ///
    /// To bypass the proxy and talk to Anthropic directly (useful for local
    /// dev, debugging, or burning your own credit), set
    /// `AEGIS_ANTHROPIC_DIRECT=1` in the environment AND provide
    /// `ANTHROPIC_API_KEY`.
    ///
    /// `http` is the shared `reqwest::Client` so connection pools (TCP/TLS)
    /// are reused across calls. Saves the ~150ms handshake on every call
    /// after the first.
    pub fn from_env(http: reqwest::Client) -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();

        if std::env::var("AEGIS_ANTHROPIC_DIRECT").is_ok() {
            let api_key = std::env::var("ANTHROPIC_API_KEY")?;
            return Ok(Claude {
                http,
                endpoint: DIRECT_URL.to_string(),
                auth: ("x-api-key".to_string(), api_key),
            });
        }

        let device_id = super::device_id::load_or_create()?;
        Ok(Claude {
            http,
            endpoint: PROXY_URL.to_string(),
            auth: ("x-aegis-device-id".to_string(), device_id),
        })
    }
}

impl Claude {
    /// Action-dispatch call optimized for SPEED. Claude looks at the
    /// screenshot, picks ONE tool (click, open_url, launch_app, or
    /// switch_to_window), and invokes it. The prompt forces the model
    /// to skip preamble and go straight to the tool call. Designed to
    /// fire in parallel with [`Claude::describe_with_image`] so the
    /// action lands before the spoken response is ready.
    ///
    /// `image_b64` is a base64-encoded JPEG captured at native resolution.
    /// This function resizes it to the aspect-matched declared resolution
    /// internally so click coords can be scaled back accurately.
    ///
    /// `on_action` fires the instant Claude finishes streaming the tool's
    /// input JSON, so the caller can dispatch the side effect (cursor
    /// move, browser open, app launch, etc.) mid-stream.
    pub async fn find_action<F>(
        &self,
        prompt: &str,
        image_b64: &str,
        window_x: i64,
        window_y: i64,
        window_width: i64,
        window_height: i64,
        mut on_action: F,
    ) -> Result<Option<Action>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(Action),
    {
        // `image_b64` is expected to be PRE-RESIZED to one of the Computer Use
        // declared resolutions. We re-derive (declared_w, declared_h) from the
        // window dimensions so the coord-scaling math stays consistent.
        let (declared_w, declared_h) = pick_declared_resolution(window_width, window_height);
        eprintln!(
            "[timing-claude:find_action] image size ({} KB b64)",
            image_b64.len() / 1024
        );

        let user_prompt = format!(
            "The user said: \"{}\". Pick the single best action and invoke its tool. \
             Skip directly to the tool call — no text, no preamble.",
            prompt
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            // 500 gives ample headroom for any preamble Claude might emit
            // before the tool call. Empirically the model uses ~60 tokens
            // on "I'll click on..." text before the actual tool block.
            "max_tokens": 500,
            "stream": true,
            "system": "You are a desktop voice-assistant action dispatcher. A screenshot \
                       of the user's screen is in this message — do NOT call \
                       action=\"screenshot\" on the computer tool, it is forbidden. \
                       Pick exactly ONE tool based on the user's request:\n\
                       - `computer` mouse_move(coordinate=[x,y]): the user wants to SEE \
                         where something is on screen, no click (\"where is the play \
                         button\", \"show me X\", \"find X\", \"point at X\"). Cursor \
                         visually moves but no input is injected.\n\
                       - `computer` left_click(coordinate=[x,y]): the user wants to \
                         actually CLICK something visible on screen (\"click the play \
                         button\", \"press X\", \"select that\"). Cursor moves AND a \
                         real click fires.\n\
                       - `computer` type(text=\"...\"): type text into the currently \
                         focused field (\"type hello\", \"search for X\"). If the user \
                         clearly wants the text submitted (e.g. \"search for X\", \"send \
                         the message X\"), end `text` with \\n so Enter fires. For multi-\
                         step intents like \"search YouTube for cats\" emit BOTH a \
                         left_click on the search bar AND a type(text=\"cats\\n\") in the \
                         same response — aegis will run them in order.\n\
                       - `open_url`: to navigate to a URL (\"open the rust docs for map\", \
                         \"pull up youtube.com\"). Use https:// URLs only.\n\
                       - `launch_app`: to start an app that may not be running yet \
                         (\"open spotify\", \"launch vs code\", \"open my terminal\"). \
                         Pass the lowercase common name.\n\
                       - `switch_to_window`: to focus an app the user already has open \
                         (\"switch to firefox\", \"focus my terminal\"). Pass a window \
                         class or title substring.\n\
                       No preamble, no description, no explanation. Skip directly to \
                       the tool call. If none fits, return plain text saying why.",
            "tools": [
                {
                    "type": "computer_20250124",
                    "name": "computer",
                    "display_width_px": declared_w,
                    "display_height_px": declared_h
                },
                {
                    "name": "open_url",
                    "description": "Open a URL in the user's default web browser. \
                        Use ONLY for full https:// or http:// URLs the user explicitly \
                        wants to navigate to. Do NOT use for clicking a link visible on \
                        screen — use the computer tool's left_click for that.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "url": {
                                "type": "string",
                                "description": "Fully-qualified URL including scheme."
                            }
                        },
                        "required": ["url"]
                    }
                },
                {
                    "name": "launch_app",
                    "description": "Launch a desktop application by name. Use for queries \
                        like 'open Spotify', 'launch Firefox', 'open my terminal'. The app \
                        argument is the app's common name (e.g. 'spotify', 'firefox', \
                        'code', 'kitty'). Do NOT use for switching to an already-running \
                        app — use switch_to_window for that.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "app": {
                                "type": "string",
                                "description": "App name or .desktop file basename, lowercase."
                            }
                        },
                        "required": ["app"]
                    }
                },
                {
                    "name": "switch_to_window",
                    "description": "Focus an already-running application window. Use for \
                        'switch to Firefox', 'focus my terminal' when the app is already \
                        open. Do NOT use to launch a new app — use launch_app for that. \
                        The target is a window class or title substring.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "target": {
                                "type": "string",
                                "description": "Window class (e.g. 'firefox') or title substring."
                            }
                        },
                        "required": ["target"]
                    }
                }
            ],
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": image_b64 } },
                    { "type": "text", "text": user_prompt }
                ]
            }]
        });

        let t_send = std::time::Instant::now();
        let response = self
            .http
            .post(&self.endpoint)
            .header(&self.auth.0, &self.auth.1)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "computer-use-2025-01-24")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        eprintln!(
            "[timing-claude:find_action] upload + headers received → {:?}",
            t_send.elapsed()
        );

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Computer Use API error {}: {}", status, text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut tool_json_buffer = String::new();
        let mut text_buffer = String::new();
        let mut current_tool_name: Option<String> = None;
        let mut last_action: Option<Action> = None;
        let mut first_byte_logged = false;
        let mut stop_reason: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            if !first_byte_logged {
                eprintln!(
                    "[timing-claude:find_action] first SSE byte → {:?}",
                    t_send.elapsed()
                );
                first_byte_logged = true;
            }
            let chunk = chunk?;
            let s = std::str::from_utf8(&chunk)?;
            buffer.push_str(s);

            while let Some(idx) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..idx + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };

                    match event["type"].as_str() {
                        Some("content_block_start") => {
                            if event["content_block"]["type"].as_str() == Some("tool_use") {
                                current_tool_name = event["content_block"]["name"]
                                    .as_str()
                                    .map(str::to_string);
                                tool_json_buffer.clear();
                            } else {
                                current_tool_name = None;
                            }
                        }
                        Some("content_block_delta") => {
                            let delta_type = event["delta"]["type"].as_str();
                            if delta_type == Some("input_json_delta") {
                                if let Some(j) = event["delta"]["partial_json"].as_str() {
                                    tool_json_buffer.push_str(j);
                                }
                            } else if delta_type == Some("text_delta") {
                                if let Some(t) = event["delta"]["text"].as_str() {
                                    text_buffer.push_str(t);
                                }
                            }
                        }
                        Some("content_block_stop") => {
                            if let Some(name) = current_tool_name.take() {
                                if !tool_json_buffer.is_empty() {
                                    match serde_json::from_str::<serde_json::Value>(
                                        &tool_json_buffer,
                                    ) {
                                        Ok(input) => {
                                            if let Some(action) = parse_tool_call(
                                                &name,
                                                &input,
                                                declared_w,
                                                declared_h,
                                                window_x,
                                                window_y,
                                                window_width,
                                                window_height,
                                            ) {
                                                on_action(action.clone());
                                                last_action = Some(action);
                                            } else {
                                                eprintln!(
                                                    "[claude:find_action] tool '{}' input didn't match any handler: {}",
                                                    name, tool_json_buffer
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[claude:find_action] tool '{}' JSON didn't parse ({}): {}",
                                                name, e, tool_json_buffer
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Some("message_delta") => {
                            if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                                stop_reason = Some(reason.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_action.is_none() {
            eprintln!(
                "[claude:find_action] NO ACTION returned. stop_reason={:?}, text_emitted={:?}",
                stop_reason.as_deref().unwrap_or("(none)"),
                if text_buffer.is_empty() {
                    "(empty)".to_string()
                } else {
                    text_buffer.clone()
                }
            );
        }

        Ok(last_action)
    }

    /// Vision call optimized for the SPOKEN RESPONSE — Claude looks at the
    /// screenshot and answers in plain text, streaming tokens as they arrive.
    /// No tools, no Computer Use overhead. Designed to be fired in parallel
    /// with [`Claude::find_action`].
    ///
    /// The `on_token` callback fires for each text delta so callers can pipe
    /// partial text to a streaming TTS.
    pub async fn describe_with_image<F>(
        &self,
        prompt: &str,
        image_b64: &str,
        mut on_token: F,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(&str),
    {
        eprintln!(
            "[timing-claude:describe] image size ({} KB b64)",
            image_b64.len() / 1024
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 200,
            "stream": true,
            "system": "You are aegis, a desktop voice assistant looking at the user's screen. Your responses will be spoken aloud. Respond conversationally in 1-2 sentences using only plain text — no markdown, no asterisks, no bullet points, no emojis.\n\nIMPORTANT: A parallel dispatcher already handles opening URLs, launching apps, switching windows, and clicking UI elements for the user. If the user is asking you to do one of those things, do NOT say you can't — assume it's being handled, and either acknowledge briefly (\"opening it now\") or just answer any non-action part of their question. Never tell the user you can't open apps, navigate to URLs, switch windows, or click things.",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": image_b64 } },
                    { "type": "text", "text": prompt }
                ]
            }]
        });

        let t_send = std::time::Instant::now();
        let response = self
            .http
            .post(&self.endpoint)
            .header(&self.auth.0, &self.auth.1)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        eprintln!(
            "[timing-claude:describe] upload + headers received → {:?}",
            t_send.elapsed()
        );

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Anthropic API error {}: {}", status, text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut accumulated = String::new();
        let mut first_byte_logged = false;

        while let Some(chunk) = stream.next().await {
            if !first_byte_logged {
                eprintln!(
                    "[timing-claude:describe] first SSE byte → {:?}",
                    t_send.elapsed()
                );
                first_byte_logged = true;
            }
            let chunk = chunk?;
            let s = std::str::from_utf8(&chunk)?;
            buffer.push_str(s);

            while let Some(idx) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..idx + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };
                    if event["type"] == "content_block_delta"
                        && event["delta"]["type"] == "text_delta"
                    {
                        if let Some(t) = event["delta"]["text"].as_str() {
                            accumulated.push_str(t);
                            on_token(t);
                        }
                    }
                }
            }
        }

        Ok(accumulated)
    }
}

/// Dispatch a completed `tool_use` block to the corresponding `Action`
/// variant. Returns None if the tool name is unknown or the input shape
/// doesn't match (e.g. `computer` with an action other than `left_click`,
/// or a custom tool missing its required field). Each Some(_) is ready to
/// hand to the caller's `on_action` callback.
#[allow(clippy::too_many_arguments)]
fn parse_tool_call(
    tool_name: &str,
    input: &serde_json::Value,
    declared_w: u32,
    declared_h: u32,
    window_x: i64,
    window_y: i64,
    window_width: i64,
    window_height: i64,
) -> Option<Action> {
    match tool_name {
        "computer" => {
            let action = input["action"].as_str()?;
            // `type` doesn't carry a coordinate; handle it before the coord
            // extraction so we don't reject it for missing one.
            if action == "type" {
                let text = input["text"].as_str()?;
                return Some(Action::Type {
                    text: text.to_string(),
                });
            }
            let coord = input["coordinate"].as_array().filter(|c| c.len() == 2)?;
            let raw_x = coord[0]
                .as_i64()
                .unwrap_or(0)
                .clamp(0, declared_w as i64 - 1);
            let raw_y = coord[1]
                .as_i64()
                .unwrap_or(0)
                .clamp(0, declared_h as i64 - 1);
            let sx =
                window_x + (raw_x as f64 * window_width as f64 / declared_w as f64) as i64;
            let sy =
                window_y + (raw_y as f64 * window_height as f64 / declared_h as f64) as i64;
            let x = sx.clamp(window_x, window_x + window_width - 1);
            let y = sy.clamp(window_y, window_y + window_height - 1);
            match action {
                "left_click" => Some(Action::Click { x, y }),
                "mouse_move" => Some(Action::Point { x, y }),
                _ => None,
            }
        }
        "open_url" => input["url"]
            .as_str()
            .map(|s| Action::OpenUrl { url: s.to_string() }),
        "launch_app" => input["app"]
            .as_str()
            .map(|s| Action::LaunchApp { app: s.to_string() }),
        "switch_to_window" => input["target"]
            .as_str()
            .map(|s| Action::SwitchToWindow { target: s.to_string() }),
        _ => None,
    }
}
