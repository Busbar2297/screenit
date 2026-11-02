# Attribution

screenIt ships no third-party images, icons or sounds: every icon is drawn in code
(`crates/ui-egui/src/icons.rs`) and the tray icon is generated at runtime.

## Bundled fonts

The UI uses the fonts that come with [egui](https://github.com/emilk/egui) (`default_fonts`):

| Font | License |
|---|---|
| Ubuntu Light | Ubuntu Font Licence 1.0 |
| Hack | MIT / Bitstream Vera |
| Noto Emoji | SIL Open Font License 1.1 |
| emoji-icon-font | SIL Open Font License 1.1 |

Annotation text is rendered with fonts already installed on the user's system (Segoe UI, Arial,
Calibri or Tahoma); none are redistributed.

## Libraries

screenIt is built on the Rust ecosystem, notably `eframe`/`egui`, `ureq` (HTTP to a local LM Studio server), `windows` / `windows-sys`
(Microsoft), `tray-icon`, `image`, `ab_glyph`, `zip`, `serde`, `clap` and `chrono`, all under
permissive licenses (MIT, Apache-2.0, or similar). The full dependency list is in `Cargo.lock`.

## Platform

Capture, recording, OCR and encoding use components that ship with Windows (GDI, DWM, Media
Foundation, WASAPI, Windows.Media.Ocr). No ffmpeg or other external binaries are used.

## Trademarks

Snagit is a trademark of TechSmith Corporation. screenIt is an independent project, not affiliated
with or endorsed by TechSmith. Its behaviour is modelled on publicly documented features only; no
proprietary code, assets or stamps are used.
