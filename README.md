<div align="center">

# screenIt

**Fast, native screen capture, recording and annotation for Windows, written in pure Rust.**

[![CI](https://github.com/Busbar2297/screenit/actions/workflows/ci.yml/badge.svg)](https://github.com/Busbar2297/screenit/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D6)
![Rust](https://img.shields.io/badge/rust-1.90%2B-orange)
![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue)

<img src="docs/images/capture-bar.png" alt="screenIt capture bar" width="720">

</div>

screenIt is a small, single-binary capture tool: press **Print Screen**, drag over anything, mark it
up and share it. It also records your screen to MP4 with microphone, system audio and webcam, and
every action can be scripted from the command line or by an AI agent.

No Electron, no webview, no ffmpeg: one native executable that uses the capture, video and OCR
components already built into Windows.

## Features

| | |
|---|---|
| **Capture** | Region, window, full screen, scrolling, menu, object, multi-area and freehand capture. Crosshair overlay with an 8x magnifier and size badge. Print Screen works even when another tool has claimed the key. |
| **Annotate** | Arrows, shapes, text, callouts, numbered steps, highlighter, blur, pen, stamps, magnifier, crop and cut-out. Everything stays editable until you export. Unlimited undo/redo. |
| **Effects** | Borders, shadows, torn edges, reflection, spotlight, watermark, perspective and page curl. |
| **Smart tools** | Grab text with OCR (offline Windows OCR in English and Arabic, or optionally a local vision model through LM Studio), Smart Redact (emails, phone numbers, card numbers, IPs, URLs, your own terms), background removal. |
| **Record** | MP4 (H.264) of an area, a window or a whole monitor, with microphone, system audio, webcam picture-in-picture and cursor effects. A floating pause/stop toolbar that never shows up in the video. |
| **Edit video** | Trim, cut, combine, export to GIF, make a video from images, export MP4. |
| **Organise** | Library grouped by day with thumbnails, tags and search. Saves go to `Pictures\screenIt` (OneDrive-aware). |
| **Export** | PNG, JPEG, GIF, BMP, TIFF, WebP, PDF, clipboard, or the editable `.scit` project format. |
| **Automate** | Every button is a command, reachable from `screenit-cli`, a local JSON control channel and an MCP server. |

<div align="center">
<img src="docs/images/editor.png" alt="screenIt editor with annotations" width="860">
</div>

## Download

Grab `screenit-v0.1.0-windows-x64.zip` from the [Releases](https://github.com/Busbar2297/screenit/releases)
page. Either run the `.msi` (per-user install, no admin rights, starts minimized with Windows; pass `STARTUP=0` to skip that) or unzip the portable zip anywhere and run `screenit.exe`. Turn *Start with Windows* on or off in Settings; `screenit.exe --minimized` starts straight into the tray. The MSI is unsigned, so SmartScreen may warn.

Requirements: Windows 10 (1903 or later) or Windows 11, 64-bit.

## Using screenIt

| Action | How |
|---|---|
| Capture | **Print Screen**, or the big **Capture** button. Drag a region or click a window. Esc or right-click cancels. |
| Full screen | **Ctrl+Shift+F** |
| Record | **Record** tile. Toggle Microphone, System audio, Webcam and Cursor effects first, then drag an area, click a window, or press Enter for the whole monitor. |
| Editor tools | **V** select, **A** arrow, **L** line, **R** rectangle, **E** ellipse, **T** text, **S** step, **H** highlight, **B** blur, **P** pen, **C** crop |
| Undo / redo | **Ctrl+Z** / **Ctrl+Y** |
| Export PNG | **Ctrl+S**, or **Export PNG** in the top bar |
| Zoom | **Ctrl+wheel**, **Ctrl+0** fit, **Ctrl+1** actual size |
| Back to the capture bar | **Ctrl+Shift+B** |

Closing the window keeps screenIt running in the system tray, so Print Screen keeps working.
Use **Quit** in the tray menu to exit. Launching it again just brings the running window forward.

## Build from source

You need the Rust stable MSVC toolchain and the Visual Studio Build Tools with the Windows SDK.

```powershell
git clone https://github.com/Busbar2297/screenit
cd screenit
cargo build --release -p screenit -p screenit-cli
.\target\release\screenit.exe
```

Create the portable release folder and zip (`release/screenit-v<version>-windows-x64/`):

```powershell
cargo xtask package   # portable zip into release/
cargo xtask msi       # also builds the MSI (downloads the free WiX Toolset v3 on first use)
```

## Command line and automation

```powershell
screenit-cli commands --ids                                   # every command id
screenit-cli exec capture.fullscreen                          # run one command
screenit-cli run '[["capture.fullscreen",{}],["doc.export",{"path":"shot.png"}]]'
screenit-cli mcp                                              # MCP server over stdio
```

The running app also listens on a token-protected loopback channel. See
[docs/control-protocol.md](docs/control-protocol.md).

## Project layout

screenIt is a layered Cargo workspace. The engine is headless and fully tested; the UI is a thin
[egui](https://github.com/emilk/egui) shell on top of it, and all Windows-specific (and all
`unsafe`) code lives in one crate, `screenit-platform`.

```text
apps/       screenit (desktop app), screenit-cli, screenit-fixture (test window)
crates/     geom raster codecs doc ops algo vector compose format capture record
            library platform engine ui-egui automation testkit
xtask/      layers, ci, parity, scorecard, panic-hunt, perf, package
```

More detail: [architecture](docs/architecture.md), [development](docs/development.md),
[known gaps](docs/gaps.md), [feature checklist](docs/parity-checklist.md).

## Contributing

Issues and pull requests are welcome. Before opening a PR:

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask ci          # add --e2e for capture/recording changes (needs a desktop session)
```

[AGENTS.md](AGENTS.md) describes the project rules (no panics in non-test code, `unsafe` only in
the platform crate, every feature is a command with tests).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option. Third-party notices: [ATTRIBUTION.md](ATTRIBUTION.md).

Snagit is a trademark of TechSmith Corporation. screenIt is an independent project and is not
affiliated with or endorsed by TechSmith.
