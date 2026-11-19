# Changelog

## Unreleased

## 0.2.32

- Built-in Whisper transcription: download a model once in **Settings > Speech > Transcription** (tiny, base or small) and Transcribe works offline with real accuracy, punctuation and many languages. **Transcribe with** picks Automatic, Whisper, the Windows recognizer or a speech server.

## 0.2.31

- New **Steps** row in the editor's Speech area: add Reduce noise, Transcribe, Cut silences and Burn subtitles in any order and run them in one go. Each step works on the previous result; a transcript is made automatically when a later step needs one; only the final video is kept.
- The Windows recognizer hears quiet recordings better: speech is raised to a steady level before recognition.

## 0.2.30

- Transcribe works without a server: with the speech-to-text URL empty (or `windows`) screenIt uses the recognizer built into Windows, offline (basic accuracy). A configured server still wins.
- The Library opens maximized from every view, and Settings/About open at their proper size (the window is now maximized and restored directly).

## 0.2.29

- Typing "windows" in the Text-to-speech URL box now selects the built-in Windows voice (an empty box did already). Settings apply at once; no restart needed.

## 0.2.28

- Zoom on clicks works again: a click whose element is the whole window zooms on the click point, and clicks outside the recorded area are ignored.
- The Auto-polish bar is easier to read: visible checkboxes and buttons, grouped rows, tooltips, and a clear **Render polished video** button.
- New guide docs/audio.md: what each sound feature needs and how to set up a speech server.

## 0.2.27

- Dragging the recording bar follows the mouse smoothly across monitors, and the bar always stays inside a screen.

## 0.2.26

- Recording waits for Start; the recording bar can be dragged, made small, and stays above the taskbar and in view across monitors.

## 0.2.25

- The recording bar keeps its size when dragged between monitors with different scaling.

## 0.2.24

- Rebuild of 0.2.23 with no further changes.

## 0.2.23

- The recording bar can be dragged to another monitor and stays there, and a small-bar button shrinks it to timer and Stop so it stays out of the way.

## 0.2.22

- The recording bar stays above the taskbar when the area is the whole screen.

## 0.2.21

- Recording waits for you to press Start (or Enter) after you pick the area, then counts down.
- The recording bar stays on one monitor and in view when the area spans several, so Pause and Stop are always reachable.
- Returning from the tutorial or editor to the capture bar restores its size.

## 0.2.20

- Starting with Windows puts screenIt in the notification area again, with no taskbar button, and Print Screen captures right away.
- Print Screen opens the capture screen even when the window was minimized.

## 0.2.18

- Starting with Windows minimizes onto the taskbar without a click, and Print Screen opens the capture screen before the window has ever been focused.

## 0.2.17

- Starting minimized no longer hides the window off-screen. It stays on the taskbar, and clicking that button opens it.

## 0.2.16

- Starting with Windows opens minimized on the taskbar. Clicking that button restores the window.

## 0.2.15

- Maximizing and then minimizing no longer leaves the window stuck on the previous frame.
- Starting with Windows no longer leaves the taskbar button highlighted on a window that cannot be focused. Clicking it opens the window.
- Right-click a capture in the library to copy it. A video copies its first frame.

## 0.2.14

- AI tutorial. Describe a task and screenIt plans it on the live app, asks when it is stuck, records the screen, and puts that video in the library.
- Planning shows a progress bar and saves the plan as a JSON file. Producing the video reads the recording straight through, so a short clip is ready quickly.
- The AI tutorial button keeps its label inside the tile, and the screen says this is an alpha release that is still being tested.

## 0.2.13

- AI tutorial director. Describe what to teach, and a local model plans the steps, rehearses them on the real app, then screenIt records a clean take with chapter cards, captions and zoom.

## 0.2.12

- screenIt can update itself. It checks GitHub Releases at start and once a day, downloads a newer installer in the background, verifies its SHA-256, and offers Restart to update. Settings has an Updates tab, and About has Check for updates.

## 0.2.11

- Opening an image keeps the picture beside the tools and the properties. They no longer cover it.
- Export saves PNG, JPEG, PDF, GIF, BMP, TIFF or WebP.

## 0.2.10

- Draw a text box, type in it, and resize or move it while the outline is showing. The outline goes away when you finish, and only the text stays.
- Select a shape to move, resize or rotate it, and change its colour, line width and fill, or delete it.
- Arabic text in an annotation is joined and reads from the right.

## 0.2.9

- Maximize works in the library. It was shown there but stayed dimmed.

## 0.2.8

- About shows the version, a link to the GitHub repository, and the credits.
- The Maximize button is only on the editor. The capture bar, library, settings and About stay a fixed size.

## 0.2.7

- While a video is recording, a solid red frame marks the area. Drag the frame to move what is being recorded.

## 0.2.6

- Hide secrets now updates the picture. It reported a hidden secret and stored the cover, but the image on screen stayed unchanged.

## 0.2.5

- Zoom in, zoom out and fit are in the editor top bar, including when you open an image from the library. Zoom goes down to 2% so a very tall capture can still be zoomed out.
- The taskbar button, Alt-Tab and the Start menu and desktop shortcuts use the screenIt icon.

## 0.2.4

- Esc now stops a scrolling capture even when you tap it quickly. A long capture also stops at a size limit instead of growing without end.
- Opening a very tall image no longer crashes: the editor splits large images into tiles, and you can zoom out further.
- The installer adds a Desktop shortcut and puts the Start menu shortcut directly in the Start list (pass DESKTOP=0 to skip the desktop one).

## 0.2.3

- Scrolling capture from the capture bar: pick a window or a region and screenIt scrolls and stitches the pages, keeping a sticky header and footer once.
- Hide secrets: cover passwords, keys and tokens. A button in the editor, plus an optional automatic pass on every new capture (Settings, Privacy, off by default). An optional local LM Studio pass can catch secrets that are only words.
- Videos play with their sound inside screenIt.
- The Microphone and System audio switches follow the saved settings.

## 0.2.2

- Arabic now reads right to left everywhere: letters join, Latin words such as "Print Screen" stay in order, and the capture button, status line, buttons, drop-downs, sidebar, library cards and the notification are anchored on the right.
- Fixed the root cause: the text renderer reverses whole right-to-left lines, which scrambled text that had already been put in display order.
- README: new screenshots and a language section.

## 0.2.1

- Grab text from the open image or from a screen area. The text is copied to the clipboard, saved in the library, and a notification shows the result.
- Paste works in the LM Studio server URL and API key fields, with a Paste button beside each.
- Arabic interface text uses a font that contains Arabic letters, so it no longer draws as boxes.

## 0.2.0

- Settings view with saved preferences; after a capture, open the editor or save silently.
- Start with Windows (minimized to the tray), per-user MSI installer (unsigned) and portable zip.
- Text recognition in Arabic and English, optional LM Studio vision model.
- 28 interface languages with right-to-left Arabic and editable translation files.
- 2K/4K recording: output size presets, GPU desktop duplication, hardware encoder, pipelined capture.

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
- 2K and 4K recording: output size presets (native, 1080p, 1440p, 2160p, aspect ratio kept), GPU desktop duplication (DXGI) with a GDI fallback, hardware encoder when available, H.264 level 4.0 to 5.2 and up to 100 Mbit/s.
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
- 28 interface languages (all 24 EU languages, Arabic with right-to-left layout, Russian, Japanese, Simplified Chinese). Translations are plain TOML files you can edit and reload without restarting.
- Per-user MSI installer (`cargo xtask msi`, WiX v3, unsigned).
- No audio playback in the built-in video player; no microphone/camera picker in the UI yet.
- Recording is GDI-polled (about 25 fps at 720p).
