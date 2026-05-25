use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::codecs::jpeg::JpegEncoder;

use super::backend::ScreenshotBackend;

/// Cross-platform backend (Windows, macOS, X11) via the `xcap` crate.
/// Zero-sized; never instantiated.
pub struct Backend;

impl ScreenshotBackend for Backend {
    /// Geometry of the primary monitor. There is no consistent "active
    /// workspace" concept across Mac/Windows/Linux, so we use the primary.
    fn active_workspace_geometry()
    -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        let monitors = ::xcap::Monitor::all()?;
        let monitor = monitors
            .into_iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .ok_or("no primary monitor")?;
        Ok((
            monitor.x()?,
            monitor.y()?,
            monitor.width()?,
            monitor.height()?,
        ))
    }

    fn capture_resized_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        target_w: u32,
        target_h: u32,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use fast_image_resize::images::Image as FirImage;
        use fast_image_resize::{
            FilterType as FirFilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
        };

        let monitor = ::xcap::Monitor::from_point(x, y)?;
        let local_x = (x - monitor.x()?).max(0) as u32;
        let local_y = (y - monitor.y()?).max(0) as u32;
        let image = monitor.capture_region(local_x, local_y, width as u32, height as u32)?;

        // xcap gives us an RGBA buffer directly. Convert to RGB for the resize.
        let rgba = image.into_raw();
        let mut rgb: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
        for chunk in rgba.chunks_exact(4) {
            rgb.push(chunk[0]);
            rgb.push(chunk[1]);
            rgb.push(chunk[2]);
        }

        let fir_src = FirImage::from_vec_u8(width as u32, height as u32, rgb, PixelType::U8x3)?;
        let mut fir_dst = FirImage::new(target_w, target_h, PixelType::U8x3);
        let opts =
            ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilterType::Bilinear));
        let mut resizer = Resizer::new();
        resizer.resize(&fir_src, &mut fir_dst, &opts)?;

        let mut out: Vec<u8> = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 85).encode(
            fir_dst.buffer(),
            target_w,
            target_h,
            image::ExtendedColorType::Rgb8,
        )?;
        Ok(BASE64.encode(&out))
    }
}

impl Backend {
    /// Capture a region, encode as JPEG q85, return base64 plus captured
    /// dimensions. Not part of the `ScreenshotBackend` contract: only the
    /// `demo_win` binary calls it, and grim has no equivalent. Kept here so
    /// the demo and the main pipeline share one screenshot module.
    #[allow(dead_code)]
    pub fn capture_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<(String, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        let monitor = ::xcap::Monitor::from_point(x, y)?;
        // capture_region takes monitor-local coords, so subtract monitor origin.
        let local_x = (x - monitor.x()?).max(0) as u32;
        let local_y = (y - monitor.y()?).max(0) as u32;
        let image = monitor.capture_region(local_x, local_y, width as u32, height as u32)?;

        let mut jpeg: Vec<u8> = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut jpeg, 85);
        image.write_with_encoder(encoder)?;
        Ok((BASE64.encode(&jpeg), width as u32, height as u32))
    }
}
