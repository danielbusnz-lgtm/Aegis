//! Side-effecting actions Claude can request via custom tools in `find_action`.
//! Each function shells out to the right host-OS command. All are fire-and-
//! forget: no waiting, no return value. Errors get logged but don't propagate
//! since these run inside the streaming SSE callback where blocking would
//! delay subsequent tokens.

use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

/// Input-injection commands serialized through one executor thread so that
/// "click then type" intents land in the right order regardless of how the
/// Claude SSE callback fires them.
enum InputCmd {
    Click { x: i64, y: i64 },
    Type { text: String },
}

static INPUT_TX: OnceLock<Sender<InputCmd>> = OnceLock::new();

/// Open a URL in the user's default browser. Validates the URL up front so
/// hallucinated junk from Claude never reaches the OS.
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
    if let Err(e) = open::that_detached(raw) {
        eprintln!("[action:open_url] open failed: {}", e);
        return;
    }

    raise_likely_browser();
}

/// Launch a desktop application by name. Tries gtk-launch first (handles
/// .desktop entries like "spotify" → spotify.desktop), falls back to spawning
/// the binary directly via shell `||`. setsid -f fully detaches so the app
/// survives if aegis exits.
pub fn launch_app(app: &str) {
    eprintln!("[action:launch_app] launching '{}'", app);
    let escaped = shell_single_quote(app);
    let cmd = format!(
        "gtk-launch {esc} 2>/dev/null || exec {esc}",
        esc = escaped
    );
    if let Err(e) = Command::new("setsid")
        .args(["-f", "sh", "-c", &cmd])
        .spawn()
    {
        eprintln!("[action:launch_app] spawn failed: {}", e);
    }
}

/// Focus an already-running window. Tries class match first, then title
/// match a moment later as a fallback (non-matches fail silently in hyprctl).
pub fn switch_to_window(target: &str) {
    eprintln!("[action:switch_to_window] focusing '{}'", target);
    let _ = Command::new("hyprctl")
        .args(["dispatch", "focuswindow", &format!("class:{}", target)])
        .spawn();
    let target = target.to_string();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        let _ = Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &format!("title:{}", target)])
            .spawn();
    });
}

/// Hyprland blocks focus-stealing per the XDG activation protocol, so a new
/// browser tab opens but the window doesn't come forward. Dispatch focus to
/// every common browser class after a short delay; non-matches no-op.
fn raise_likely_browser() {
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(300));
        for class in &["firefox", "Chromium", "Brave-browser", "Google-chrome", "chromium"] {
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

/// Start the single-threaded input executor. Must be called exactly once,
/// at startup, before any click or type can fire. Subsequent calls no-op.
pub fn init_input_executor() {
    let (tx, rx) = channel::<InputCmd>();
    if INPUT_TX.set(tx).is_err() {
        return;
    }
    thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                InputCmd::Click { x, y } => exec_click(x, y),
                InputCmd::Type { text } => exec_type(&text),
            }
        }
    });
}

fn exec_click(x: i64, y: i64) {
    let move_status = Command::new("ydotool")
        .args([
            "mousemove",
            "--absolute",
            "-x",
            &x.to_string(),
            "-y",
            &y.to_string(),
        ])
        .status();
    if let Err(e) = move_status {
        eprintln!("[action:click] mousemove failed: {}", e);
        return;
    }
    // Tiny gap so the move lands at the OS level before the click
    // registers — most apps debounce events within ~10ms.
    thread::sleep(Duration::from_millis(30));
    // 0xC0 = BTN_LEFT down + up combined (ydotool's click encoding).
    if let Err(e) = Command::new("ydotool").args(["click", "0xC0"]).status() {
        eprintln!("[action:click] click failed: {}", e);
    }
}

fn exec_type(text: &str) {
    // Brief settle after any focus-changing action that came before
    // (e.g. a click on a search bar). Without it, the first few
    // keystrokes can land before the field is ready.
    thread::sleep(Duration::from_millis(80));
    // `--` to separate the text from ydotool flags in case it starts with -.
    if let Err(e) = Command::new("ydotool")
        .args(["type", "--", text])
        .status()
    {
        eprintln!("[action:type] type failed: {}", e);
    }
}

/// Startup probe: warn if ydotool isn't installed or the daemon isn't
/// reachable, so the user knows clicks will silently no-op. Doesn't fail
/// startup — pointing/opening/launching still work without it.
pub fn check_input_injection_available() {
    match Command::new("ydotool").arg("--version").output() {
        Ok(o) if o.status.success() => {
            eprintln!("[startup] ydotool available — click actions will fire real input");
        }
        _ => {
            eprintln!(
                "[startup] WARNING: ydotool not found on PATH. Click actions will move the\n\
                 \toverlay but NOT inject a real click. To enable:\n\
                 \t  sudo pacman -S ydotool   # (or apt/dnf equivalent)\n\
                 \t  sudo usermod -aG input $USER\n\
                 \t  systemctl --user enable --now ydotool.service"
            );
        }
    }
}
