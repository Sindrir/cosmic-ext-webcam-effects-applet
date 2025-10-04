// SPDX-License-Identifier: GPL-3.0

use image::RgbImage;
use ndarray::{Array4, ArrayD, IxDyn};
use ort::ep;
use ort::session::Session;
use ort::value::Value;

const MODEL_BYTES: &[u8] = include_bytes!("../../models/rvm_mobilenetv3_fp32.onnx");

/// Input height for inference. RVM accepts dynamic resolution but we resize
/// for consistent performance. Aspect ratio is preserved.
const INPUT_HEIGHT: u32 = 480;

pub struct Segmenter {
    session: Session,
    /// Whether CUDA execution provider is active.
    pub gpu_enabled: bool,
    /// Recurrent states from previous frame for temporal coherence.
    r1: Option<ArrayD<f32>>,
    r2: Option<ArrayD<f32>>,
    r3: Option<ArrayD<f32>>,
    r4: Option<ArrayD<f32>>,
}

impl Segmenter {
    pub fn new() -> Result<Self, String> {
        tracing::info!("Creating ONNX session builder...");
        let builder = Session::builder()
            .map_err(|e| format!("session builder: {e}"))?;

        // Try CUDA first, falls back to CPU automatically if unavailable
        tracing::info!("Registering execution providers (CUDA with CPU fallback)...");
        let mut gpu_enabled = false;
        let builder = match builder
            .with_execution_providers([ep::CUDA::default().build()])
        {
            Ok(b) => {
                gpu_enabled = true;
                tracing::info!("CUDA execution provider registered successfully");
                b
            }
            Err(e) => {
                tracing::warn!("CUDA EP registration failed, using CPU: {e}");
                e.recover()
            }
        };

        tracing::info!("Setting intra threads...");
        let mut builder = builder
            .with_intra_threads(2)
            .map_err(|e| format!("intra threads: {e}"))?;

        tracing::info!("Loading RVM model from memory ({} bytes)...", MODEL_BYTES.len());
        let session = builder
            .commit_from_memory(MODEL_BYTES)
            .map_err(|e| format!("commit model: {e}"))?;

        tracing::info!("Loaded RVM segmentation model successfully");
        Ok(Self {
            session,
            gpu_enabled,
            r1: None,
            r2: None,
            r3: None,
            r4: None,
        })
    }

    /// Reset recurrent states. Call when starting a new capture session.
    pub fn reset_state(&mut self) {
        self.r1 = None;
        self.r2 = None;
        self.r3 = None;
        self.r4 = None;
    }

    /// Run segmentation on an RGB image, returning a mask the same size as the input.
    /// Mask values are 0.0 (background) to 1.0 (person).
    /// Uses RVM's recurrent architecture for temporally coherent results.
    pub fn segment(&mut self, frame: &RgbImage) -> Result<Vec<f32>, String> {
        let orig_w = frame.width() as usize;
        let orig_h = frame.height() as usize;

        // Resize maintaining aspect ratio
        let scale = INPUT_HEIGHT as f32 / orig_h as f32;
        let input_w = ((orig_w as f32 * scale) as u32).max(1);
        let input_h = INPUT_HEIGHT;

        let resized = image::imageops::resize(
            frame,
            input_w,
            input_h,
            image::imageops::FilterType::Triangle,
        );

        // Build NCHW tensor [1, 3, H, W] normalized to [0, 1]
        let mut src_arr = Array4::<f32>::zeros((1, 3, input_h as usize, input_w as usize));
        for y in 0..input_h as usize {
            for x in 0..input_w as usize {
                let pixel = resized.get_pixel(x as u32, y as u32);
                src_arr[[0, 0, y, x]] = pixel[0] as f32 / 255.0;
                src_arr[[0, 1, y, x]] = pixel[1] as f32 / 255.0;
                src_arr[[0, 2, y, x]] = pixel[2] as f32 / 255.0;
            }
        }

        let src = Value::from_array(src_arr)
            .map_err(|e| format!("src tensor: {e}"))?;

        // Recurrent states: previous frame's output, or [1,1,1,1] zeros for first frame
        let r1i = self.r1.take().unwrap_or_else(|| ArrayD::zeros(IxDyn(&[1, 1, 1, 1])));
        let r2i = self.r2.take().unwrap_or_else(|| ArrayD::zeros(IxDyn(&[1, 1, 1, 1])));
        let r3i = self.r3.take().unwrap_or_else(|| ArrayD::zeros(IxDyn(&[1, 1, 1, 1])));
        let r4i = self.r4.take().unwrap_or_else(|| ArrayD::zeros(IxDyn(&[1, 1, 1, 1])));

        let r1v = Value::from_array(r1i).map_err(|e| format!("r1i: {e}"))?;
        let r2v = Value::from_array(r2i).map_err(|e| format!("r2i: {e}"))?;
        let r3v = Value::from_array(r3i).map_err(|e| format!("r3i: {e}"))?;
        let r4v = Value::from_array(r4i).map_err(|e| format!("r4i: {e}"))?;

        // Downsample ratio controls internal resolution for speed/quality tradeoff
        // 0.25 recommended for ~1080p, 0.375 for ~720p
        let ds_ratio = ndarray::arr1(&[0.25f32]);
        let ds_val = Value::from_array(ds_ratio)
            .map_err(|e| format!("downsample_ratio: {e}"))?;

        let outputs = self
            .session
            .run(ort::inputs![
                "src" => src,
                "r1i" => r1v,
                "r2i" => r2v,
                "r3i" => r3v,
                "r4i" => r4v,
                "downsample_ratio" => ds_val
            ])
            .map_err(|e| format!("inference: {e}"))?;

        // RVM outputs: [fgr, pha, r1o, r2o, r3o, r4o]
        // pha (alpha matte) is index 1: [1, 1, H, W]
        let pha = &outputs[1];
        let pha_tensor = pha
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("extract pha: {e}"))?;
        let pha_data: Vec<f32> = pha_tensor.1.to_vec();

        // Save recurrent states for next frame's temporal coherence
        for (idx, field) in [
            (2, &mut self.r1),
            (3, &mut self.r2),
            (4, &mut self.r3),
            (5, &mut self.r4),
        ] {
            let output = &outputs[idx];
            let tensor = output
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("extract r{}o: {e}", idx - 1))?;
            let shape: Vec<usize> = tensor.0.iter().map(|&d| d as usize).collect();
            let data: Vec<f32> = tensor.1.to_vec();
            *field = Some(
                ArrayD::from_shape_vec(IxDyn(&shape), data)
                    .map_err(|e| format!("rebuild r{}o: {e}", idx - 1))?,
            );
        }

        // Resize mask back to original dimensions
        let mask_w = input_w as usize;
        let mask_h = input_h as usize;

        let mut full_mask = vec![0.0f32; orig_w * orig_h];
        for y in 0..orig_h {
            for x in 0..orig_w {
                let src_x = (x * mask_w) / orig_w;
                let src_y = (y * mask_h) / orig_h;
                let idx = src_y * mask_w + src_x;
                full_mask[y * orig_w + x] = pha_data.get(idx).copied().unwrap_or(0.0);
            }
        }

        Ok(full_mask)
    }
}
