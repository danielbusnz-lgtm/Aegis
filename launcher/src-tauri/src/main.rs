#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;

/// Platform-specific onboarding URL. Each OS has its own HTML page with the
/// right hotkey instructions. macOS uses Ctrl+Space; everyone else uses the
/// Insert key (matches the Hyprland config).
#[cfg(target_os = "macos")]
const ONBOARDING_URL: &str = "onboarding/macos.html";
#[cfg(not(target_os = "macos"))]
const ONBOARDING_URL: &str = "onboarding/index.html";

/// Launch the actual aegis cursor + voice agent as a child process. Looks for
/// the binary in the workspace's debug then release target dirs; for shipped
/// builds we'd bundle it next to the launcher instead.
#[tauri::command]
fn spawn_aegis() -> Result<(), String> {
    let candidates = ["target/debug/aegis", "target/release/aegis"];
    for path in candidates {
        if Command::new(path).spawn().is_ok() {
            return Ok(());
        }
    }
    Err("aegis binary not found in target/debug or target/release. Build it with `cargo build -p aegis` first.".to_string())
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
