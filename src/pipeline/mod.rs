// SPDX-License-Identifier: GPL-3.0

pub mod capture;
pub mod compositor;
pub mod output;
pub mod segmentation;

use crate::config::EffectMode;
use image::RgbImage;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// Commands sent from the UI to the pipeline thread.
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    Start {
        webcam_device: String,
        output_device: String,
    },
    Stop,
    SetWebcam(String),
    SetOutput(String),
    SetEffect(EffectMode),
    SetBlurIntensity(u32),
    SetBackgroundImage(Option<String>),
    SetBackgroundColor([u8; 3]),
    SetPreviewEnabled(bool),
}

/// Status updates sent from the pipeline thread to the UI.
#[derive(Debug, Clone)]
pub enum PipelineStatus {
    Started,
    Stopped,
    Error(String),
    Fps(f32),
    /// Preview frame as (width, height, JPEG bytes)
    PreviewFrame(Arc<(u32, u32, Vec<u8>)>),
    /// Whether GPU acceleration is available
    GpuEnabled(bool),
}

const PREVIEW_MAX_WIDTH: u32 = 320;

/// Scale `img` to cover `target_w x target_h` while preserving aspect ratio,
/// then center-crop to the exact target size.
fn resize_cover(img: &RgbImage, target_w: u32, target_h: u32) -> RgbImage {
    let scale = (target_w as f32 / img.width() as f32)
        .max(target_h as f32 / img.height() as f32);
    let scaled_w = ((img.width() as f32 * scale).round() as u32).max(target_w);
    let scaled_h = ((img.height() as f32 * scale).round() as u32).max(target_h);
    let scaled = image::imageops::resize(img, scaled_w, scaled_h, image::imageops::FilterType::Triangle);
    let x = (scaled_w - target_w) / 2;
    let y = (scaled_h - target_h) / 2;
    image::imageops::crop_imm(&scaled, x, y, target_w, target_h).to_image()
}

/// Run the pipeline in a background thread, communicating via channels.
pub fn spawn_pipeline(
    mut cmd_rx: mpsc::Receiver<PipelineCommand>,
    status_tx: mpsc::Sender<PipelineStatus>,
) {
    std::thread::Builder::new()
        .name("pipeline".into())
        .spawn(move || {
            // Initialize segmenter eagerly, outside the tokio runtime.
            // ort's Session::builder() initializes the ORT environment which can
            // deadlock inside a single-threaded tokio block_on.
            let segmenter = match segmentation::Segmenter::new() {
                Ok(s) => {
                    tracing::info!("Segmenter initialized successfully (GPU: {})", s.gpu_enabled);
                    let _ = status_tx.blocking_send(PipelineStatus::GpuEnabled(s.gpu_enabled));
                    Some(s)
                }
                Err(e) => {
                    tracing::error!("Failed to initialize segmenter: {e}");
                    let _ = status_tx.blocking_send(PipelineStatus::Error(
                        format!("Model load failed: {e}"),
                    ));
                    let _ = status_tx.blocking_send(PipelineStatus::GpuEnabled(false));
                    None
                }
            };

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            rt.block_on(async move {
                let mut state = PipelineState::default();
                state.segmenter = segmenter;

                loop {
                    if state.running {
                        // Process commands without blocking
                        while let Ok(cmd) = cmd_rx.try_recv() {
                            if handle_command(&mut state, cmd, &status_tx).await {
                                return;
                            }
                        }

                        // A Stop command may have been processed above
                        if !state.running {
                            continue;
                        }

                        // Run one pipeline frame
                        match state.process_frame() {
                            Ok(result) => {
                                // Send preview frame every frame when popup is open
                                // try_send drops frames if the UI can't keep up
                                if state.preview_enabled {
                                    if let Some(preview) = make_preview(&result) {
                                        let _ = status_tx
                                            .try_send(PipelineStatus::PreviewFrame(Arc::new(
                                                preview,
                                            )));
                                    }
                                }

                                // Write to output
                                if let Err(e) = state.write_output(&result) {
                                    tracing::error!("Output write error: {e}");
                                    let _ =
                                        status_tx.try_send(PipelineStatus::Error(e.to_string()));
                                    state.running = false;
                                    state.capture = None;
                                    state.output = None;
                                    let _ = status_tx.try_send(PipelineStatus::Stopped);
                                }
                            }
                            Err(e) => {
                                tracing::error!("Pipeline frame error: {e}");
                                let _ = status_tx.try_send(PipelineStatus::Error(e.to_string()));
                                state.running = false;
                                state.capture = None;
                                state.output = None;
                                let _ = status_tx.try_send(PipelineStatus::Stopped);
                            }
                        }

                        // Report FPS periodically
                        state.frame_count += 1;
                        let elapsed = state.fps_timer.elapsed().as_secs_f32();
                        if elapsed >= 1.0 {
                            let fps = state.frame_count as f32 / elapsed;
                            tracing::debug!("FPS: {fps:.1}");
                            let _ = status_tx.try_send(PipelineStatus::Fps(fps));
                            state.frame_count = 0;
                            state.fps_timer = Instant::now();
                        }
                    } else {
                        // Wait for commands when not running
                        match cmd_rx.recv().await {
                            Some(cmd) => {
                                if handle_command(&mut state, cmd, &status_tx).await {
                                    return;
                                }
                            }
                            None => return,
                        }
                    }
                }
            });
        })
        .expect("spawn pipeline thread");
}

/// Convert an RGB image to a downscaled JPEG for D-Bus preview transport.
fn make_preview(frame: &RgbImage) -> Option<(u32, u32, Vec<u8>)> {
    use image::codecs::jpeg::JpegEncoder;
    use std::io::Cursor;

    let w = frame.width();
    let h = frame.height();
    if w == 0 || h == 0 {
        return None;
    }

    // Downscale if needed
    let (pw, ph) = if w > PREVIEW_MAX_WIDTH {
        let scale = PREVIEW_MAX_WIDTH as f32 / w as f32;
        (PREVIEW_MAX_WIDTH, (h as f32 * scale) as u32)
    } else {
        (w, h)
    };

    let small = if pw != w || ph != h {
        image::imageops::resize(frame, pw, ph, image::imageops::FilterType::Nearest)
    } else {
        image::ImageBuffer::from_raw(w, h, frame.as_raw().clone()).unwrap()
    };

    // Encode as JPEG for efficient D-Bus transport
    let mut buf = Cursor::new(Vec::with_capacity(16 * 1024));
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, 50);
    encoder
        .encode_image(&small)
        .map_err(|e| tracing::error!("JPEG encode error: {e}"))
        .ok()?;

    Some((pw, ph, buf.into_inner()))
}

async fn handle_command(
    state: &mut PipelineState,
    cmd: PipelineCommand,
    status_tx: &mpsc::Sender<PipelineStatus>,
) -> bool {
    match cmd {
        PipelineCommand::Start {
            webcam_device,
            output_device,
        } => {
            tracing::info!("Starting pipeline: {webcam_device} -> {output_device}");

            // Don't start if already running
            if state.running {
                tracing::warn!("Pipeline already running, ignoring Start command");
                return false;
            }

            // Check segmenter is available and reset its temporal state
            if let Some(seg) = state.segmenter.as_mut() {
                seg.reset_state();
            }
            if state.segmenter.is_none() {
                let _ = status_tx
                    .send(PipelineStatus::Error("Segmentation model not loaded".into()))
                    .await;
                return false;
            }

            // Open capture device
            tracing::info!("Opening capture device: {webcam_device}");
            match capture::CaptureDevice::open(&webcam_device) {
                Ok(cap) => {
                    let w = cap.width();
                    let h = cap.height();

                    // Open output device
                    match output::OutputDevice::open(&output_device, w, h) {
                        Ok(out) => {
                            state.capture = Some(cap);
                            state.output = Some(out);
                            state.output_path = Some(output_device);
                            state.update_resized_background();
                            state.running = true;
                            state.fps_timer = Instant::now();
                            state.frame_count = 0;
                            let _ = status_tx.send(PipelineStatus::Started).await;
                        }
                        Err(e) => {
                            let _ = status_tx
                                .send(PipelineStatus::Error(format!(
                                    "Output device error: {e}"
                                )))
                                .await;
                        }
                    }
                }
                Err(e) => {
                    let _ = status_tx
                        .send(PipelineStatus::Error(format!("Capture error: {e}")))
                        .await;
                }
            }
        }
        PipelineCommand::Stop => {
            state.running = false;
            state.capture = None;
            state.output = None;
            state.output_path = None;
            let _ = status_tx.send(PipelineStatus::Stopped).await;
        }
        PipelineCommand::SetWebcam(path) => {
            if state.running {
                tracing::info!("Hot-swapping capture device to {path}");
                // Drop old capture first to release the device
                state.capture = None;
                match capture::CaptureDevice::open(&path) {
                    Ok(cap) => {
                        let w = cap.width();
                        let h = cap.height();
                        state.capture = Some(cap);
                        // Re-open output at new capture resolution
                        // Drop the old output first so the v4l2loopback device is free
                        if let Some(out_path) = state.output_path.clone() {
                            state.output = None;
                            match output::OutputDevice::open(&out_path, w, h) {
                                Ok(new_out) => { state.output = Some(new_out); }
                                Err(e) => {
                                    tracing::error!("Failed to reopen output: {e}");
                                    let _ = status_tx.send(PipelineStatus::Error(format!("Output reopen: {e}"))).await;
                                }
                            }
                        }
                        state.update_resized_background();
                        if let Some(seg) = state.segmenter.as_mut() {
                            seg.reset_state();
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to open capture device {path}: {e}");
                        let _ = status_tx.send(PipelineStatus::Error(format!("Capture error: {e}"))).await;
                    }
                }
            }
        }
        PipelineCommand::SetOutput(path) => {
            if state.running {
                tracing::info!("Hot-swapping output device to {path}");
                let (w, h) = state.capture.as_ref()
                    .map(|c| (c.width(), c.height()))
                    .unwrap_or((640, 480));
                state.output = None; // drop old output before opening new one
                match output::OutputDevice::open(&path, w, h) {
                    Ok(out) => {
                        state.output = Some(out);
                        state.output_path = Some(path);
                    }
                    Err(e) => {
                        tracing::error!("Failed to open output device {path}: {e}");
                        let _ = status_tx.send(PipelineStatus::Error(format!("Output error: {e}"))).await;
                    }
                }
            }
        }
        PipelineCommand::SetEffect(mode) => {
            state.effect_mode = mode;
        }
        PipelineCommand::SetBlurIntensity(intensity) => {
            state.blur_intensity = intensity;
        }
        PipelineCommand::SetBackgroundImage(path) => {
            state.background_image = path.and_then(|p| {
                image::open(&p)
                    .map_err(|e| tracing::error!("Failed to load background image: {e}"))
                    .ok()
                    .map(|img| img.to_rgb8())
            });
            state.update_resized_background();
        }
        PipelineCommand::SetBackgroundColor(color) => {
            state.background_color = color;
        }
        PipelineCommand::SetPreviewEnabled(enabled) => {
            state.preview_enabled = enabled;
        }
    }
    false
}

struct PipelineState {
    running: bool,
    capture: Option<capture::CaptureDevice>,
    output: Option<output::OutputDevice>,
    output_path: Option<String>,
    segmenter: Option<segmentation::Segmenter>,
    effect_mode: EffectMode,
    blur_intensity: u32,
    background_image: Option<RgbImage>,
    /// Background image pre-resized to match capture dimensions. Avoids
    /// expensive per-frame resizing.
    background_image_resized: Option<RgbImage>,
    background_color: [u8; 3],
    fps_timer: Instant,
    frame_count: u32,
    preview_enabled: bool,
}

impl Default for PipelineState {
    fn default() -> Self {
        Self {
            running: false,
            capture: None,
            output: None,
            output_path: None,
            segmenter: None,
            effect_mode: EffectMode::default(),
            blur_intensity: 0,
            background_image: None,
            background_image_resized: None,
            background_color: [0; 3],
            fps_timer: Instant::now(),
            frame_count: 0,
            preview_enabled: false,
        }
    }
}

impl PipelineState {
    fn process_frame(&mut self) -> Result<RgbImage, String> {
        let capture = self
            .capture
            .as_mut()
            .ok_or("Capture device not available")?;
        let frame = capture.grab_frame().map_err(|e| e.to_string())?;

        if self.effect_mode == EffectMode::None {
            Ok(frame)
        } else {
            let segmenter = self
                .segmenter
                .as_mut()
                .ok_or("Segmenter not available")?;
            let mask = segmenter.segment(&frame)?;
            Ok(compositor::composite(
                &frame,
                &mask,
                self.effect_mode,
                self.blur_intensity,
                self.background_image_resized.as_ref(),
                self.background_color,
            ))
        }
    }

    /// Rebuild the cached resized background image to match capture dimensions.
    fn update_resized_background(&mut self) {
        let Some(cap) = self.capture.as_ref() else {
            self.background_image_resized = None;
            return;
        };
        let w = cap.width();
        let h = cap.height();
        self.background_image_resized = self.background_image.as_ref().map(|bg| {
            resize_cover(bg, w, h)
        });
    }

    fn write_output(&mut self, frame: &RgbImage) -> Result<(), String> {
        let output = self
            .output
            .as_mut()
            .ok_or("Output device not available")?;
        output.write_frame(frame).map_err(|e| e.to_string())
    }
}
