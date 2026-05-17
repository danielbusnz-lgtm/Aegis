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

/// Open a URL in the user's currently-focused browser when possible, falling
/// back to xdg-open. Priority:
///   1. `AEGIS_BROWSER` env var — force a specific binary.
///   2. Hyprland's currently-focused window, if it's a Chromium-family
///      browser (Chrome, Brave, Chromium, Edge, Vivaldi). Chromium-family
///      can be invoked directly without D-Bus session issues.
///   3. xdg-open — uses the system default browser. Necessary for Firefox
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
        eprintln!("[action:open_url] focused window is {} → routing there", bin);
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
        // Firefox-family deliberately not routed directly — defer to xdg-open
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
