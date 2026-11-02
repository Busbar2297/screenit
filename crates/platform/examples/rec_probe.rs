//! Record the primary monitor for a few seconds at a chosen output size and report what came out:
//! `cargo run --release -p screenit-platform --example rec_probe -- <native|1080p|1440p|2160p> <fps> <out.mp4>`.
use screenit_capture::ScreenSource;
use screenit_platform::{MfRecorder, Win32Screen};
use screenit_record::{Codec, OutputSize, RecordConfig, Recorder};
use std::sync::Arc;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    screenit_platform::init_dpi_awareness();
    let a: Vec<String> = std::env::args().collect();
    let size = a
        .get(1)
        .and_then(|s| OutputSize::parse(s))
        .unwrap_or_default();
    let fps = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let out = a.get(3).cloned().unwrap_or_else(|| "rec_probe.mp4".into());
    let screen = Arc::new(Win32Screen::new());
    let m = screen
        .monitors()?
        .into_iter()
        .find(|m| m.primary)
        .ok_or("no primary monitor")?;
    let mut rec = MfRecorder::new(screen);
    rec.start(&RecordConfig {
        region: m.rect,
        window: None,
        fps,
        codec: Codec::H264,
        size,
        mic: None,
        system_audio: false,
        cursor_highlight: false,
        click_ripples: false,
        output: out.into(),
    })?;
    std::thread::sleep(Duration::from_secs(4));
    let r = rec.stop()?;
    println!(
        "{}x{} {} frames in {} ms = {:.1} fps (target {fps}) -> {}",
        r.width,
        r.height,
        r.frames,
        r.duration_ms,
        r.frames as f64 * 1000.0 / r.duration_ms.max(1) as f64,
        r.path.display()
    );
    Ok(())
}
