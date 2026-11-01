//! Manual probe for recording devices: lists microphones and cameras, saves one webcam frame and
//! records a short clip with audio and the webcam composited.
//! `cargo run -p screenit-platform --example av_probe -- <out-dir> [secs] [nomic] [system] [nocam]`

use screenit_geom::Rect;
use screenit_record::{CameraSource, Codec, PipLayout, RecordConfig, Recorder, VideoReader};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "target".into()));
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4);
    let has = |f: &str| args.iter().any(|a| a == f);
    std::fs::create_dir_all(&dir)?;
    screenit_platform::init_dpi_awareness();

    let svc = screenit_platform::services();
    if let Some(d) = &svc.devices {
        println!("microphones: {:?}", d.microphones());
        println!("cameras: {:?}", d.cameras());
    }
    let cam: Arc<dyn CameraSource> = Arc::new(screenit_platform::camera::MfCamera::new(None));
    if !has("nocam") {
        println!("camera name: {}", cam.name());
        let t = std::time::Instant::now();
        let mut frame = None;
        while frame.is_none() && t.elapsed().as_secs() < 8 {
            frame = cam.frame();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Some(f) = cam.frame() {
                let d = f.data();
                let mean = d.iter().map(|&v| u64::from(v)).sum::<u64>() / d.len().max(1) as u64;
                let max = d.chunks_exact(4).map(|p| p[0].max(p[1]).max(p[2])).max();
                println!("camera mean {mean} max {max:?}");
                frame = Some(f);
            }
        }
        match frame {
            Some(f) => {
                println!(
                    "camera frame {}x{} after {:?}",
                    f.width(),
                    f.height(),
                    t.elapsed()
                );
                let png = screenit_codecs::encode(&f, screenit_codecs::Format::Png, 90)?;
                std::fs::write(dir.join("camera.png"), png)?;
            }
            None => println!("no camera frame"),
        }
    }

    let screen = svc.screen.clone().ok_or("no screen")?;
    let b = screen
        .monitors()?
        .first()
        .map(|m| m.rect)
        .ok_or("no monitor")?;
    let out = dir.join("av_probe.mp4");
    let cfg = RecordConfig {
        region: Rect::new(b.x, b.y, b.w.min(1280), b.h.min(720)),
        window: None,
        fps: 30,
        codec: Codec::H264,
        mic: (!has("nomic")).then(|| "default".to_string()),
        system_audio: has("system"),
        cursor_highlight: true,
        click_ripples: false,
        output: out.clone(),
    };
    let mut rec = screenit_platform::MfRecorder::new(screen);
    if !has("nocam") {
        rec.attach_webcam(cam.clone(), PipLayout::default())?;
    }
    rec.start(&cfg)?;
    println!("recording {secs}s (speak now)...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    let res = rec.stop()?;
    cam.stop();
    println!("result: {res:?}");
    let info = screenit_platform::MfVideoReader::new().probe(&out)?;
    println!("probe: {info:?}");
    let mid = screenit_platform::MfVideoReader::new().frame_at(&out, info.duration_ms / 2)?;
    let png = screenit_codecs::encode(&mid, screenit_codecs::Format::Png, 90)?;
    std::fs::write(dir.join("av_probe_mid.png"), png)?;
    Ok(())
}
