//! Transcript classification heuristics. Pure functions over the STT output
//! that decide how the orchestrator should handle a turn: whether the query
//! needs the screen attached, and whether the answer should be spoken back.

/// True when the transcript clearly needs the current screen as visual
/// context. Used as the gate for attaching the initial screenshot to
/// step 1 of the agent loop.
///
/// Default is "no screenshot" because most queries (identity, conversation,
/// integration commands, general questions) don't need pixels. Sending the
/// screenshot anyway costs ~700ms upload and ~1500 input tokens per turn,
/// and biases Claude toward narrating what's on screen when the user
/// didn't ask. Only attach when the query has a strong visual signal.
///
/// If a query is visual but doesn't match these keywords, the user just
/// gets an answer that ignores the screen. Re-phrasing or following up
/// with "what's on screen" still works.
pub fn is_visual_query(transcript: &str) -> bool {
    let padded = format!(" {} ", transcript.trim().to_lowercase());
    const KEYWORDS: &[&str] = &[
        // pointing / locating
        " where is ",
        " where's ",
        " where are ",
        " point at ",
        " point to ",
        // direct interaction with UI
        " click ",
        " press ",
        " tap ",
        " select ",
        // explicit references to the screen
        " on screen",
        " on the screen",
        " on my screen",
        " what's on",
        " what is on",
        " look at this",
        " look at that",
        " what do you see",
        " describe ",
        // UI elements that imply visual targeting
        " icon",
        " button",
        " tab ",
        " menu",
    ];
    KEYWORDS.iter().any(|k| padded.contains(k))
}

/// Returns true if the user's query expects a spoken description from Claude.
/// False for "just do X" commands — pointing AND action verbs — where the
/// effect (cursor flying, browser opening, app launching) is the response
/// and TTS narration would just be noise.
///
/// Conservative: defaults to true on anything ambiguous, so we err on the
/// side of giving more info rather than less.
pub fn wants_description(transcript: &str) -> bool {
    let lower = transcript.trim().to_lowercase();
    // Strip leading conversational filler AND polite-request scaffolding
    // so phrases like "can you open up youtube please" resolve to a bare
    // "open up youtube" before the action-prefix match.
    let stripped = lower
        .trim_start_matches("um, ")
        .trim_start_matches("uh, ")
        .trim_start_matches("ok. ")
        .trim_start_matches("ok, ")
        .trim_start_matches("okay. ")
        .trim_start_matches("okay, ")
        .trim_start_matches("no. ")
        .trim_start_matches("no, ")
        .trim_start_matches("hey, ")
        .trim_start_matches("hey ")
        .trim_start_matches("can you ")
        .trim_start_matches("could you ")
        .trim_start_matches("would you ")
        .trim_start_matches("please ")
        .trim_start_matches("i want to ")
        .trim_start_matches("i'd like to ")
        .trim_start_matches("i want you to ")
        .trim_start_matches("let's ")
        .trim_start_matches("lets ")
        .trim_start_matches("just ");

    // Integration-domain queries ALWAYS want spoken answers; the API result
    // is prose Claude composes from JSON. "Open up my issues" matches the
    // "open " action prefix below but is really "tell me my issues" — so
    // short-circuit on these keywords before the action match runs.
    let integration_keywords = [
        " mail",
        " email",
        " inbox",
        " messages",
        " unread",
        " pr ",
        " prs",
        " pull request",
        " pull requests",
        " issue",
        " issues",
        " notification",
        " notifications",
        " ci ",
        " actions ",
        " github",
        " repo ",
        " repos",
    ];
    let padded = format!(" {stripped} ");
    if integration_keywords.iter().any(|k| padded.contains(k)) {
        return true;
    }

    // Phrases that are unambiguously commands, not questions. Pointing verbs
    // ("where is X", "click X") and action verbs ("open X", "launch X",
    // "switch to X", "go to X") all dispatch through find_action's tools;
    // narrating them adds latency without adding info.
    let action_starts = [
        // Pointing
        "where is",
        "where's",
        "where are",
        "click",
        "click on",
        "point at",
        "point to",
        // Actions
        "open ",
        "launch ",
        "switch to ",
        "switch ",
        "go to ",
        "navigate to ",
        "focus ",
        "start ",
        // Media / integration commands — these route to tools like
        // spotify_play, never narration.
        "play ",
        "pause",
        "resume",
        "skip",
        "next track",
        "previous track",
    ];
    !action_starts.iter().any(|p| stripped.starts_with(p))
}
