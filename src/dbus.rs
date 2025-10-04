// SPDX-License-Identifier: GPL-3.0

use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use zbus::object_server::SignalEmitter;

use crate::config::EffectMode;
use crate::pipeline::{self, PipelineCommand, PipelineStatus};

pub const DBUS_NAME: &str = "dev.sindrir.CosmicExtWebcamEffectsApplet";
pub const DBUS_PATH: &str = "/dev/sindrir/CosmicExtWebcamEffectsApplet";

// ── Server-side state ────────────────────────────────────────────────────────

struct DaemonInner {
    cmd_tx: mpsc::Sender<PipelineCommand>,
    running: bool,
    fps: f64,
    gpu_enabled: bool,
}

// ── Server interface ─────────────────────────────────────────────────────────

pub struct WebcamEffectsInterface {
    inner: Arc<Mutex<DaemonInner>>,
}

impl WebcamEffectsInterface {
    /// Create the interface and return it along with a status receiver.
    /// The caller (daemon main) must call `spawn_relay` after the zbus
    /// connection is built so signals are emitted on the correct connection.
    pub fn new() -> (Self, mpsc::Receiver<PipelineStatus>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<PipelineCommand>(16);
        let (status_tx, status_rx) = mpsc::channel::<PipelineStatus>(16);

        pipeline::spawn_pipeline(cmd_rx, status_tx);

        let inner = Arc::new(Mutex::new(DaemonInner {
            cmd_tx,
            running: false,
            fps: 0.0,
            gpu_enabled: false,
        }));

        (Self { inner }, status_rx)
    }

    /// Spawn the background task that relays pipeline status to D-Bus signals.
    /// Must be called after the zbus connection is built so we use the right
    /// connection for signal emission.
    pub fn spawn_relay(
        &self,
        conn: zbus::Connection,
        status_rx: mpsc::Receiver<PipelineStatus>,
    ) {
        tokio::spawn(Self::relay_status(self.inner.clone(), conn, status_rx));
    }

    async fn relay_status(
        inner: Arc<Mutex<DaemonInner>>,
        conn: zbus::Connection,
        mut status_rx: mpsc::Receiver<PipelineStatus>,
    ) {
        let ctxt = match SignalEmitter::new(&conn, DBUS_PATH) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to create signal emitter: {e}");
                return;
            }
        };

        while let Some(status) = status_rx.recv().await {
            match status {
                PipelineStatus::Started => {
                    inner.lock().await.running = true;
                    Self::state_changed(&ctxt, "Running").await.ok();
                }
                PipelineStatus::Stopped => {
                    let mut state = inner.lock().await;
                    state.running = false;
                    state.fps = 0.0;
                    drop(state);
                    Self::state_changed(&ctxt, "Stopped").await.ok();
                }
                PipelineStatus::Error(msg) => {
                    Self::pipeline_error(&ctxt, &msg).await.ok();
                }
                PipelineStatus::Fps(fps) => {
                    inner.lock().await.fps = fps as f64;
                    Self::fps_updated(&ctxt, fps as f64).await.ok();
                }
                PipelineStatus::PreviewFrame(frame_data) => {
                    let (width, height, jpeg_data) = frame_data.as_ref();
                    Self::preview_frame(&ctxt, *width, *height, jpeg_data)
                        .await
                        .ok();
                }
                PipelineStatus::GpuEnabled(enabled) => {
                    inner.lock().await.gpu_enabled = enabled;
                }
            }
        }
    }
}

#[zbus::interface(name = "dev.sindrir.CosmicExtWebcamEffectsApplet1")]
impl WebcamEffectsInterface {
    // ── Signals ──────────────────────────────────────────────────────────────

    #[zbus(signal)]
    pub async fn state_changed(ctxt: &SignalEmitter<'_>, state: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn pipeline_error(ctxt: &SignalEmitter<'_>, message: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn fps_updated(ctxt: &SignalEmitter<'_>, fps: f64) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn preview_frame(
        ctxt: &SignalEmitter<'_>,
        width: u32,
        height: u32,
        jpeg_data: &[u8],
    ) -> zbus::Result<()>;

    // ── Methods ──────────────────────────────────────────────────────────────

    async fn start(
        &self,
        webcam_device: String,
        output_device: String,
    ) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        if inner.running {
            return Err(zbus::fdo::Error::Failed("Pipeline already running".into()));
        }
        inner
            .cmd_tx
            .send(PipelineCommand::Start {
                webcam_device,
                output_device,
            })
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        drop(inner);
        Ok(())
    }

    async fn stop(&self) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::Stop)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn set_effect(&self, mode: &str) -> zbus::fdo::Result<()> {
        let effect = EffectMode::from_str(mode)
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("Unknown effect: {mode}")))?;
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::SetEffect(effect))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn set_blur_intensity(&self, intensity: u32) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::SetBlurIntensity(intensity))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn set_background_image(&self, path: &str) -> zbus::fdo::Result<()> {
        let img_path = if path.is_empty() {
            None
        } else {
            Some(path.to_string())
        };
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::SetBackgroundImage(img_path))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn set_background_color(&self, r: u8, g: u8, b: u8) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::SetBackgroundColor([r, g, b]))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn set_webcam(&self, device: &str) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::SetWebcam(device.to_string()))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn set_output(&self, device: &str) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::SetOutput(device.to_string()))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn set_preview_enabled(&self, enabled: bool) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        inner
            .cmd_tx
            .send(PipelineCommand::SetPreviewEnabled(enabled))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn current_state(&self) -> String {
        if self.inner.lock().await.running {
            "Running".to_string()
        } else {
            "Stopped".to_string()
        }
    }

    async fn current_fps(&self) -> f64 {
        self.inner.lock().await.fps
    }

    async fn enumerate_capture_devices(&self) -> Vec<(String, String)> {
        pipeline::capture::enumerate_devices()
    }

    async fn gpu_enabled(&self) -> bool {
        self.inner.lock().await.gpu_enabled
    }
}

// ── Client proxy (used by the applet) ────────────────────────────────────────

#[zbus::proxy(
    interface = "dev.sindrir.CosmicExtWebcamEffectsApplet1",
    default_service = "dev.sindrir.CosmicExtWebcamEffectsApplet",
    default_path = "/dev/sindrir/CosmicExtWebcamEffectsApplet",
    gen_blocking = false
)]
pub trait WebcamEffectsDaemon {
    async fn start(&self, webcam_device: &str, output_device: &str) -> zbus::Result<()>;
    async fn stop(&self) -> zbus::Result<()>;
    async fn set_effect(&self, mode: &str) -> zbus::Result<()>;
    async fn set_blur_intensity(&self, intensity: u32) -> zbus::Result<()>;
    async fn set_background_image(&self, path: &str) -> zbus::Result<()>;
    async fn set_background_color(&self, r: u8, g: u8, b: u8) -> zbus::Result<()>;
    async fn set_webcam(&self, device: &str) -> zbus::Result<()>;
    async fn set_output(&self, device: &str) -> zbus::Result<()>;
    async fn set_preview_enabled(&self, enabled: bool) -> zbus::Result<()>;
    async fn current_state(&self) -> zbus::Result<String>;
    async fn current_fps(&self) -> zbus::Result<f64>;
    async fn enumerate_capture_devices(&self) -> zbus::Result<Vec<(String, String)>>;
    async fn gpu_enabled(&self) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn state_changed(&self, state: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn pipeline_error(&self, message: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn fps_updated(&self, fps: f64) -> zbus::Result<()>;

    #[zbus(signal)]
    fn preview_frame(&self, width: u32, height: u32, jpeg_data: &[u8]) -> zbus::Result<()>;
}
