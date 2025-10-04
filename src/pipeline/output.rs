// SPDX-License-Identifier: GPL-3.0

use image::RgbImage;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use v4l::video::Output;
use v4l::{Device, FourCC};

pub struct OutputDevice {
    fd: std::fs::File,
    width: u32,
    height: u32,
}

impl OutputDevice {
    pub fn open(
        path: &str,
        width: u32,
        height: u32,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dev = Device::with_path(path)?;

        // Configure the output format to match our frame size
        let mut fmt = dev.format()?;
        fmt.width = width;
        fmt.height = height;
        // v4l2loopback typically accepts RGB24 or YUYV
        fmt.fourcc = FourCC::new(b"RGB3");
        match dev.set_format(&fmt) {
            Ok(f) => {
                tracing::info!("Output device format: {}x{} {:?}", f.width, f.height, f.fourcc);
            }
            Err(e) => {
                tracing::warn!("Could not set RGB3 format on output device: {e}, trying raw write");
            }
        }

        // Open the device file for raw writing
        let fd = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;

        Ok(Self { fd, width, height })
    }

    pub fn write_frame(
        &mut self,
        frame: &RgbImage,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let frame = if frame.width() != self.width || frame.height() != self.height {
            image::imageops::resize(
                frame,
                self.width,
                self.height,
                image::imageops::FilterType::Triangle,
            )
            .into()
        } else {
            frame.clone()
        };

        let data = frame.into_raw();
        self.fd.write_all(&data)?;
        Ok(())
    }
}
