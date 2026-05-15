use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hyprland::data::{Monitors, Workspace};
use hyprland::shared::{HyprData, HyprDataActive};
use image::ImageReader;
use image::codecs::jpeg::JpegEncoder;
use std::io::Cursor;
use std::process::Command;

/// Returns the geometry (x, y, width, height) of the monitor showing the
/// currently active workspace, without capturing a screenshot. Used by
/// callers that delegate the capture step (e.g., `detect_element_location`).
pub fn active_workspace_geometry()
-> Result<(i32, i32, u32, u32), Box<dyn std::error::Error>> {
    let active = Workspace::get_active()?;
    let monitor = Monitors::get()?
        .into_iter()
        .find(|m| m.active_workspace.id == active.id)
        .ok_or("no monitor for active workspace")?;
    Ok((
        monitor.x,
        monitor.y,
        monitor.width as u32,
        monitor.height as u32,
    ))
}

/// Captures a screen region with grim, encodes as JPEG q85, and returns base64
/// plus the captured dimensions. Resize-to-Computer-Use is commented out for
/// now — image is returned at native resolution.
pub fn capture_for_claude(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(String, u32, u32), Box<dyn std::error::Error>> {
    let geometry = format!("{},{} {}x{}", x, y, width, height);
    let output = Command::new("grim").args(["-g", &geometry, "-"]).output()?;

    if !output.status.success() {
        return Err(format!("grim failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let img = image::load_from_memory(&output.stdout)?;
    // let img = img.resize_exact(declared_w, declared_h, FilterType::Lanczos3);

    let mut jpeg: Vec<u8> = Vec::new();
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, 85))?;
    Ok((BASE64.encode(&jpeg), width as u32, height as u32))
}

/// Pick one of three aspect-matched resolutions Anthropic recommends for
/// Computer Use. Matching the input's aspect ratio avoids stretching that
/// degrades coordinate accuracy. Ported from Tabby.
pub fn pick_declared_resolution(window_width: i64, window_height: i64) -> (u32, u32) {
    let ratio = window_width as f64 / window_height.max(1) as f64;
    let candidates: [(u32, u32, f64); 3] = [
        (1024, 768, 4.0 / 3.0),
        (1280, 800, 16.0 / 10.0),
        (1366, 768, 16.0 / 9.0),
    ];
    let mut best = candidates[1];
    let mut smallest_diff = f64::INFINITY;
    for (w, h, ar) in candidates {
        let diff = (ratio - ar).abs();
        if diff < smallest_diff {
            smallest_diff = diff;
            best = (w, h, ar);
        }
    }
    (best.0, best.1)
}

/// Decode an existing base64 JPEG, resize to exactly the declared dimensions
/// with SIMD-accelerated bilinear filtering, and re-encode as JPEG q85. Used
/// by Computer Use callers so Claude's returned coordinates can be scaled
/// back accurately. Ported from Tabby.
///
/// Was using `image::imageops::resize` with FilterType::Triangle which is
/// single-threaded scalar code: ~1.7s for a 4K screenshot down to 1280×800
/// on a desktop CPU. Switching to `fast_image_resize` (SIMD via AVX2/SSE4.1
/// on x86_64, NEON on aarch64) gets the same bilinear result in ~80–200ms.
/// On short voice turns this saves more than a second of dead wait.
pub fn resize_jpeg_for_computer_use(
    src_b64: &str,
    target_w: u32,
    target_h: u32,
) -> Result<String, Box<dyn std::error::Error>> {
    use fast_image_resize::images::Image as FirImage;
    use fast_image_resize::{
        FilterType as FirFilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    };

    let bytes = BASE64.decode(src_b64.as_bytes())?;
    let src_dyn = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;

    // Force RGB8 so the resize and JPEG encode below have a consistent
    // pixel layout. JPEG doesn't carry alpha anyway, so dropping it here
    // is free and avoids fast_image_resize's pre/post alpha multiplication.
    let src_rgb = src_dyn.to_rgb8();
    let (src_w, src_h) = (src_rgb.width(), src_rgb.height());

    let fir_src = FirImage::from_vec_u8(src_w, src_h, src_rgb.into_raw(), PixelType::U8x3)?;
    let mut fir_dst = FirImage::new(target_w, target_h, PixelType::U8x3);

    // Bilinear matches the old Triangle filter; the speedup is from SIMD,
    // not from a different algorithm. Claude can't tell the difference.
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilterType::Bilinear));
    let mut resizer = Resizer::new();
    resizer.resize(&fir_src, &mut fir_dst, &opts)?;

    let mut out: Vec<u8> = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, 85);
    encoder.encode(
        fir_dst.buffer(),
        target_w,
        target_h,
        image::ExtendedColorType::Rgb8,
    )?;

    Ok(BASE64.encode(&out))
}
