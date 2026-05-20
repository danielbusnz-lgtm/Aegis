//! Tool schema generation and incoming-tool-call parsing for `run_agent_loop`.
//! All functions are crate-private to the `claude` module.

use super::Action;

/// Shared tools array for the agent loop. Accepts extra tool schemas
/// (from integrations, etc.) to append so the function doesn't need to
/// import the integrations module directly.
pub(super) fn tools_array_value(
    declared_w: u32,
    declared_h: u32,
    extra_tools: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut tools: Vec<serde_json::Value> = serde_json::json!([
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
                    "url": { "type": "string", "description": "Fully-qualified URL including scheme." }
                },
                "required": ["url"]
            }
        },
        {
            "name": "launch_app",
            "description": "Launch a desktop application by name. Use for queries \
                like 'open Spotify', 'launch Firefox'. The app argument is the \
                app's common name. Do NOT use for switching to an already-running \
                app — use switch_to_window for that.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "App name or .desktop file basename, lowercase." }
                },
                "required": ["app"]
            }
        },
        {
            "name": "switch_to_window",
            "description": "Focus an already-running application window. Use for \
                'switch to Firefox' when the app is already open. Do NOT use to \
                launch a new app.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Window class or title substring." }
                },
                "required": ["target"]
            }
        }
    ])
    .as_array()
    .expect("tools literal must be an array")
    .clone();

    tools.extend(extra_tools);

    // Anthropic prompt caching: a cache_control marker on the LAST tool
    // tells Anthropic to cache the whole prefix (system + tools). Subsequent
    // requests within the 5-minute TTL pay ~10% input-token cost on this
    // prefix and skip the preprocessing step → faster TTFT on every turn
    // after the first. The user transcript and screenshots in `messages`
    // are AFTER this breakpoint and remain uncached, so they ship fresh
    // each turn. Watch [sse-debug] message_start usage:
    //   cache_creation_input_tokens > 0 on first turn (write)
    //   cache_read_input_tokens > 0 on subsequent turns (hit)
    if let Some(last) = tools.last_mut() {
        if let Some(obj) = last.as_object_mut() {
            obj.insert(
                "cache_control".to_string(),
                serde_json::json!({ "type": "ephemeral" }),
            );
        }
    }

    serde_json::Value::Array(tools)
}

/// Trim image data from `tool_result` blocks older than the most recent
/// `keep_last_n` screenshots. Replaces the image with a text placeholder
/// so Claude knows there WAS a screenshot at that point, but the bytes
/// are gone. Keeps the conversation graph intact while controlling cost.
pub(super) fn trim_old_screenshots(messages: &mut [serde_json::Value], keep_last_n: usize) {
    let placeholder = || {
        serde_json::json!({
            "type": "text",
            "text": "[older screenshot omitted]"
        })
    };
    let mut seen = 0usize;
    for msg in messages.iter_mut().rev() {
        if msg["role"] != "user" {
            continue;
        }
        let Some(content) = msg["content"].as_array_mut() else {
            continue;
        };
        for block in content.iter_mut() {
            // Two image shapes show up in user messages:
            //   1. Direct image block on the initial transcript turn:
            //      {"type": "image", "source": {...}}
            //   2. Image inside a tool_result content array on each loop
            //      iteration:
            //      {"type": "tool_result", "content": [{"type": "image", ...}]}
            // Both count toward the "N most recent screenshots" budget so
            // the initial transcript image doesn't live forever and bloat
            // the body indefinitely.
            if block["type"] == "image" {
                if seen < keep_last_n {
                    seen += 1;
                } else {
                    *block = placeholder();
                }
                continue;
            }
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(inner) = block["content"].as_array_mut() else {
                continue;
            };
            for item in inner.iter_mut() {
                if item["type"] == "image" {
                    if seen < keep_last_n {
                        seen += 1;
                    } else {
                        *item = placeholder();
                    }
                }
            }
        }
    }
}

/// Dispatch a completed `tool_use` block to the corresponding `Action`
/// variant. Returns None if the tool name is unknown or the input shape
/// doesn't match (e.g. `computer` with an action other than `left_click`,
/// or a custom tool missing its required field). Each Some(_) is ready to
/// hand to the caller's `on_action` callback.
#[allow(clippy::too_many_arguments)]
pub(super) fn parse_tool_call(
    tool_name: &str,
    input: &serde_json::Value,
    declared_w: u32,
    declared_h: u32,
    window_x: i64,
    window_y: i64,
    window_width: i64,
    window_height: i64,
) -> Option<Action> {
    // Claude sometimes emits malformed tool calls where the tool NAME is
    // the action (e.g. `name: "left_click"`) instead of the proper
    // `name: "computer", input.action: "left_click"`. Detect both shapes
    // and normalize to a single (action_name, input) pair before matching.
    let (effective_action, effective_input) = match tool_name {
        "computer" => (input["action"].as_str()?.to_string(), input),
        // The action-as-name fallback. Coordinate / text comes straight
        // from input without an `action` field.
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click"
        | "mouse_move" | "type" | "key" | "scroll" | "screenshot" | "wait" | "cursor_position" => {
            (tool_name.to_string(), input)
        }
        // Custom tools handled below.
        _ => return parse_custom_tool(tool_name, input),
    };

    let action = effective_action.as_str();

    // Text-only actions (no coordinate).
    if action == "type" {
        let text = effective_input["text"].as_str()?;
        return Some(Action::Type {
            text: text.to_string(),
        });
    }
    if action == "key" {
        let key = effective_input["text"].as_str()?;
        return Some(Action::Key {
            key: key.to_string(),
        });
    }
    if action == "scroll" {
        let direction = effective_input["scroll_direction"]
            .as_str()
            .unwrap_or("down")
            .to_string();
        // scroll_amount may arrive as integer or stringified integer.
        let amount = effective_input["scroll_amount"]
            .as_u64()
            .or_else(|| {
                effective_input["scroll_amount"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .unwrap_or(3) as u32;
        return Some(Action::Scroll { direction, amount });
    }

    // Coordinate actions. Accept either a JSON array [x, y] OR a JSON
    // string like "[640, 47]" — Claude's malformed shape emits the latter.
    let (raw_x, raw_y) = extract_coordinate(&effective_input["coordinate"])?;
    let raw_x = raw_x.clamp(0, declared_w as i64 - 1);
    let raw_y = raw_y.clamp(0, declared_h as i64 - 1);
    let sx = window_x + (raw_x as f64 * window_width as f64 / declared_w as f64) as i64;
    let sy = window_y + (raw_y as f64 * window_height as f64 / declared_h as f64) as i64;
    let x = sx.clamp(window_x, window_x + window_width - 1);
    let y = sy.clamp(window_y, window_y + window_height - 1);

    match action {
        // Treat right/middle/double/triple clicks as left clicks for now —
        // most apps treat them similarly for the "I want to interact with
        // THIS element" case. We can add separate Action variants if a
        // real use case appears.
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => {
            Some(Action::Click { x, y })
        }
        "mouse_move" => Some(Action::Point { x, y }),
        _ => None,
    }
}

/// Parse a coordinate field that may be either a JSON array `[x, y]` or a
/// JSON string `"[x, y]"`. Returns (x, y) as i64.
fn extract_coordinate(value: &serde_json::Value) -> Option<(i64, i64)> {
    if let Some(arr) = value.as_array() {
        if arr.len() == 2 {
            return Some((arr[0].as_i64()?, arr[1].as_i64()?));
        }
    }
    if let Some(s) = value.as_str() {
        // Strip brackets/whitespace, split on comma.
        let trimmed = s.trim().trim_start_matches('[').trim_end_matches(']');
        let parts: Vec<&str> = trimmed.split(',').map(|p| p.trim()).collect();
        if parts.len() == 2 {
            let x = parts[0].parse::<i64>().ok()?;
            let y = parts[1].parse::<i64>().ok()?;
            return Some((x, y));
        }
    }
    None
}

/// Custom tools (open_url, launch_app, switch_to_window) PLUS the
/// integration fallback. Any tool name not in the built-in list is
/// returned as `Action::Integration` for runtime dispatch to whichever
/// integration owns it (Spotify, etc.). If no integration owns it, the
/// dispatcher logs the unknown name.
fn parse_custom_tool(tool_name: &str, input: &serde_json::Value) -> Option<Action> {
    match tool_name {
        "open_url" => input["url"]
            .as_str()
            .map(|s| Action::OpenUrl { url: s.to_string() }),
        "launch_app" => input["app"]
            .as_str()
            .map(|s| Action::LaunchApp { app: s.to_string() }),
        "switch_to_window" => input["target"].as_str().map(|s| Action::SwitchToWindow {
            target: s.to_string(),
        }),
        _ => Some(Action::Integration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anthropic prompt caching only kicks in if every request marks the
    /// SAME prefix boundary. If a future refactor strips the cache_control
    /// field off the last tool, requests still succeed but cost ~10x more
    /// and every TTFT jumps ~300-500ms — a silent performance regression.
    /// This test fails loudly if the marker disappears.
    #[test]
    fn last_tool_has_cache_control_marker() {
        let tools = tools_array_value(1280, 800, vec![]);
        let arr = tools
            .as_array()
            .expect("tools_array_value should return a JSON array");
        let last = arr
            .last()
            .expect("tools array should be non-empty");
        let cache_control = last.get("cache_control").unwrap_or_else(|| {
            panic!(
                "last tool is missing the cache_control marker — prompt caching is OFF.\nlast tool was: {}",
                serde_json::to_string_pretty(last).unwrap_or_default()
            )
        });
        assert_eq!(
            cache_control,
            &serde_json::json!({ "type": "ephemeral" }),
            "cache_control shape changed; Anthropic expects {{ type: ephemeral }}"
        );
    }

    /// Extra defense: the marker should ALSO survive integration tools
    /// being added to the array. integrations::all_tools() is the real
    /// caller, so the marker needs to land on the last appended tool, not
    /// the last hardcoded one.
    #[test]
    fn cache_control_on_last_tool_with_extras() {
        let extra = vec![
            serde_json::json!({ "name": "fake_extra_tool", "input_schema": {} }),
        ];
        let tools = tools_array_value(1280, 800, extra);
        let arr = tools.as_array().expect("array");
        let last = arr.last().expect("non-empty");
        assert_eq!(
            last.get("name").and_then(|v| v.as_str()),
            Some("fake_extra_tool"),
            "extra tool should be appended last"
        );
        assert!(
            last.get("cache_control").is_some(),
            "cache_control must land on the LAST tool after extras are appended"
        );
    }
}
