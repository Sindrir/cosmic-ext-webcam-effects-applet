// SPDX-License-Identifier: GPL-3.0

use crate::config::EffectMode;
use image::RgbImage;

/// Composite a frame with a segmentation mask and the selected effect.
pub fn composite(
    frame: &RgbImage,
    mask: &[f32],
    effect: EffectMode,
    blur_intensity: u32,
    background_image: Option<&RgbImage>,
    background_color: [u8; 3],
) -> RgbImage {
    let width = frame.width();
    let height = frame.height();

    let background = match effect {
        EffectMode::None => return frame.clone(),
        EffectMode::Blur => fast_blur_background(frame, blur_intensity),
        EffectMode::Replace => {
            if let Some(bg) = background_image {
                bg.clone()
            } else {
                create_solid_background(width, height, background_color)
            }
        }
        EffectMode::SolidColor => create_solid_background(width, height, background_color),
    };

    alpha_blend(frame, &background, mask, width, height)
}

/// Fast blur using repeated box blur (approximates Gaussian).
/// Three passes of box blur closely approximate a Gaussian blur.
fn fast_blur_background(frame: &RgbImage, intensity: u32) -> RgbImage {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width == 0 || height == 0 {
        return frame.clone();
    }

    // Map intensity 0-100 to radius 0-30
    let radius = ((intensity as f32 / 100.0) * 30.0) as usize;
    if radius == 0 {
        return frame.clone();
    }

    let raw = frame.as_raw();
    let mut src = raw.to_vec();
    let mut dst = vec![0u8; src.len()];

    // Three passes of box blur approximate Gaussian
    for _ in 0..3 {
        box_blur_horizontal(&src, &mut dst, width, height, radius);
        box_blur_vertical(&dst, &mut src, width, height, radius);
    }

    RgbImage::from_raw(width as u32, height as u32, src).expect("buffer size matches")
}

fn box_blur_horizontal(src: &[u8], dst: &mut [u8], width: usize, height: usize, radius: usize) {
    let diameter = radius * 2 + 1;
    let inv = 1.0 / diameter as f32;

    for y in 0..height {
        let row_offset = y * width * 3;
        let mut r_sum: u32 = 0;
        let mut g_sum: u32 = 0;
        let mut b_sum: u32 = 0;

        // Initialize window with left edge padding + first radius pixels
        for i in 0..=radius {
            let x = i.min(width - 1);
            let idx = row_offset + x * 3;
            let count = if i == 0 { radius + 1 } else { 1 };
            r_sum += src[idx] as u32 * count as u32;
            g_sum += src[idx + 1] as u32 * count as u32;
            b_sum += src[idx + 2] as u32 * count as u32;
        }

        for x in 0..width {
            let out_idx = row_offset + x * 3;
            dst[out_idx] = (r_sum as f32 * inv) as u8;
            dst[out_idx + 1] = (g_sum as f32 * inv) as u8;
            dst[out_idx + 2] = (b_sum as f32 * inv) as u8;

            // Add right pixel entering window
            let add_x = (x + radius + 1).min(width - 1);
            let add_idx = row_offset + add_x * 3;
            r_sum += src[add_idx] as u32;
            g_sum += src[add_idx + 1] as u32;
            b_sum += src[add_idx + 2] as u32;

            // Remove left pixel leaving window
            let rem_x = if x >= radius { x - radius } else { 0 };
            let rem_idx = row_offset + rem_x * 3;
            r_sum -= src[rem_idx] as u32;
            g_sum -= src[rem_idx + 1] as u32;
            b_sum -= src[rem_idx + 2] as u32;
        }
    }
}

fn box_blur_vertical(src: &[u8], dst: &mut [u8], width: usize, height: usize, radius: usize) {
    let diameter = radius * 2 + 1;
    let inv = 1.0 / diameter as f32;

    for x in 0..width {
        let col_offset = x * 3;
        let mut r_sum: u32 = 0;
        let mut g_sum: u32 = 0;
        let mut b_sum: u32 = 0;

        // Initialize window with top edge padding + first radius pixels
        for i in 0..=radius {
            let y = i.min(height - 1);
            let idx = y * width * 3 + col_offset;
            let count = if i == 0 { radius + 1 } else { 1 };
            r_sum += src[idx] as u32 * count as u32;
            g_sum += src[idx + 1] as u32 * count as u32;
            b_sum += src[idx + 2] as u32 * count as u32;
        }

        for y in 0..height {
            let out_idx = y * width * 3 + col_offset;
            dst[out_idx] = (r_sum as f32 * inv) as u8;
            dst[out_idx + 1] = (g_sum as f32 * inv) as u8;
            dst[out_idx + 2] = (b_sum as f32 * inv) as u8;

            // Add bottom pixel entering window
            let add_y = (y + radius + 1).min(height - 1);
            let add_idx = add_y * width * 3 + col_offset;
            r_sum += src[add_idx] as u32;
            g_sum += src[add_idx + 1] as u32;
            b_sum += src[add_idx + 2] as u32;

            // Remove top pixel leaving window
            let rem_y = if y >= radius { y - radius } else { 0 };
            let rem_idx = rem_y * width * 3 + col_offset;
            r_sum -= src[rem_idx] as u32;
            g_sum -= src[rem_idx + 1] as u32;
            b_sum -= src[rem_idx + 2] as u32;
        }
    }
}

fn create_solid_background(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
    let pixels = vec![color; (width * height) as usize];
    RgbImage::from_raw(width, height, pixels.into_iter().flatten().collect())
        .expect("buffer size matches")
}

fn alpha_blend(
    foreground: &RgbImage,
    background: &RgbImage,
    mask: &[f32],
    width: u32,
    height: u32,
) -> RgbImage {
    let fg_raw = foreground.as_raw();
    let bg_raw = background.as_raw();
    let len = (width * height) as usize;
    let mut out = vec![0u8; len * 3];

    for i in 0..len {
        let alpha = mask.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let inv_alpha = 1.0 - alpha;
        let idx = i * 3;

        out[idx] = (fg_raw[idx] as f32 * alpha + bg_raw[idx] as f32 * inv_alpha) as u8;
        out[idx + 1] =
            (fg_raw[idx + 1] as f32 * alpha + bg_raw[idx + 1] as f32 * inv_alpha) as u8;
        out[idx + 2] =
            (fg_raw[idx + 2] as f32 * alpha + bg_raw[idx + 2] as f32 * inv_alpha) as u8;
    }

    RgbImage::from_raw(width, height, out).expect("buffer size matches")
}
