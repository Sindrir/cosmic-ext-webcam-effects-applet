// SPDX-License-Identifier: GPL-3.0

use crate::config::{Config, EffectMode};
use crate::dbus::WebcamEffectsDaemonProxy;
use crate::fl;
use cosmic::applet::{menu_button, padded_control};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::dialog::file_chooser;
use cosmic::iced::widget::Image;
use cosmic::iced::{window::Id, Color, Length, Limits, Subscription};
use cosmic::iced_runtime::core::image::Handle as ImageHandle;
use cosmic::iced_winit::commands::popup::{destroy_popup, get_popup};
use cosmic::prelude::*;
use cosmic::widget::{self, color_picker::ColorPickerUpdate, ColorPickerModel};
use palette::IntoColor;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

const PREVIEW_WIDTH: f32 = 320.0;
const PREVIEW_HEIGHT: f32 = 240.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IsOpen {
    None,
    Webcam,
    Effect,
    ColorPicker,
}

pub struct AppModel {
    core: cosmic::Core,
    popup: Option<Id>,
    config: Config,
    config_helper: Option<cosmic_config::Config>,

    // Pipeline state (mirrored from daemon)
    pipeline_running: bool,
    pipeline_fps: f32,
    pipeline_error: Option<String>,

    // Device lists
    capture_devices: Vec<(String, String)>,
    capture_labels: Vec<String>,
    output_device: Option<(String, String)>, // (path, label) — auto-detected v4l2loopback
    effect_labels: Vec<String>,

    // GPU acceleration status
    gpu_enabled: Option<bool>,

    // Revealer state (accordion)
    is_open: IsOpen,

    // Color picker for solid color mode
    color_picker: ColorPickerModel,

    // Preview frame from daemon (decoded from JPEG)
    preview_handle: Option<ImageHandle>,

    // Prevent popup close while file chooser is open
    file_chooser_open: bool,
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            core: cosmic::Core::default(),
            popup: None,
            config: Config::default(),
            config_helper: None,
            pipeline_running: false,
            pipeline_fps: 0.0,
            pipeline_error: None,
            capture_devices: Vec::new(),
            capture_labels: Vec::new(),
            output_device: None,
            gpu_enabled: None,
            is_open: IsOpen::None,
            effect_labels: Vec::new(),
            color_picker: ColorPickerModel::new("HEX", "RGB", None, Some(Color::BLACK)),
            preview_handle: None,
            file_chooser_open: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    UpdateConfig(Config),

    // Daemon state (from D-Bus signals)
    DaemonStateChanged(String),
    DaemonFpsUpdated(f64),
    DaemonError(String),
    DaemonPreviewFrame(Arc<(u32, u32, Vec<u8>)>),
    DaemonDevices(Vec<(String, String)>),
    DaemonGpuEnabled(bool),

    // Pipeline control (sent to daemon via D-Bus)
    TogglePipeline,
    DaemonCallResult(Result<(), String>),

    // Settings
    ToggleWebcamRevealer,
    SelectWebcam(usize),
    ToggleEffectRevealer,
    SetEffectMode(usize),
    SetBlurIntensity(f32),
    BlurIntensityReleased,
    ToggleAutoStart(bool),

    // Background image picker
    ChooseBackgroundImage,
    BackgroundImageResult(Arc<Result<url::Url, file_chooser::Error>>),

    // Color picker
    ToggleColorPicker,
    ColorPickerUpdate(ColorPickerUpdate),

    // Error
    CopyError,
    DismissError,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = crate::APP_ID;

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        let config_helper = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();
        let config = config_helper
            .as_ref()
            .map(|context| match Config::get_entry(context) {
                Ok(config) => config,
                Err((errors, config)) => {
                    for err in errors {
                        tracing::error!(?err, "error loading app config");
                    }
                    config
                }
            })
            .unwrap_or_default();

        let effect_labels = vec![
            fl!("effect-none"),
            fl!("effect-blur"),
            fl!("effect-replace"),
            fl!("effect-solid"),
        ];

        // Initialize color picker from saved config
        let [r, g, b] = config.background_color;
        let initial_color = Color::from_rgb8(r, g, b);
        let color_picker = ColorPickerModel::new("HEX", "RGB", None, Some(initial_color));

        let app = AppModel {
            core,
            config,
            config_helper,
            effect_labels,
            color_picker,
            ..Default::default()
        };

        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let icon_name = if self.pipeline_running {
            "camera-web-symbolic"
        } else {
            "camera-disabled-symbolic"
        };

        self.core
            .applet
            .icon_button(icon_name)
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let spacing = cosmic::theme::spacing();
        let div = || {
            padded_control(widget::divider::horizontal::default())
                .padding([spacing.space_xxs, spacing.space_s])
        };

        let mut col = widget::column();

        // GPU/CPU badge in the top-right corner
        if let Some(gpu) = self.gpu_enabled {
            let (badge_text, badge_color) = if gpu {
                (fl!("gpu-accelerated"), Color::from_rgb8(76, 175, 80))
            } else {
                (fl!("cpu-only"), Color::from_rgb8(255, 152, 0))
            };
            col = col.push(padded_control(
                widget::container(
                    widget::container(
                        widget::text::caption(badge_text),
                    )
                    .padding([2, 8])
                    .class(cosmic::theme::Container::custom(move |_theme| {
                        cosmic::widget::container::Style {
                            background: Some(badge_color.into()),
                            border: cosmic::iced::Border {
                                radius: 4.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })),
                )
                .align_right(Length::Fill),
            ));
        }

        // Live preview thumbnail
        if let Some(ref handle) = self.preview_handle {
            col = col.push(padded_control(
                widget::container(
                    Image::new(handle.clone())
                        .width(Length::Fixed(PREVIEW_WIDTH))
                        .height(Length::Fixed(PREVIEW_HEIGHT))
                        .content_fit(cosmic::iced_core::ContentFit::Contain),
                )
                .center_x(Length::Fill),
            ));
        } else {
            col = col.push(padded_control(
                widget::container(
                    widget::text::body(fl!("preview-placeholder"))
                        .width(Length::Fill)
                        .center(),
                )
                .width(Length::Fill)
                .height(Length::Fixed(PREVIEW_HEIGHT))
                .center(Length::Fill),
            ));
        }

        col = col.push(div());

        // Webcam device revealer
        {
            let selected_label = self
                .capture_devices
                .iter()
                .find(|(path, _)| path == &self.config.webcam_device)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| {
                    if self.capture_devices.is_empty() {
                        fl!("no-devices")
                    } else {
                        "—".to_string()
                    }
                });

            col = col.push(revealer_head(
                fl!("webcam-device"),
                selected_label,
                Message::ToggleWebcamRevealer,
            ));

            if self.is_open == IsOpen::Webcam {
                for (idx, (_, label)) in self.capture_devices.iter().enumerate() {
                    col = col.push(
                        menu_button(widget::text::body(label.as_str()))
                            .on_press(Message::SelectWebcam(idx))
                            .padding([8, 48]),
                    );
                }
            }
        }

        // Output device (show virtual camera name)
        {
            let output_label = self
                .output_device
                .as_ref()
                .map(|(_, label)| label.as_str())
                .unwrap_or("—");
            col = col.push(padded_control(
                widget::row()
                    .push(widget::text::body(fl!("output-device")))
                    .push(widget::Space::new().width(Length::Fill))
                    .push(widget::text::body(output_label))
                    .align_y(cosmic::iced::Alignment::Center),
            ));
        }

        col = col.push(div());

        // Effect mode revealer
        {
            let selected_effect_label = match self.config.effect_mode {
                EffectMode::None => fl!("effect-none"),
                EffectMode::Blur => fl!("effect-blur"),
                EffectMode::Replace => fl!("effect-replace"),
                EffectMode::SolidColor => fl!("effect-solid"),
            };

            col = col.push(revealer_head(
                fl!("effect-mode"),
                selected_effect_label,
                Message::ToggleEffectRevealer,
            ));

            if self.is_open == IsOpen::Effect {
                for (idx, label) in self.effect_labels.iter().enumerate() {
                    col = col.push(
                        menu_button(widget::text::body(label.as_str()))
                            .on_press(Message::SetEffectMode(idx))
                            .padding([8, 48]),
                    );
                }
            }
        }

        // Effect-specific controls
        match self.config.effect_mode {
            EffectMode::Blur => {
                col = col.push(padded_control(
                    widget::column::with_children(vec![
                        widget::text::body(fl!("blur-intensity")).into(),
                        widget::slider(
                            0.0..=100.0_f32,
                            self.config.blur_intensity as f32,
                            Message::SetBlurIntensity,
                        )
                        .on_release(Message::BlurIntensityReleased)
                        .into(),
                    ])
                    .spacing(spacing.space_xxs),
                ));
            }
            EffectMode::Replace => {
                if self.config.background_image_path.is_empty() {
                    col = col.push(padded_control(
                        widget::row()
                            .push(widget::text::body(fl!("background-image")))
                            .push(widget::Space::new().width(Length::Fill))
                            .push(
                                widget::button::suggested(fl!("choose-image"))
                                    .on_press(Message::ChooseBackgroundImage),
                            )
                            .align_y(cosmic::iced::Alignment::Center),
                    ));
                } else {
                    let filename = std::path::Path::new(&self.config.background_image_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| self.config.background_image_path.clone());

                    col = col.push(padded_control(
                        widget::column()
                            .push(widget::text::body(fl!("background-image")))
                            .push(
                                widget::row()
                                    .push(widget::text::caption(filename).width(Length::Fill))
                                    .push(
                                        widget::button::standard(fl!("change-image"))
                                            .on_press(Message::ChooseBackgroundImage),
                                    )
                                    .align_y(cosmic::iced::Alignment::Center)
                                    .spacing(spacing.space_xs),
                            )
                            .spacing(spacing.space_xxs),
                    ));
                }
            }
            EffectMode::SolidColor => {
                // Color swatch revealer
                let [r, g, b] = self.config.background_color;
                let swatch_color = Color::from_rgb8(r, g, b);

                col = col.push(
                    menu_button(
                        widget::row()
                            .push(widget::text::body(fl!("background-color")))
                            .push(widget::Space::new().width(Length::Fill))
                            .push(
                                widget::container(widget::Space::new())
                                    .width(Length::Fixed(24.0))
                                    .height(Length::Fixed(24.0))
                                    .class(cosmic::theme::Container::custom(move |_theme| {
                                        cosmic::widget::container::Style {
                                            background: Some(swatch_color.into()),
                                            border: cosmic::iced::Border {
                                                radius: 4.0.into(),
                                                width: 1.0,
                                                color: Color::from_rgba8(128, 128, 128, 0.4),
                                            },
                                            ..Default::default()
                                        }
                                    })),
                            )
                            .align_y(cosmic::iced::Alignment::Center)
                            .spacing(spacing.space_xs),
                    )
                    .on_press(Message::ToggleColorPicker),
                );

                if self.is_open == IsOpen::ColorPicker {
                    col = col.push(padded_control(
                        self.color_picker
                            .builder(Message::ColorPickerUpdate)
                            .build(fl!("background-color"), String::new(), String::new()),
                    ));
                }
            }
            EffectMode::None => {}
        }

        col = col.push(div());

        // Auto-start toggle
        col = col.push(padded_control(
            widget::row()
                .push(widget::text::body(fl!("auto-start")))
                .push(widget::Space::new().width(Length::Fill))
                .push(widget::toggler(self.config.auto_start).on_toggle(Message::ToggleAutoStart))
                .align_y(cosmic::iced::Alignment::Center),
        ));

        col = col.push(div());

        // Status & start/stop
        let status_text = if self.pipeline_running {
            format!(
                "{} ({:.0} {})",
                fl!("running"),
                self.pipeline_fps,
                fl!("fps")
            )
        } else {
            fl!("stopped")
        };

        let webcam_valid = self
            .capture_devices
            .iter()
            .any(|(path, _)| path == &self.config.webcam_device);
        let output_valid = self.output_device.is_some();
        let can_start = webcam_valid && output_valid;

        let toggle_button: Element<'_, Self::Message> = if self.pipeline_running {
            widget::button::destructive(fl!("stop"))
                .on_press(Message::TogglePipeline)
                .into()
        } else {
            widget::button::suggested(fl!("start"))
                .on_press_maybe(if can_start {
                    Some(Message::TogglePipeline)
                } else {
                    None
                })
                .into()
        };

        col = col.push(padded_control(
            widget::row()
                .push(widget::text::body(status_text))
                .push(widget::Space::new().width(Length::Fill))
                .push(toggle_button)
                .align_y(cosmic::iced::Alignment::Center),
        ));

        // Error banner (below status, dismissable with copy button)
        if let Some(ref err) = self.pipeline_error {
            col = col.push(div());
            col = col.push(padded_control(
                widget::container(
                    widget::row()
                        .push(
                            widget::column()
                                .push(widget::text::body(fl!("pipeline-error")))
                                .push(widget::text::caption(err.clone()))
                                .width(Length::Fill),
                        )
                        .push(
                            widget::button::icon(widget::icon::from_name("edit-copy-symbolic"))
                                .on_press(Message::CopyError),
                        )
                        .push(
                            widget::button::icon(widget::icon::from_name("window-close-symbolic"))
                                .on_press(Message::DismissError),
                        )
                        .align_y(cosmic::iced::Alignment::Center)
                        .spacing(spacing.space_xxs),
                )
                .padding(spacing.space_xs)
                .class(cosmic::theme::Container::custom(|theme| {
                    let destructive = theme.cosmic().destructive_color();
                    cosmic::widget::container::Style {
                        background: Some(
                            Color::from_rgba8(
                                (destructive.red * 255.0) as u8,
                                (destructive.green * 255.0) as u8,
                                (destructive.blue * 255.0) as u8,
                                0.15,
                            )
                            .into(),
                        ),
                        border: cosmic::iced::Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })),
            ));
        }

        self.core
            .applet
            .popup_container(col.padding([8, 0]))
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch(vec![
            daemon_subscription(),
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| {
                    for err in update.errors {
                        tracing::error!(?err, "error watching config");
                    }
                    Message::UpdateConfig(update.config)
                }),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::DaemonStateChanged(state) => {
                let was_running = self.pipeline_running;
                self.pipeline_running = state == "Running";
                if !self.pipeline_running {
                    self.pipeline_fps = 0.0;
                    self.preview_handle = None;
                }
                if self.pipeline_running && !was_running {
                    self.pipeline_error = None;
                }
            }
            Message::DaemonFpsUpdated(fps) => {
                self.pipeline_fps = fps as f32;
            }
            Message::DaemonError(msg) => {
                tracing::error!("Daemon error: {msg}");
                self.pipeline_error = Some(msg);
            }
            Message::DaemonPreviewFrame(frame_data) => {
                let (_width, _height, ref jpeg_data) = *frame_data;
                // Decode JPEG to RGBA for display
                if let Ok(img) = image::load_from_memory_with_format(jpeg_data, image::ImageFormat::Jpeg) {
                    let mirrored = img.fliph();
                    let rgba = mirrored.to_rgba8();
                    self.preview_handle = Some(ImageHandle::from_rgba(
                        rgba.width(),
                        rgba.height(),
                        rgba.into_raw(),
                    ));
                }
            }
            Message::DaemonDevices(devices) => {
                self.capture_labels = devices
                    .iter()
                    .map(|(_path, label)| label.clone())
                    .collect();
                self.capture_devices = devices;

                // Auto-select first device if current selection is invalid
                if !self
                    .capture_devices
                    .iter()
                    .any(|(path, _)| path == &self.config.webcam_device)
                {
                    self.config.webcam_device = self
                        .capture_devices
                        .first()
                        .map(|(path, _)| path.clone())
                        .unwrap_or_default();
                }

                // Auto-detect v4l2loopback output device
                self.output_device =
                    crate::pipeline::capture::enumerate_output_devices()
                        .into_iter()
                        .next();
            }
            Message::DaemonGpuEnabled(enabled) => {
                self.gpu_enabled = Some(enabled);
            }
            Message::DaemonCallResult(Err(e)) => {
                tracing::error!("D-Bus call failed: {e}");
                self.pipeline_error = Some(e);
            }
            Message::DaemonCallResult(Ok(())) => {}
            Message::CopyError => {
                if let Some(ref err) = self.pipeline_error {
                    return cosmic::iced::clipboard::write(err.clone());
                }
            }
            Message::DismissError => {
                self.pipeline_error = None;
            }
            Message::TogglePipeline => {
                if self.pipeline_running {
                    return daemon_call(|proxy| async move {
                        proxy.stop().await.map_err(|e| e.to_string())
                    });
                } else {
                    let webcam = self.config.webcam_device.clone();
                    let output = match self.output_device {
                        Some((ref path, _)) => path.clone(),
                        None => return Task::none(),
                    };
                    let effect = self.config.effect_mode;
                    let blur = self.config.blur_intensity;
                    let bg_color = self.config.background_color;
                    let bg_image = self.config.background_image_path.clone();

                    return daemon_call(move |proxy| async move {
                        proxy
                            .start(&webcam, &output)
                            .await
                            .map_err(|e| e.to_string())?;

                        // Push current config so pipeline doesn't start with defaults
                        proxy
                            .set_effect(effect.as_str())
                            .await
                            .map_err(|e| e.to_string())?;
                        proxy
                            .set_blur_intensity(blur)
                            .await
                            .map_err(|e| e.to_string())?;
                        let [r, g, b] = bg_color;
                        proxy
                            .set_background_color(r, g, b)
                            .await
                            .map_err(|e| e.to_string())?;
                        if !bg_image.is_empty() {
                            proxy
                                .set_background_image(&bg_image)
                                .await
                                .map_err(|e| e.to_string())?;
                        }
                        Ok(())
                    });
                }
            }
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::ToggleWebcamRevealer => {
                self.is_open = if self.is_open == IsOpen::Webcam {
                    IsOpen::None
                } else {
                    IsOpen::Webcam
                };
            }
            Message::SelectWebcam(idx) => {
                self.is_open = IsOpen::None;
                if let Some((path, _)) = self.capture_devices.get(idx) {
                    self.config.webcam_device = path.clone();
                    self.save_config();
                    if self.pipeline_running {
                        let path = path.clone();
                        return daemon_call(move |proxy| async move {
                            proxy.set_webcam(&path).await.map_err(|e| e.to_string())
                        });
                    }
                }
            }
            Message::ToggleEffectRevealer => {
                self.is_open = if self.is_open == IsOpen::Effect {
                    IsOpen::None
                } else {
                    IsOpen::Effect
                };
            }
            Message::SetEffectMode(idx) => {
                self.is_open = IsOpen::None;
                let mode = match idx {
                    1 => EffectMode::Blur,
                    2 => EffectMode::Replace,
                    3 => EffectMode::SolidColor,
                    _ => EffectMode::None,
                };
                self.config.effect_mode = mode;
                self.save_config();
                let mode_str = mode.as_str().to_string();
                return daemon_call(move |proxy| async move {
                    proxy.set_effect(&mode_str).await.map_err(|e| e.to_string())
                });
            }
            Message::SetBlurIntensity(val) => {
                self.config.blur_intensity = val as u32;
                if self.pipeline_running {
                    let intensity = val as u32;
                    return daemon_call(move |proxy| async move {
                        proxy
                            .set_blur_intensity(intensity)
                            .await
                            .map_err(|e| e.to_string())
                    });
                }
            }
            Message::BlurIntensityReleased => {
                self.save_config();
            }
            Message::ToggleAutoStart(val) => {
                self.config.auto_start = val;
                self.save_config();
            }
            Message::ChooseBackgroundImage => {
                self.file_chooser_open = true;
                return cosmic::task::future(async {
                    let result = file_chooser::open::Dialog::new()
                        .title("Choose Background Image")
                        .filter(
                            file_chooser::FileFilter::new("Images")
                                .mimetype("image/png")
                                .mimetype("image/jpeg")
                                .mimetype("image/webp")
                                .mimetype("image/bmp"),
                        )
                        .open_file()
                        .await
                        .map(|response| response.url().to_owned());

                    Message::BackgroundImageResult(Arc::new(result))
                });
            }
            Message::BackgroundImageResult(result) => {
                self.file_chooser_open = false;
                let reopen = self.open_popup();
                match result.as_ref() {
                    Ok(url) => {
                        if let Ok(path) = url.to_file_path() {
                            let path_str = path.to_string_lossy().to_string();
                            self.config.background_image_path = path_str.clone();
                            self.save_config();
                            return Task::batch([
                                reopen,
                                daemon_call(move |proxy| async move {
                                    proxy
                                        .set_background_image(&path_str)
                                        .await
                                        .map_err(|e| e.to_string())
                                }),
                            ]);
                        }
                    }
                    Err(file_chooser::Error::Cancelled) => {}
                    Err(e) => {
                        tracing::error!("File chooser error: {e:?}");
                    }
                }
                return reopen;
            },
            Message::ToggleColorPicker => {
                self.is_open = if self.is_open == IsOpen::ColorPicker {
                    IsOpen::None
                } else {
                    IsOpen::ColorPicker
                };
            }
            Message::ColorPickerUpdate(update) => {
                // Intercept events that would auto-close the picker — we manage
                // open/close via the revealer toggle instead.
                let skip = matches!(
                    update,
                    ColorPickerUpdate::ToggleColorPicker | ColorPickerUpdate::Cancel
                );
                if skip {
                    return Task::none();
                }

                // Extract live color from ActiveColor before the model consumes the update
                let live_rgb = if let ColorPickerUpdate::ActiveColor(hsv) = &update {
                    let srgb: palette::Srgb = (*hsv).into_color();
                    Some([
                        (srgb.red * 255.0) as u8,
                        (srgb.green * 255.0) as u8,
                        (srgb.blue * 255.0) as u8,
                    ])
                } else {
                    None
                };

                let is_final = matches!(
                    update,
                    ColorPickerUpdate::AppliedColor | ColorPickerUpdate::ActionFinished
                );

                let task = self.color_picker.update::<Message>(update);

                // On final apply: save config + send to daemon
                if is_final {
                    if let Some(color) = self.color_picker.get_applied_color() {
                        let r = (color.r * 255.0) as u8;
                        let g = (color.g * 255.0) as u8;
                        let b = (color.b * 255.0) as u8;
                        self.config.background_color = [r, g, b];
                        self.save_config();
                        return Task::batch([
                            task.map(cosmic::Action::App),
                            daemon_call(move |proxy| async move {
                                proxy
                                    .set_background_color(r, g, b)
                                    .await
                                    .map_err(|e| e.to_string())
                            }),
                        ]);
                    }
                }

                // On drag: send live color to daemon (no config save)
                if let Some([r, g, b]) = live_rgb {
                    if self.pipeline_running {
                        return Task::batch([
                            task.map(cosmic::Action::App),
                            daemon_call(move |proxy| async move {
                                proxy
                                    .set_background_color(r, g, b)
                                    .await
                                    .map_err(|e| e.to_string())
                            }),
                        ]);
                    }
                }

                return task.map(cosmic::Action::App);
            }
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    self.preview_handle = None;
                    let disable_preview = daemon_call(|proxy| async move {
                        proxy
                            .set_preview_enabled(false)
                            .await
                            .map_err(|e| e.to_string())
                    });
                    Task::batch([destroy_popup(p), disable_preview])
                } else {
                    self.open_popup()
                }
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                    self.preview_handle = None;
                    return daemon_call(|proxy| async move {
                        proxy
                            .set_preview_enabled(false)
                            .await
                            .map_err(|e| e.to_string())
                    });
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

// ── D-Bus helpers ────────────────────────────────────────────────────────────

async fn get_proxy() -> Result<WebcamEffectsDaemonProxy<'static>, String> {
    let conn = zbus::Connection::session()
        .await
        .map_err(|e| e.to_string())?;
    WebcamEffectsDaemonProxy::new(&conn)
        .await
        .map_err(|e| e.to_string())
}

/// Send a D-Bus method call to the daemon, mapping the result to a Message.
fn daemon_call<F, Fut>(f: F) -> Task<cosmic::Action<Message>>
where
    F: FnOnce(WebcamEffectsDaemonProxy<'static>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send,
{
    cosmic::task::future(async move {
        let result = match get_proxy().await {
            Ok(proxy) => f(proxy).await,
            Err(e) => Err(e),
        };
        Message::DaemonCallResult(result)
    })
}

// ── D-Bus signal subscription ────────────────────────────────────────────────

fn daemon_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        futures_util::stream::once(async {
            let (tx, rx) = tokio::sync::mpsc::channel::<Message>(32);
            tokio::spawn(daemon_task(tx));
            futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|msg| (msg, rx))
            })
        })
        .flatten()
    })
}

async fn daemon_task(tx: tokio::sync::mpsc::Sender<Message>) {
    loop {
        if let Err(e) = daemon_stream(&tx).await {
            tx.send(Message::DaemonError(format!("Daemon: {e}")))
                .await
                .ok();
        }
        // Brief pause before reconnecting.
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn daemon_stream(tx: &tokio::sync::mpsc::Sender<Message>) -> Result<(), String> {
    let conn = zbus::Connection::session()
        .await
        .map_err(|e| e.to_string())?;
    let proxy = WebcamEffectsDaemonProxy::new(&conn)
        .await
        .map_err(|e| e.to_string())?;

    // Sync initial state so the icon is correct on startup.
    if let Ok(state) = proxy.current_state().await {
        tx.send(Message::DaemonStateChanged(state)).await.ok();
    }

    let mut state_stream = proxy
        .receive_state_changed()
        .await
        .map_err(|e| e.to_string())?;
    let mut error_stream = proxy
        .receive_pipeline_error()
        .await
        .map_err(|e| e.to_string())?;
    let mut fps_stream = proxy
        .receive_fps_updated()
        .await
        .map_err(|e| e.to_string())?;
    let mut preview_stream = proxy
        .receive_preview_frame()
        .await
        .map_err(|e| e.to_string())?;

    loop {
        tokio::select! {
            Some(sig) = state_stream.next() => {
                if let Ok(args) = sig.args() {
                    tx.send(Message::DaemonStateChanged(args.state().to_string())).await.ok();
                }
            }
            Some(sig) = error_stream.next() => {
                if let Ok(args) = sig.args() {
                    tx.send(Message::DaemonError(args.message().to_string())).await.ok();
                }
            }
            Some(sig) = fps_stream.next() => {
                if let Ok(args) = sig.args() {
                    tx.send(Message::DaemonFpsUpdated(*args.fps())).await.ok();
                }
            }
            Some(sig) = preview_stream.next() => {
                if let Ok(args) = sig.args() {
                    let data = Arc::new((
                        *args.width(),
                        *args.height(),
                        args.jpeg_data().to_vec(),
                    ));
                    // Use try_send to drop frames if UI can't keep up
                    tx.try_send(Message::DaemonPreviewFrame(data)).ok();
                }
            }
            else => break,
        }
    }

    Ok(())
}

// ── UI helpers ───────────────────────────────────────────────────────────────

fn revealer_head(
    title: String,
    selected: String,
    toggle: Message,
) -> cosmic::widget::Button<'static, Message> {
    menu_button(
        widget::column()
            .push(widget::text::body(title).width(Length::Fill))
            .push(widget::text::caption(selected)),
    )
    .on_press(toggle)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

impl AppModel {
    fn open_popup(&mut self) -> Task<cosmic::Action<Message>> {
        let enable_preview = daemon_call(|proxy| async move {
            proxy
                .set_preview_enabled(true)
                .await
                .map_err(|e| e.to_string())
        });
        let refresh_devices = cosmic::task::future(async {
            match get_proxy().await {
                Ok(proxy) => match proxy.enumerate_capture_devices().await {
                    Ok(devices) => Message::DaemonDevices(devices),
                    Err(e) => Message::DaemonError(format!("Device enum: {e}")),
                },
                Err(e) => Message::DaemonError(e),
            }
        });
        let query_gpu = cosmic::task::future(async {
            match get_proxy().await {
                Ok(proxy) => match proxy.gpu_enabled().await {
                    Ok(enabled) => Message::DaemonGpuEnabled(enabled),
                    Err(e) => Message::DaemonError(format!("GPU query: {e}")),
                },
                Err(e) => Message::DaemonError(e),
            }
        });

        let new_id = Id::unique();
        self.popup.replace(new_id);
        let mut popup_settings = self.core.applet.get_popup_settings(
            self.core.main_window_id().unwrap(),
            new_id,
            None,
            None,
            None,
        );
        popup_settings.positioner.size_limits = Limits::NONE
            .max_width(480.0)
            .min_width(360.0)
            .min_height(200.0)
            .max_height(1080.0);
        Task::batch([get_popup(popup_settings), enable_preview, refresh_devices, query_gpu])
    }

    fn save_config(&self) {
        if let Some(ref helper) = self.config_helper {
            if let Err(err) = self.config.write_entry(helper) {
                tracing::error!(?err, "error writing config");
            }
        }
    }
}
