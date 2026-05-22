#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;

/// Platform-specific onboarding URL. Each OS has its own HTML page with the
/// right hotkey instructions. macOS uses Ctrl+Space; everyone else uses the
/// Insert key (matches the Hyprland config).
#[cfg(target_os = "macos")]
const ONBOARDING_URL: &str = "onboarding/macos.html";
#[cfg(not(target_os = "macos"))]
const ONBOARDING_URL: &str = "onboarding/index.html";

/// Launch the actual aegis cursor + voice agent as a child process.
///
/// Path lookup order:
/// 1. `../../target/{debug,release}/aegis`: workspace dev layout, the case
///    when `cargo tauri dev` runs from `launcher/` and the launcher binary
///    has cwd of `launcher/src-tauri/`.
/// 2. `target/{debug,release}/aegis`: workspace root cwd (e.g. someone
///    launches the launcher binary directly from `/Projects/aegis/`).
/// 3. `./aegis`: sibling-of-binary layout, used by shipped `.app` bundles
///    where both launcher and aegis live in `Contents/MacOS/`.
#[tauri::command]
fn spawn_aegis() -> Result<(), String> {
    let candidates = [
        "../../target/debug/aegis",
        "../../target/release/aegis",
        "target/debug/aegis",
        "target/release/aegis",
        "./aegis",
    ];
    for path in candidates {
        if std::path::Path::new(path).exists() {
            if let Ok(_child) = Command::new(path).spawn() {
                return Ok(());
            }
        }
    }
    Err(format!(
        "aegis binary not found. Tried: {}. Build it with \
         `cargo build -p aegis --no-default-features --features winit-window,crossplatform` first.",
        candidates.join(", ")
    ))
}

fn main() {
    // webkit2gtk's DMABUF renderer crashes against Hyprland and several
    // other Wayland compositors with "Error 71 (Protocol error)". Disabling
    // it forces a software path that works everywhere. Harmless on non-Linux
    // platforms but gated since the env var only exists on Linux.
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![spawn_aegis])
        .setup(|app| {
            // Onboarding window is created here (not in tauri.conf.json) so
            // we can pick the URL at compile time per target OS. welcome.js
            // finds it later via `WebviewWindow.getByLabel("onboarding")`.
            tauri::WebviewWindowBuilder::new(
                app,
                "onboarding",
                tauri::WebviewUrl::App(ONBOARDING_URL.into()),
            )
            .title("Aegis")
            .inner_size(600.0, 350.0)
            .resizable(false)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .transparent(true)
            .visible(false)
            .focused(false)
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running launcher");
}
