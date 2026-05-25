//! Push-to-talk hotkey wiring. Backend selected at compile time by target OS.
//!
//! Callers use the free functions directly; the backend is an implementation
//! detail. Adding a backend requires only a file implementing `HotkeyBackend`
//! plus a `mod` + type-alias arm below; no call site changes.

mod backend;
pub use backend::HotkeyBackend;

#[cfg(all(target_os = "linux", not(feature = "force-crossplatform")))]
mod hyprland;
#[cfg(any(not(target_os = "linux"), feature = "force-crossplatform"))]
mod crossplatform;

// Linux uses the native Hyprland backend; everything else (and Linux under the
// force-crossplatform dev override) uses the portable backend. The two cfgs are
// mutually exclusive and exhaustive, so exactly one Active is always defined.
#[cfg(all(target_os = "linux", not(feature = "force-crossplatform")))]
type Active = hyprland::Backend;
#[cfg(any(not(target_os = "linux"), feature = "force-crossplatform"))]
type Active = crossplatform::Backend;

/// Start listening for the push-to-talk key.
///
/// # Errors
///
/// Propagates any setup error from the active backend.
pub fn init() -> std::io::Result<()> {
    Active::init()
}

/// Drain pending hotkey events into the recording state. No-op on backends
/// with an independent listener thread. Call once per main-loop iteration.
///
/// `allow(dead_code)`: only the polling backend's build calls this.
#[allow(dead_code)]
pub fn poll() {
    Active::poll();
}

/// True while the hotkey is held.
pub fn is_recording() -> bool {
    Active::is_recording()
}

/// Block the calling thread until the hotkey is pressed.
pub fn wait_for_press() {
    Active::wait_for_press();
}

/// Register a callback fired on press. Call before [`init`].
///
/// `allow(dead_code)`: only the signal backend's build calls this.
#[allow(dead_code)]
pub fn on_press(f: impl Fn() + Send + Sync + 'static) {
    Active::on_press(Box::new(f));
}

/// Register a callback fired on release. Call before [`init`].
#[allow(dead_code)]
pub fn on_release(f: impl Fn() + Send + Sync + 'static) {
    Active::on_release(Box::new(f));
}
