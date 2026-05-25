//! Side-effecting actions Claude can request via custom tools in `find_action`.
//! Each function shells out to the right host-OS command. All are fire-and-
//! forget: no waiting, no return value. Errors get logged but don't propagate
//! since these run inside the streaming SSE callback where blocking would
//! delay subsequent tokens.
//!
//! Input synthesis (click/type/key/scroll) is platform-divergent and lives in
//! the `crate::input` module; this file owns the serialized executor that
//! calls into it, plus the window/app-management actions.

use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

/// Commands serialized through one executor thread to preserve ordering
/// across async SSE callbacks (e.g. click-then-type must stay in that
/// order even though both arrive via the same callback fast enough to
/// race ydotool's per-call latency).
enum InputCmd {
    /// Moves the OS cursor to (x, y) and fires a left button down+up.
    Click { x: i64, y: i64 },
    /// Trailing `\n` in `text` submits (fires Enter after typing).
    Type { text: String },
    /// `combo` is human syntax like "Return", "ctrl+a".
    Key { combo: String },
    /// `amount` is wheel-clicks; mapped to arrow-key presses by the backend.
    Scroll { direction: String, amount: u32 },
}

/// Set by `init_input_executor` at startup. OnceLock makes the static
/// callable from anywhere without plumbing and makes double-init a no-op.
static INPUT_TX: OnceLock<Sender<InputCmd>> = OnceLock::new();

/// Open a URL in the user's currently-focused browser when possible, falling
/// back to xdg-open. Priority:
///   1. `AEGIS_BROWSER` env var: force a specific binary.
///   2. Hyprland's currently-focused window, if it's a Chromium-family
///      browser (Chrome, Brave, Chromium, Edge, Vivaldi). Chromium-family
///      can be invoked directly without D-Bus session issues.
///   3. xdg-open: uses the system default browser. Necessary for Firefox
///      since direct `firefox <url>` calls hang on D-Bus when aegis isn't
///      in the user session.
pub fn open_url(raw: &str) {
    let parsed = match url::Url::parse(raw) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[action:open_url] rejecting '{}': {}", raw, e);
            return;
        }
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        eprintln!(
            "[action:open_url] rejecting non-http scheme '{}'",
            parsed.scheme()
        );
        return;
    }

    eprintln!("[action:open_url] opening {}", raw);

    if let Ok(forced) = std::env::var("AEGIS_BROWSER") {
        eprintln!("[action:open_url] AEGIS_BROWSER override → {}", forced);
        if let Err(e) = Command::new(&forced).arg(raw).spawn() {
            eprintln!("[action:open_url] AEGIS_BROWSER spawn failed: {}", e);
        }
        raise_likely_browser();
        return;
    }

    if let Some(bin) = focused_browser_binary() {
        eprintln!(
            "[action:open_url] focused window is {} → routing there",
            bin
        );
        if let Err(e) = Command::new(&bin).arg(raw).spawn() {
            eprintln!(
                "[action:open_url] direct browser spawn failed ({}), falling back to xdg-open: {}",
                bin, e
            );
            let _ = open::that_detached(raw);
        }
        raise_likely_browser();
        return;
    }

    eprintln!("[action:open_url] no Chromium-family browser focused → xdg-open (default)");
    if let Err(e) = open::that_detached(raw) {
        eprintln!("[action:open_url] xdg-open failed: {}", e);
        return;
    }

    raise_likely_browser();
}

/// List the distinct window classes of all currently-mapped Hyprland
/// clients. Used to inject "what apps are open right now" context into
/// the agent loop's prompt so Claude can prefer switching to a running
/// app over launching/web-versioning it. Empty Vec on any failure.
pub fn list_running_apps() -> Vec<String> {
    let Ok(output) = Command::new("hyprctl").args(["clients", "-j"]).output() else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    let Ok(arr) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return vec![];
    };
    let Some(clients) = arr.as_array() else {
        return vec![];
    };
    let mut classes: Vec<String> = clients
        .iter()
        .filter_map(|c| c["class"].as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    classes.sort();
    classes.dedup();
    classes
}

/// Query Hyprland for the currently-focused window's class. If that class
/// maps to a Chromium-family browser AND the corresponding binary exists
/// on PATH, return the binary name. Returns None for Firefox (intentionally,
/// since direct calls hang on D-Bus) and for non-browser windows.
fn focused_browser_binary() -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let window: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let class = window["class"].as_str()?.to_lowercase();

    let candidates: &[&str] = match class.as_str() {
        // Firefox-family deliberately not routed directly. Defer to xdg-open
        // so it goes through the session's lock-file + D-Bus handshake.
        "firefox" | "firefox-esr" | "librewolf" | "waterfox" | "zen" => return None,
        "chromium" | "chromium-browser" => &["chromium"],
        "google-chrome" | "google-chrome-stable" | "chrome" => {
            &["google-chrome-stable", "google-chrome", "chrome"]
        }
        "brave-browser" | "brave-browser-stable" | "brave" => &["brave-browser", "brave"],
        "vivaldi-stable" | "vivaldi" => &["vivaldi-stable", "vivaldi"],
        "microsoft-edge" | "microsoft-edge-stable" | "msedge" => {
            &["microsoft-edge-stable", "microsoft-edge", "msedge"]
        }
        _ => return None,
    };

    candidates
        .iter()
        .find(|bin| binary_on_path(bin))
        .map(|s| s.to_string())
}

/// True iff `which <bin>` succeeds. Output suppressed because misses are
/// expected (caller probes several candidates).
fn binary_on_path(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Launch a desktop application by name. Tries gtk-launch first (handles
/// .desktop entries like "spotify" → spotify.desktop), falls back to spawning
/// the binary directly via shell `||`. setsid -f fully detaches so the app
/// survives if aegis exits.
pub fn launch_app(app: &str) {
    eprintln!("[action:launch_app] launching '{}'", app);
    let escaped = shell_single_quote(app);
    let cmd = format!("gtk-launch {esc} 2>/dev/null || exec {esc}", esc = escaped);
    if let Err(e) = Command::new("setsid")
        .args(["-f", "sh", "-c", &cmd])
        .spawn()
    {
        eprintln!("[action:launch_app] spawn failed: {}", e);
    }
}

/// Focuses a window matching `target` as a class first, then as a title
/// substring 150ms later. Claude's `target` is whatever the user said,
/// which is ambiguous between class names ("firefox") and title text
/// ("Inbox"), so we try both. Non-matches fail silently in hyprctl.
pub fn switch_to_window(target: &str) {
    eprintln!("[action:switch_to_window] focusing '{}'", target);
    let _ = Command::new("hyprctl")
        .args(["dispatch", "focuswindow", &format!("class:{}", target)])
        .spawn();
    let target = target.to_string();
    thread::spawn(move || {
        // > hyprctl dispatch round-trip (~30ms) so the class attempt
        // resolves first; < perceptual instant.
        thread::sleep(Duration::from_millis(150));
        let _ = Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &format!("title:{}", target)])
            .spawn();
    });
}

/// Works around Hyprland's XDG-activation focus-steal block by dispatching
/// focuswindow at every common browser class after the new window is
/// likely to exist. Misses no-op.
fn raise_likely_browser() {
    thread::spawn(|| {
        // Below ~300ms the focuswindow dispatch can race the browser's
        // window creation, leaving Hyprland with no matching client.
        thread::sleep(Duration::from_millis(300));
        for class in &[
            "firefox",
            "Chromium",
            "Brave-browser",
            "Google-chrome",
            "chromium",
        ] {
            let _ = Command::new("hyprctl")
                .args(["dispatch", "focuswindow", &format!("class:{}", class)])
                .spawn();
        }
    });
}

/// Single-quote escape for shell -c. Replaces ' with '\'' (close, escape, reopen).
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Move the system mouse to (x, y) and synthesize a left-button click.
/// Enqueues onto the input executor; returns immediately. The overlay
/// animation (GTK thread) and the click work (executor thread) run in
/// parallel so they look simultaneous to the user, but click-then-type
/// sequences within the executor stay strictly ordered.
pub fn click_at(x: i64, y: i64) {
    eprintln!("[action:click_at] queueing click at ({}, {})", x, y);
    enqueue(InputCmd::Click { x, y });
}

/// Type text into the currently-focused field. Embed a trailing \n in
/// `text` if you want Enter to fire after (search submission, message
/// send). Enqueues onto the input executor so it always lands after any
/// pending click that came before it.
pub fn type_text(text: &str) {
    eprintln!("[action:type_text] queueing type ({} chars)", text.len());
    enqueue(InputCmd::Type {
        text: text.to_string(),
    });
}

/// Press a key or key combination ("Return", "Tab", "Escape", "ctrl+a",
/// "ctrl+f", etc.). Enqueues onto the input executor so it's serialized
/// against pending clicks and types.
pub fn press_key(combo: &str) {
    eprintln!("[action:press_key] queueing key '{}'", combo);
    enqueue(InputCmd::Key {
        combo: combo.to_string(),
    });
}

/// Scroll by sending repeated arrow-key presses. Wayland has no clean
/// "scroll at point" primitive, so we approximate with keyboard scrolling.
/// Works in browsers, terminals, file managers, anywhere arrow keys move
/// the viewport. The `amount` parameter is roughly "wheel clicks".
pub fn scroll(direction: &str, amount: u32) {
    eprintln!("[action:scroll] queueing scroll {} × {}", direction, amount);
    enqueue(InputCmd::Scroll {
        direction: direction.to_string(),
        amount,
    });
}

/// Drops the command (loud log) if `init_input_executor` hasn't run.
/// Indicates a startup-order regression, not normal flow.
fn enqueue(cmd: InputCmd) {
    match INPUT_TX.get() {
        Some(tx) => {
            let _ = tx.send(cmd);
        }
        None => {
            eprintln!(
                "[action] input executor not initialized; \
                 dropping command. Call init_input_executor() at startup."
            );
        }
    }
}

/// Starts the single-threaded input executor. Must be called once at
/// startup, before any click/type/key/scroll can fire. Idempotent: a
/// second call is silently ignored. Each command dispatches to the
/// OS-specific backend in `crate::input`.
pub fn init_input_executor() {
    let (tx, rx) = channel::<InputCmd>();
    if INPUT_TX.set(tx).is_err() {
        return;
    }
    thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                InputCmd::Click { x, y } => crate::input::exec_click(x, y),
                InputCmd::Type { text } => crate::input::exec_type(&text),
                InputCmd::Key { combo } => crate::input::exec_key(&combo),
                InputCmd::Scroll { direction, amount } => {
                    crate::input::exec_scroll(&direction, amount)
                }
            }
        }
    });
}

/// Startup probe: check whether input injection is available on this platform.
/// Delegates to the OS-specific backend in `crate::input`. Never fails startup;
/// pointing, opening URLs, and launching apps still work without it.
pub fn check_input_injection_available() {
    crate::input::check_available();
}
