# Dev log

## M0 start
- Workspace of 17 crates + 3 apps + xtask compiles. AGENTS.md and docs written.

## Engine + harness
- Engine command modules (doc, editor, image, library, record, system, capture) + control protocol complete and tested against FakeScreen.
- testkit: golden diff (check_golden / assert_image_eq!), FakeScreen/Clipboard/Recorder, fake_session.
- CLI: commands --ids, exec, serve. panic_hunt + perf tests. xtask ci green.

## M4 recording
Media Foundation H.264 MP4 recorder (GDI-polled, pause-aware timestamps, window follow, cursor highlight/ripples) and IMFSourceReader frame reader; e2e records the fixture and decodes a frame.

## M5 webcam
CameraSource trait, composite_webcam (PiP shapes/corners/size, swap), record.pip/record.swap commands, MfRecorder compositing, FakeCamera.

## M6 effects
algo::fx (edges, reflection, spotlight, fade, watermark, perspective, page curl, canvas, trim, combine), codecs::encode_pdf, commands image.* + batch.convert + PDF export. Gaps: capture-info stamp, templates, print, bevel edges, true perspective quad, curl flap.

## M7 video editing
VideoWriter trait + MfVideoWriter; EditList-driven render (trim/cut/combine) via cached IMFSourceReader; animated GIF; video from images; e2e verifies durations. Gaps: narration, re-encodes (no lossless trim).

## M8 library/MCP/packaging
MCP stdio server (engine::mcp, cli mcp), library.share, egui library panel + video transport bar, xtask package (portable zip). Gaps: MSI installer, share-to-app.

## M9 ML-adjacent
Smart Redact (OCR + email/phone/card(Luhn)/ssn/ip/url scanners + custom terms, span-level boxes), OCR text replace, classical background removal (corner flood), time-lapse to MP4. Gaps: Simplify, Smart Move, printer capture; redact is OCR-accuracy bound; background removal is colour-based, not ML.

## UX pass
- Compact capture bar + editor + library (grouped by day/time, thumbnails) in one window; no console window; close/minimize parks in tray (off-screen tool window); tray All-in-One works while parked.
- Overlay: Esc / right-click cancels; HiDPI 1:1 rendering; drag previews match final shapes.
- Print Screen: registry PrintScreenKeyForSnippingEnabled=0 on start.

## UI revamp + Print Screen while another tool owns it
- theme.rs (dark palette, violet-to-cyan accent), icons.rs (painter-drawn vector icons, no assets), chrome.rs (launcher tiles, editor top bar, tool palette with single-key shortcuts, properties panel, status bar). Canvas centred with fit-to-window; Ctrl+wheel zoom. Overlay: hint pill, size badge, corner handles, 8x magnifier.
- Print Screen: platform/keyhook.rs. RegisterHotKey fails while another capture tool holds the key and WH_KEYBOARD_LL proved unreliable next to other hooks, so Raw Input (RIDEV_INPUTSINK, message-only window) is used; PrtSc often arrives as key-up only, so either edge fires, debounced 600 ms. Probe: `cargo run -p screenit-platform --example prtsc_probe`.
- Still open: when another capture tool also listens, both fire; Raw Input cannot swallow the key.

## Recording with voice and camera
- platform/audio.rs: WASAPI capture (microphone; system audio via render loopback), AUTOCONVERTPCM to 48 kHz stereo s16. record/audio.rs: AudioMixer sums sources onto the video clock. MfRecorder adds an AAC stream; video.probe reports audio tracks.
- platform/camera.rs: MfCamera (MF source reader on the first video capture device, RGB32), opened lazily, released by CameraSource::stop. New command record.devices.
- FIX: recorded MP4s were upside down in other players; writer and reader now both use the MF convention (positive stride = top-down).
- UI (ui-egui/recording.rs): Record tile, option chips (Microphone, System audio, Webcam, Cursor effects), record-mode overlay, 3-2-1 countdown, floating toolbar excluded from capture via WDA_EXCLUDEFROMCAPTURE (set SCREENIT_TOOLBAR_IN_CAPTURE=1 to screenshot it).
- Still open: record hotkeys, mic/camera picker, audio meters, audio playback in the player, region border while recording.

## Save location, Save As, Export MP4, sheared video frames
- Output folder is `<Pictures known folder>\screenIt` (SHGetKnownFolderPath), so it follows OneDrive redirection.
- platform/shell.rs: pictures_dir, output_dir, save_dialog (IFileSaveDialog), reveal (explorer /select).
- New command video.export. Export PNG, Save (.scit) and Export MP4 open a native Save As dialog; the status bar shows the full saved path plus "Show in folder".
- FIX: frames decoded from recordings whose width is not a multiple of 16 were sheared. The reader re-reads the output type after decoding, takes the pitch from IMF2DBuffer2::Lock2DSize and crops to MF_MT_MINIMUM_DISPLAY_APERTURE. e2e: odd_width_recording_decodes_without_shear.

## Naming and file format
- Product name **screenIt**; crates/binaries screenit-* / screenit; env vars SCREENIT_TOOLBAR_IN_CAPTURE, SCREENIT_BLESS, SCREENIT_HUNT_ROUNDS.
- Native format .scit (screenit_format::EXT); early .sxs project files still open (is_project).

## Dark title bar on launch, single instance
- platform::window::dark_title_bar sets DWMWA_USE_IMMERSIVE_DARK_MODE and forces SWP_FRAMECHANGED on the first frame (otherwise the caption stays light until the next activation change).
- platform/instance.rs: named mutex + auto-reset event. A second launch grants AllowSetForegroundWindow, signals the event and exits; the first instance shows and activates its window.

## Settings
- engine/settings.rs: versioned serde Settings (general, recording, ocr, presets) in %LOCALAPPDATA%\screenIt\settings.json, atomic write, corrupt file -> defaults, invalid values repaired. Commands settings.get/set/reset/open; presets persist on save/delete.
- ui-egui: View::Settings (gear in capture bar and top bar), live edits.

## After-capture mode
- general.after_capture editor|silent (Settings view). Silent skips the editor, still saves/copies/adds to the library, and returns the window to where it was; a preset with open_in_editor=false overrides. Settings::opens_editor is the single decision point.

## Start with Windows + MSI
- platform/autostart.rs: HKCU Run value `screenIt` = quoted current exe + --minimized (works for the portable EXE); Settings toggles it via Session.on_autostart (only on change). Registry is the truth at startup.
- --minimized parks the window in the tray on the first frame (created off-screen, no flash).
- `cargo xtask msi`: WiX v3.14 (downloaded to target/wix314), installer/screenit.wxs, per-user, Start Menu shortcut, STARTUP=1 default adds the Run value. Verified install/uninstall cleanup.

## OCR: Arabic/English and LM Studio
- capture: Ocr::recognize_lang/languages, LlmOcr trait, rtl_logical_order (Windows returns RTL words in visual order; reversed per RTL run).
- platform/ocr.rs: language selection (auto/en/ar) via TryCreateFromLanguage, clear error when the pack is missing.
- New crate screenit-llm (L5): OpenAI-compatible /v1/models + /v1/chat/completions, image downscaled to 2048px, 2MB response cap, timeouts; fake-server tests.
- engine: OcrJob (settings ocr.engine windows|llm|windows_then_llm), command ocr.info; UI runs OCR on a worker thread (editor Copy text button) and has a Test connection button. Verified live against LM Studio (gemma-4-e4b) with an Arabic+English image.

## Languages
- New crate screenit-i18n (L0): 28 bundled locale files (en, 24 EU languages, ar, ru, ja, zh-Hans) as flat TOML in crates/i18n/locales; English is the fallback, then the key. Overrides load from <exe>\locales then %LOCALAPPDATA%\screenIt\locales (Settings: Reload / Edit translations).
- Arabic: ar-reshaper (no ligatures: lam-alef glyphs are missing in system fonts) + unicode-bidi, so egui's LTR renderer shows it right; panels and rows mirror for RTL (h_row, start_layout, far_layout). System fallback fonts for ar/ja/zh installed per language.
- cargo xtask i18n checks keys used in code exist in en.toml and every locale is complete; package/msi ship the locales folder.
- Translations are machine-drafted, not natively reviewed.
- Verified visually via PrintWindow: ar (mirrored), ja, ru.

## 2K/4K recording
- record: OutputSize (native|1080p|1440p|2160p; longer side 1920/2560/3840, aspect kept, even, <=8192) on RecordConfig.size; record.start param size defaults from settings recording.output. UI start now also passes settings fps/codec.
- platform/dup.rs: DXGI Desktop Duplication per output (BGRA8 only; rotated/HDR/lost access fall back to GDI after 3 failures). First frame of a static desktop carries no image (LastPresentTime=0), so the staging texture is seeded from one GDI grab. ScreenSource::grab_stream (default = grab) is used by the recorder.
- mf.rs: hardware MFT first above 1080p with software fallback (retry on sharing violation), H.264 High + level by macroblock rate (4.0..5.2), bitrate 0.125 bpp capped at 100 Mbit/s (H.265 x0.6), threaded bilinear scaler, two-stage pipeline (grab | convert+write), 1 ms timer resolution during recording (default tick capped 30 fps loops near 24).
- Measured here: 2560x1600 native 57 fps at target 60, 28 at 30 (variable); 3840x2400 scaled 33 fps at target 45. 2560x1440 and 3840x2160 encode/decode tests check bar positions for shear.
- Examples: dup_probe (GDI vs DXGI ms/frame), rec_probe (record the primary monitor at a size).

## 2026-11-02 - LM Studio API key and Grab Text in the UI
- Added ocr.llm_api_key (Settings > Text recognition, masked field), sent as Bearer token (default lm-studio); test covers header and CR/LF stripping.
- Added Grab text tile on the capture bar and a top-bar button: pick an area or window on the frozen overlay, OCR it, copy to clipboard.
- 6 new i18n keys in all 28 locales (English fallback text except Arabic).
- Still open: translate the new keys; the key is stored in plain text in settings.json.

## 2026-11-02 - Grab text library, paste, Arabic font
- Grab text (open image or screen area) copies to the clipboard, stores a text entry in the library, and shows an always-on-top notice.
- Enabled the egui clipboard feature and added Paste buttons; Ctrl+V was a no-op because clipboard support was compiled out.
- Arabic UI uses Tahoma/Arial as the primary font so presentation forms draw. Latin inside Arabic strings stays in order.
- Still open: translate the newest keys; the API key is plain text in settings.json.

## 2026-11-02 - Arabic reads from the right
- Root cause: epaint 0.37 shapes with harfrust and reverses a whole run whose first letter is RTL (no paragraph bidi). Tahoma covers every character, so a line is one run and our visual-order string was reversed again ("neercS tnirP"). `i18n::display` now = `visual` (shape + bidi) then `for_shaper`, which pre-reverses the line (brackets swapped) so the pixels match; fallback-font locales reverse each Arabic run instead. Tests model the shaper.
- Status text, pills, capture button, combos, sidebar and toast now anchor on the right in Arabic.
- Simplified Arabic was tried as primary font: several presentation forms are empty boxes. Tahoma stays.
- Still open: a visual line that starts RTL but ends with a Latin letter would still be reversed; no such string ships.

## 2026-11-03 - In-app video playback was silent
- Critical: the player only asked for video frames (`video.frame`). The MP4's audio track was never opened, so the file was silent in the app and fine in any other player. `video.audio.play/pause/stop` now decode that track to 48 kHz stereo PCM on its own Media Foundation reader and play it through the default speakers (WASAPI render). Pause, scrub and close stop it; play starts again at the playhead.
- Settings `recording.microphone` and `recording.system_audio` were saved by the Settings page and ignored by the capture bar, which always started with both on. The chips now load from settings and both sides write the same two fields. Defaults stay off. A saved file with both false will show the chips off, so a new recording has no mic or system audio until one of them is turned on. Files already recorded are unchanged.

## 2026-11-03 - Scrolling capture UI and Hide secrets
- Scrolling: the stitcher now keeps sticky headers/footers once, ignores the scrollbar strip and tolerates 1% pixel noise (caret blink). New `capture.scroll.step` scrolls once and grabs one frame; the capture bar's Scrolling tile (and top-bar icon) pick a window/region, then drive begin/step/finish with a progress toast excluded from capture. Esc stops; 30 pages max in the UI.
- Hide secrets: `editor.hide_secrets` (scan + one-step editable blur objects) and `editor.redact.apply` (rects to blur objects). Finds: UI Automation password fields (`ScreenSource::secret_fields`, 300 ms deadline), OCR pattern scan (key/token formats, labelled values such as "Password: ...", masked runs), and an optional LM Studio text pass (`LlmOcr::complete_text`, `privacy.use_llm`, off by default).
- `privacy.auto_hide_secrets` (off by default) burns the cover into the pixels in `finish()` before clipboard/save/library; never uses the model. Settings has a Privacy tab; the editor top bar has a Hide secrets pill (worker thread).
- Still open: the scroll driver relies on wheel notches, so apps that ignore wheel input are not covered; secrets inside images with unusual fonts depend on OCR quality; translate the 21 new keys outside Arabic.

## 2026-11-03 - Scrolling Esc, huge image crash, installer shortcuts (0.2.4)
- Esc was only sampled between capture steps (~0.5 s each), so a tap was missed. `input::escape_pressed` also reads the "pressed since last call" bit.
- A 30-page capture was 2560x27710; the editor uploaded it as one texture, beyond the GPU limit (egui asserts on max_texture_side). The canvas now uploads `DocTex` tiles (<= min(4096, max_texture_side)); zoom floor 2%. `capture.scroll.step` also ends at 100 Mpx of frames.
- MSI: Start menu shortcut now sits in the Start list root; new Desktop shortcut (DESKTOP=0 skips).

## 2026-11-03 - Editor zoom and taskbar icon (0.2.5)
- Opening a library image lands in the editor already fitted, with zoom out / percent / zoom in / fit in the top bar (same controls as the side panel). Zoom floor is 2% in both places; the old side-panel floor of 10% turned a zoom-out click into a zoom-in on a tall capture.
- screenit.exe had no icon resource, and the taskbar button is recreated when the window leaves the tray, so it showed a blank page. The mark is drawn in icon_glyph.rs, embedded as icon 1 by build.rs, set on the window, and the process AppUserModelID is screenIt.
- Project rule: a finished, tested feature is committed and pushed to origin.

## 2026-11-03 - Hide secrets did not redraw (0.2.6)
- The button reported "hid N" because editor.redact.apply succeeded, but the UI called session.dispatch directly, so doc_rev never moved and the canvas kept the old texture. It now goes through UiApp::run, which bumps the revision before the same frame redraws.

## 2026-11-03 - Library Maximize was dimmed (0.2.9)
- Windows 11 draws Maximize grey on a window that cannot be resized. The library was fixed like the capture bar, so the button was visible and dead. The library is resizable now, same as the editor. The capture bar stays fixed.

## 2026-11-03 - About, and Maximize only in the editor (0.2.8)
- About (capture bar and the editor top bar) shows the version, opens https://github.com/Busbar2297/screenit, and scrolls the bundled credits.
- The window is created without a Maximize button. The button is added only while the editor is showing, and the window is restored when you leave the editor. On Windows 11 the square stays drawn on a resizable window, so every other view is also not resizable.

## 2026-11-03 - Recording frame can be seen and moved (0.2.7)
- There was no border once recording started, so the area was easy to lose and could not be moved. Four solid red bars now sit just outside the area (inside the screen when a side is at the edge) and are excluded from the capture. Dragging a bar calls record.move, which shifts the grab origin without changing the frame size.

## Arabic annotation text
- vector: text is now shaped + bidi-ordered (screenit_i18n::visual), drawn with Tahoma/Arial for Arabic glyphs, wrapped by layout_text, RTL lines right-aligned in a box.
