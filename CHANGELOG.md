# Changelog

## 0.1.0 (first public release)

### Capture
- Region, window, fullscreen, scrolling, menu, object, multi-area and freehand capture with a
  crosshair overlay, magnifier and size badge.
- Print Screen works even while another capture tool holds the key; capture presets and global
  hotkeys.
- Text grab (OCR), time-lapse, import from file or clipboard.

### Editor
- Arrow, line, rectangle, ellipse, text, callout, step, highlight, blur, pen, stamp, magnify,
  crop, cut-out and fill, all editable until export, with undo/redo.
- Effects: borders, shadows, torn edges, reflection, spotlight, watermark, perspective, page curl.
- Smart Redact (emails, phone numbers, cards, IPs, URLs, custom terms), OCR text replace,
  background removal.

### Recording
- MP4 (H.264) screen recording of an area, a window or a whole monitor.
- Microphone and system audio, webcam picture-in-picture, cursor highlight and click ripples.
- Countdown, floating pause/resume/stop toolbar that never appears in the recording.
- Trim, cut, combine, GIF export, video from images, Export MP4.

### Organise and share
- Library grouped by day with thumbnails, tags and search.
- Native `.scit` project format; PNG, JPEG, GIF, BMP, TIFF, WebP and PDF export.
- Save As dialogs, full saved path in the status bar and "Show in folder".

### Automation
- Every action is a command, available from `screenit-cli`, a local JSON control channel and an
  MCP server.

### Known gaps
- Start with Windows (minimized to the tray) from Settings, for the portable EXE and the installer.
- Per-user MSI installer (`cargo xtask msi`, WiX v3, unsigned).
- No audio playback in the built-in video player; no microphone/camera picker in the UI yet.
- Recording is GDI-polled (about 25 fps at 720p).
