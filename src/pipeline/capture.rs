// SPDX-License-Identifier: GPL-3.0

use image::RgbImage;
use std::collections::HashSet;
use std::path::Path;
use v4l::buffer::Type;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;
use v4l::{Device, FourCC};

/// Enumerates available V4L2 video capture devices.
///
/// UVC cameras often expose multiple V4L2 nodes per physical device (e.g. RGB + IR).
/// We deduplicate by card name, keeping only the first (lowest-numbered) node per camera.
pub fn enumerate_devices() -> Vec<(String, String)> {
    let mut devices = Vec::new();
    let mut seen_cards = HashSet::new();
    for i in 0..16 {
        let path = format!("/dev/video{i}");
        if !Path::new(&path).exists() {
            continue;
        }
        let Ok(dev) = Device::with_path(&path) else {
            continue;
        };
        let Ok(caps) = dev.query_caps() else {
            continue;
        };
        if !caps.capabilities.contains(v4l::capability::Flags::VIDEO_CAPTURE) {
            continue;
        }
        // Skip v4l2loopback (virtual output) devices
        if caps.driver.contains("v4l2 loopback") {
            continue;
        }
        // Prefer the full USB product name over the truncated V4L2 card field
        let label = sysfs_product_name(&path).unwrap_or(caps.card.clone());
        if !seen_cards.insert(label.clone()) {
            tracing::debug!("Skipping duplicate capture node {path} for card {label:?}");
            continue;
        }
        devices.push((path, label));
    }
    devices
}

/// Read the USB product name from sysfs, which isn't truncated like the V4L2 card field.
fn sysfs_product_name(dev_path: &str) -> Option<String> {
    let dev_name = Path::new(dev_path).file_name()?.to_str()?;
    let product_path = Path::new("/sys/class/video4linux")
        .join(dev_name)
        .join("device/../product");
    std::fs::read_to_string(product_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Finds v4l2loopback devices by checking the V4L2 driver name.
///
/// v4l2loopback devices present as VIDEO_CAPTURE to consumers but are used
/// as virtual camera outputs by this application.
pub fn enumerate_output_devices() -> Vec<(String, String)> {
    let mut devices = Vec::new();
    let mut seen_cards = HashSet::new();
    for i in 0..16 {
        let path = format!("/dev/video{i}");
        if !Path::new(&path).exists() {
            continue;
        }
        let Ok(dev) = Device::with_path(&path) else {
            continue;
        };
        let Ok(caps) = dev.query_caps() else {
            continue;
        };
        if !caps.driver.contains("v4l2 loopback") {
            continue;
        }
        let label = caps.card.clone();
        if !seen_cards.insert(label.clone()) {
            continue;
        }
        devices.push((path, label));
    }
    devices
}

pub struct CaptureDevice {
    stream: Stream<'static>,
    width: u32,
    height: u32,
    format: FourCC,
}

impl CaptureDevice {
    pub fn open(path: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dev = Device::with_path(path)?;

        // Try to set a reasonable format
        let mut fmt = dev.format()?;
        // Prefer MJPEG, fall back to YUYV
        fmt.fourcc = FourCC::new(b"MJPG");
        fmt.width = 640;
        fmt.height = 480;
        let fmt = match dev.set_format(&fmt) {
            Ok(f) => f,
            Err(_) => {
                let mut fmt = dev.format()?;
                fmt.fourcc = FourCC::new(b"YUYV");
                fmt.width = 640;
                fmt.height = 480;
                dev.set_format(&fmt)?
            }
        };

        let width = fmt.width;
        let height = fmt.height;
        let format = fmt.fourcc;

        tracing::info!(
            "Opened capture device {path}: {width}x{height} {:?}",
            format
        );

        let stream = Stream::with_buffers(&dev, Type::VideoCapture, 4)?;

        Ok(Self {
            stream,
            width,
            height,
            format,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn grab_frame(&mut self) -> Result<RgbImage, Box<dyn std::error::Error + Send + Sync>> {
        let (buf, _meta) = self.stream.next()?;
        decode_frame(buf, self.width, self.height, &self.format)
    }
}

fn decode_frame(
    data: &[u8],
    width: u32,
    height: u32,
    fourcc: &FourCC,
) -> Result<RgbImage, Box<dyn std::error::Error + Send + Sync>> {
    let repr = fourcc.repr;
    match &repr {
        b"MJPG" => {
            let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)?;
            Ok(img.to_rgb8())
        }
        b"YUYV" => Ok(yuyv_to_rgb(data, width, height)),
        b"RGB3" | b"RGB\0" => {
            let expected = (width * height * 3) as usize;
            if data.len() >= expected {
                Ok(RgbImage::from_raw(width, height, data[..expected].to_vec())
                    .expect("buffer size matches"))
            } else {
                Err("RGB buffer too small".into())
            }
        }
        _ => Err(format!("Unsupported pixel format: {:?}", fourcc).into()),
    }
}

fn yuyv_to_rgb(data: &[u8], width: u32, height: u32) -> RgbImage {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for chunk in data.chunks_exact(4) {
        let (y0, u, y1, v) = (chunk[0] as f32, chunk[1] as f32, chunk[2] as f32, chunk[3] as f32);
        for y in [y0, y1] {
            let r = (y + 1.402 * (v - 128.0)).clamp(0.0, 255.0) as u8;
            let g = (y - 0.344136 * (u - 128.0) - 0.714136 * (v - 128.0)).clamp(0.0, 255.0) as u8;
            let b = (y + 1.772 * (u - 128.0)).clamp(0.0, 255.0) as u8;
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    RgbImage::from_raw(width, height, rgb).expect("buffer size matches")
}
