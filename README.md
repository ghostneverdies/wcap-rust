# wcap — Rust fork

A Windows screen recorder based on [wcap](https://github.com/mmozeiko/wcap) by [mmozeiko](https://github.com/mmozeiko).

This fork ports the original C implementation to Rust and adds a compact desktop interface, updated recording controls, and fixes to frame pacing and resource handling. It keeps the original approach: Windows.Graphics.Capture, Direct3D 11, WASAPI, and Media Foundation.

## Features

- Record a display, an application window, or a selected area to MP4.
- Compact, fixed-size interface with separate Capture and Settings pages.
- Select an area using a lightly dimmed desktop preview and a crosshair cursor, then start recording separately.
- Mouse input isolation during area selection, released on completion, cancellation, or loss of focus.
- Window picker that filters out common Windows shell and background windows.
- Capture controls and recording settings are disabled during an active recording.
- Native tray icon with recording time and an immediate reset when recording stops.
- Recording folder picker with a folder glyph beside the output path.
- Save settings appears only when values differ from the saved configuration; reverting changes hides it again.
- Hardware or software encoding, resolution and frame-rate limits, and optional gamma-correct resizing.
- System audio or application audio for window recording.
- Recording length and file-size limits, plus optional fragmented MP4 output.

Video options include H.264, HEVC, and AV1, with 10-bit profiles for HEVC and AV1. Audio options include AAC and FLAC. Availability depends on Windows, installed codecs, and the selected GPU or encoder.

## Requirements

- Windows 10 version 1903 or newer, or Windows 11.
- Application-only audio requires Windows build 20348 or newer.
- Some capture options, including recording borders, rounded corners, and secondary windows, require newer Windows versions.

To build from source, install the Rust MSVC toolchain and Visual Studio Build Tools with the **Desktop development with C++** workload and a Windows SDK.

## Build and run

From the repository root:

```powershell
cargo build --release --manifest-path rust/Cargo.toml
.\rust\target\release\wcap.exe
```

Cargo downloads dependencies on the first build. Subsequent builds can use `--offline` once those dependencies are cached.

Alternatively, run `rust\build.bat`. The script builds the release executable and reports an error if Cargo is not on PATH.

## Usage

1. Open **Capture** and choose a display or window, then press its Record button.
2. To record an area, press **Select area**, drag a rectangle, and release. Press **Record area** when ready. Selecting an area does not start recording.
3. Press **Stop recording**, or use any recording shortcut, to finish the file.
4. Open **Settings** to change capture, output, video, or audio preferences. Under **Output**, use the folder button to browse for a recording folder, then press **Save settings**.

Press **Esc** to cancel area selection. Right-click the tray icon for its menu, or double-click it to open Settings.

### Default shortcuts

| Shortcut | Action when idle |
| --- | --- |
| Ctrl + PrintScreen | Record the display under the cursor |
| Ctrl + Win + PrintScreen | Record the active window |
| Ctrl + Shift + PrintScreen | Select an area on the display under the cursor |

While recording, any of these shortcuts stops the current recording.

### Settings and output

Settings are stored in `wcap.ini` beside `wcap.exe`. Recordings default to your Windows Videos folder. The default video settings are H.264, up to 1920 × 1080 at 60 FPS, with AAC audio.

Set maximum width, height, or frame rate to `0` to use the source value. Resizing preserves the aspect ratio. Shortcuts can be changed in the INI file while the app is closed.

## Frame pacing

Capture follows actual Windows capture events. Duplicate and out-of-order timestamps are rejected before advancing the frame limiter. After a stall, expired frame slots are skipped rather than producing a burst of catch-up frames.

Idle stream ticks maintain the recording timeline without generating duplicate images. Audio uses the same clock as video, and submitted encoding buffers are reused only after the encoder releases them.

## Development

```powershell
cargo check --manifest-path rust/Cargo.toml
cargo test --manifest-path rust/Cargo.toml
```

Tests cover frame pacing, audio timing and buffering, configuration, GPU conversion, MP4 encoding and decoding, and picker input isolation. The native input test verifies that clicks and scrolling are blocked even when another window owns mouse capture, and that normal input resumes after selection closes.

Hardware encoder and codec support varies by machine. For implementation and validation details, see [the Rust development notes](rust/README.md).

### Repository layout

```text
rust/       Rust application, assets, and development tools
wcap src/   Original upstream source, documentation, and license
```

## Credits

The original wcap project, recording architecture, shaders, and app icons are by [mmozeiko](https://github.com/mmozeiko/wcap). This is an independent fork.
