use hyprland::data::CursorPosition;
use hyprland::shared::HyprData;

/// Cursor position in absolute screen pixels via Hyprland IPC.
pub fn mouse_movement() -> hyprland::Result<(i64, i64)> {
    let pos = CursorPosition::get()?;
    Ok((pos.x, pos.y))
}
