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

## AI tutorials

`screenit-tutor` (L4) holds the tutorial script, selectors, safety guard, player, rehearse/perform
and the production plan behind traits (`UiTree`, `InputDriver`, `AppLauncher`, `LlmChat`, `Pace`).
`platform::automation` implements them with UI Automation and SendInput; `llm` implements
`LlmChat` for LM Studio. Engine commands `ui.*`, `input.*`, `app.launch` and `tutor.*` run
long steps as background jobs (`tutor.status`, `tutor.wait`, `tutor.abort`). `tutor.explore`
(`tutor::explore`) builds the script from the live app instead of guessing it: a storyboard of
chapters, then one pointer move per turn. The model sees the window (picture plus controls) and
answers with a mouse or keyboard instruction; points are 0–1000 inside that picture, so a resized
screenshot still lands. A title change is kept, a dialog is followed and a closed one returns to
the app the run started on. A move is kept only when the screen changed; when stuck or before a
risky step it asks the user (`tutor.answer`).
Flow: explore (or plan), rehearse
(a failed step is repaired from a screenshot of the window in front, and the corrected step is
written back into the script), perform (one recording per chapter), produce (chapter cards,
idle trim, zoom, click rings, captions, SRT).
