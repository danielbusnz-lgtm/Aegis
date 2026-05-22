//! macOS-specific window configuration for the cursor overlay.
//!
//! Handles the platform quirks that make transparent overlays work on macOS:
//! - Manual window sizing (fullscreen mode kills transparency)
//! - NSWindow/CALayer opacity configuration via Objective-C runtime
//! - Retina display scale factor adjustments

use std::sync::Arc;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

/// Size the window to fill the primary monitor without entering fullscreen.
/// On macOS, `Fullscreen::Borderless` puts the window in its own Space and
/// forces it opaque, killing the transparency we set via `with_transparent(true)`.
pub fn configure_window_size(event_loop: &ActiveEventLoop, window: &Window) {
    if let Some(monitor) = event_loop.primary_monitor() {
        let size = monitor.size();
        let pos = monitor.position();
        let _ = window.request_inner_size(size);
        window.set_outer_position(pos);
    }
}

/// Force every layer in the window's hierarchy non-opaque.
/// softbuffer adds its own CALayer during `Surface::new` which defaults to opaque;
/// without this the entire screen renders black even though the window is
/// set to transparent.
///
/// # Safety
/// Uses raw Objective-C messaging to configure NSWindow and CALayer properties.
/// Only call this on macOS after the window and surface have been created.
pub unsafe fn configure_transparency(window: &Arc<Window>) {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};

    // Get NSWindow via raw-window-handle API (winit 0.30+)
    // The handle provides ns_view, so we get the window from the view.
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit_handle) = handle.as_raw() else {
        return;
    };
    let ns_view = appkit_handle.ns_view.as_ptr() as *mut AnyObject;
    if ns_view.is_null() {
        return;
    }
    // Get NSWindow from NSView via [view window]
    let ns_window: *mut AnyObject = msg_send![ns_view, window];
    if ns_window.is_null() {
        return;
    }

    // NSWindow: setOpaque:NO and backgroundColor = [NSColor clearColor]
    let _: () = msg_send![ns_window, setOpaque: Bool::NO];
    let ns_color_class = objc2::class!(NSColor);
    let clear_color: *mut AnyObject = msg_send![ns_color_class, clearColor];
    let _: () = msg_send![ns_window, setBackgroundColor: clear_color];

    // contentView: ensure layer-backed and the root layer is non-opaque
    let content_view: *mut AnyObject = msg_send![ns_window, contentView];
    if content_view.is_null() {
        return;
    }

    let _: () = msg_send![content_view, setWantsLayer: Bool::YES];
    let layer: *mut AnyObject = msg_send![content_view, layer];
    if layer.is_null() {
        return;
    }

    let _: () = msg_send![layer, setOpaque: Bool::NO];

    // Walk softbuffer's sublayers (added by Surface::new) and force each non-opaque
    let sublayers: *mut AnyObject = msg_send![layer, sublayers];
    if sublayers.is_null() {
        return;
    }

    let count: usize = msg_send![sublayers, count];
    for i in 0..count {
        let sub: *mut AnyObject = msg_send![sublayers, objectAtIndex: i];
        if !sub.is_null() {
            let _: () = msg_send![sub, setOpaque: Bool::NO];
        }
    }
}

/// Scale logical cursor coordinates to physical pixels for Retina displays.
/// The `mouse_position` crate returns logical points on macOS but physical
/// pixels on X11, so we scale only on macOS. The canvas is sized in physical
/// pixels (`window.inner_size()` returns physical), so without this the sprite
/// renders in the upper-left quadrant on Retina displays.
pub fn scale_cursor_position(window: &Window, pos: (f64, f64)) -> (f64, f64) {
    let sf = window.scale_factor();
    (pos.0 * sf, pos.1 * sf)
}
