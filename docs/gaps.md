# Gaps (ranked work list)

Source of truth for status is `parity/snagit.toml`; `docs/parity-checklist.md` is generated from it.

1. Tier 0 capture: region/window/fullscreen via WGC/GDI, crosshair overlay, presets, hotkeys.
2. Editor foundation: doc objects, history, compositor oracle, core annotation tools.
3. Advanced capture: scrolling, menu, object, multi-area, freehand, OCR grab text.
4. Recording: Media Foundation pipeline, audio, cursor effects, screen draw.
5. Webcam/PiP, effects, video editing, library, sharing, MCP.
6. ML features (Simplify, Smart Move, background removal).

## Known gaps (post-M8)
- MSI is unsigned (SmartScreen warns); no code-signing certificate yet.
- Recording: no global record hotkeys (start/pause/stop), no audio meters or noise removal, no mic/camera picker in the UI (defaults are used; `record.devices` lists them), no audio playback in the in-app video player, capture runs about 21-25 fps at 1280x720 (GDI polling).
- No GPU compositor, mixed-DPI overlay, file-open dialog, print, templates, capture-info stamp.
- ML features are classical approximations: Simplify, Smart Move and printer capture are not implemented; background removal is corner flood-fill, not a segmentation model.
