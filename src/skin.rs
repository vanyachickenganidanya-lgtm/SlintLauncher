//! Turn a Minecraft skin PNG into a small head icon for the UI.

use anyhow::{anyhow, Result};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const SCALE: u32 = 8; // 8x8 head -> 64x64 icon

/// Crop the 8x8 head (plus the hat overlay) from a skin and upscale it.
pub fn head_from_skin(png: &[u8]) -> Result<Image> {
    let img = image::load_from_memory(png)
        .map_err(|e| anyhow!("cannot decode skin: {e}"))?
        .to_rgba8();

    if img.width() < 64 || img.height() < 32 {
        return Err(anyhow!("unexpected skin size {}x{}", img.width(), img.height()));
    }

    let size = 8u32;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(size * SCALE, size * SCALE);
    let width = buffer.width();
    let pixels = buffer.make_mut_slice();

    for y in 0..size * SCALE {
        for x in 0..size * SCALE {
            let sx = x / SCALE;
            let sy = y / SCALE;

            // base head at (8,8), hat overlay at (40,8)
            let base = img.get_pixel(8 + sx, 8 + sy).0;
            let hat = img.get_pixel(40 + sx, 8 + sy).0;

            let rgba = if hat[3] > 16 { hat } else { base };
            pixels[(y * width + x) as usize] = Rgba8Pixel {
                r: rgba[0],
                g: rgba[1],
                b: rgba[2],
                a: 255,
            };
        }
    }

    Ok(Image::from_rgba8(buffer))
}
