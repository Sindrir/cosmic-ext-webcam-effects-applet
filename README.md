# Webcam Effects for COSMIC™

**Unofficial third-party applet.** This project is not affiliated with or endorsed by System76. COSMIC™ is a trademark of System76.

> **Vibe coded.** This project was heavily vibe coded with [Claude](https://claude.ai). Expect rough edges.

A COSMIC™ desktop applet that applies real-time background effects to your webcam feed. The processed video is written to a virtual camera device ([v4l2loopback](https://github.com/umlaeute/v4l2loopback)), making it available to any video conferencing application as a regular webcam.

## Features

- **Background blur** — adjustable intensity
- **Background replacement** — use any image as your backdrop
- **Solid color background** — with a live color picker
- **Live preview** — see the processed output directly in the applet popup
- **GPU acceleration** — CUDA when available, automatic CPU fallback
- **Hot-swap devices** — change webcam or output device while running
- **Auto-start** — optionally start the pipeline when the applet loads
- **Persistent settings** — effect mode, blur intensity, background image/color saved via `cosmic-config`

## How it works

The applet is split into two binaries communicating over D-Bus:

| Binary | Role |
|---|---|
| `cosmic-ext-webcam-effects-applet` | Panel applet UI (libcosmic) |
| `cosmic-ext-webcam-effects-applet-daemon` | Video pipeline (D-Bus activated) |

The daemon captures frames from a real webcam via [V4L2](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/v4l2.html), runs person segmentation using the [RVM (Robust Video Matting)](https://github.com/PeterL1n/RobustVideoMatting) MobileNetV3 ONNX model, composites the chosen background effect, and writes the result to a v4l2loopback virtual camera device.

## Requirements

### V4L2 devices

This applet uses the [Video4Linux2](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/v4l2.html) (V4L2) API for both **capture** (reading from your real webcam) and **output** (writing to a virtual camera). Your real webcam is accessed through its `/dev/videoN` device node.

For **output**, the applet requires the [v4l2loopback](https://github.com/umlaeute/v4l2loopback) kernel module, which creates a virtual V4L2 device. This virtual camera is what video conferencing apps (Zoom, Teams, Google Meet, etc.) see as a webcam source. Without v4l2loopback loaded, the applet has nowhere to write its processed frames — the start button will be disabled if no output device is detected.

#### NixOS

Add to your system configuration:

```nix
boot = {
  extraModulePackages = [ config.boot.kernelPackages.v4l2loopback ];
  kernelModules = [ "v4l2loopback" ];
  extraModprobeConfig = ''
    options v4l2loopback devices=1 video_nr=2 card_label="Virtual Webcam" exclusive_caps=1
  '';
};
```

Then rebuild: `sudo nixos-rebuild switch`

#### Other distros

Install the `v4l2loopback` package from your distribution, then load it:

```sh
sudo modprobe v4l2loopback devices=1 video_nr=2 card_label="Virtual Webcam" exclusive_caps=1
```

To make it persist across reboots, add `v4l2loopback` to your modules configuration.

### ONNX Runtime

The segmentation model runs via [ONNX Runtime](https://onnxruntime.ai/). The `ort` crate loads `libonnxruntime.so` dynamically at startup. Ensure it is available in your library path. The included `flake.nix` handles this for development.

For GPU acceleration, CUDA and cuDNN libraries must be present. The daemon will log whether it is using GPU or CPU on startup and the applet displays a badge in the popup.

## Development

A `flake.nix` is provided with all build and runtime dependencies:

```sh
nix develop
just build-debug
```

For rapid iteration with hot-reload (installs user-local desktop/service files, no sudo):

```sh
just dev
```

## Building & Installing

A [justfile](./justfile) is included:

| Recipe | Description |
|---|---|
| `just` | Build release binaries |
| `just run` | Build and run the applet |
| `just install` | Install binaries, desktop entry, D-Bus service, icon, and metainfo |
| `just dev` | Debug build with user-local hot-reload |
| `just check` | Run clippy with pedantic warnings |
| `just vendor` | Vendor dependencies for offline builds |
| `just uninstall` | Remove installed files |

## D-Bus Interface

The daemon exposes `dev.sindrir.CosmicExtWebcamEffectsApplet1` on the session bus:

**Methods:** `Start`, `Stop`, `SetEffect`, `SetBlurIntensity`, `SetBackgroundImage`, `SetBackgroundColor`, `SetWebcam`, `SetOutput`, `SetPreviewEnabled`, `CurrentState`, `CurrentFps`, `EnumerateCaptureDevices`, `GpuEnabled`

**Signals:** `StateChanged`, `PipelineError`, `FpsUpdated`, `PreviewFrame`

## Documentation

Refer to the [libcosmic API documentation](https://pop-os.github.io/libcosmic/cosmic/) and [book](https://pop-os.github.io/libcosmic-book/) for help building applets with [libcosmic](https://github.com/pop-os/libcosmic/).

## License

[GPL-3.0](https://www.gnu.org/licenses/gpl-3.0.en.html)
