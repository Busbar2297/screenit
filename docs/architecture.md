# screenIt architecture

Engine-first, headless-first, everything is a command, layered workspace, CPU reference
compositor as oracle, thin data-driven UI.

## Seam

```text
clients (ui-egui, cli, control channel, MCP) -> engine::Session (command registry, jobs, events)
   -> capture / record / library models -> traits implemented by platform (Win32) or testkit (fake)
   -> ops (history) -> doc (base raster + annotation objects) -> compose (flatten) -> format/codecs
```

`Session::dispatch(CommandInvocation{id, params})` is the only entry point. `Session::services`
holds trait objects (`ScreenSource`, `WindowSource`, `Clipboard`, `Clock`) injected by the app.

## Document model

`doc::CaptureDoc` = one base `Raster` (RGBA8) + ordered `Vec<Object>` (arrow, line, text, callout,
shape, stamp, step, blur, magnify, highlight, pen, fill, cutout) + `Metadata` (app, window title,
url, time, monitor, dpi). Objects stay editable until export; `compose::flatten` is the oracle that
renders the final pixels. `.scit` (legacy `.sxs` still opens) stores manifest.json + base.png (+ assets) in a zip.

## Capture

`capture` holds the selection model (region, window, fullscreen, scrolling, menu, object, multi-area,
freehand, fixed, last region), zones math across monitors, presets, hotkey table, the scrolling
stitcher and capture-time effects. Frames come from `ScreenSource`, implemented in `platform` with
Windows Graphics Capture / GDI. Tests use `testkit::FakeScreen` and the fixture window.

## Recording

`record` holds the session state machine (countdown, recording, paused), timeline, PiP layout,
screen-draw objects, click ripples, edit lists (trim/cut/combine). `platform` implements the encode
pipeline: GDI-polled frames (+ webcam PiP, cursor effects) -> Media Foundation H.264/HEVC ->
IMFSinkWriter MP4, with WASAPI microphone / system-audio capture mixed into an AAC track.

## Unsafe isolation

Only `screenit-platform` may use `unsafe`. Everything else is `forbid`.
