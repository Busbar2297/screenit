//! Decode one frame of a video through the platform reader and save it as PNG.
//! `cargo run -p screenit-platform --example frame_dump -- <video.mp4> <out.png> [ms]`

use screenit_record::VideoReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(src), Some(out)) = (args.get(1), args.get(2)) else {
        return Err("usage: frame_dump <video.mp4> <out.png> [ms]".into());
    };
    let ms: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(500);
    let r = screenit_platform::MfVideoReader::new();
    let info = r.probe(std::path::Path::new(src))?;
    println!("{info:?}");
    let f = r.frame_at(std::path::Path::new(src), ms)?;
    println!("frame {}x{}", f.width(), f.height());
    std::fs::write(
        out,
        screenit_codecs::encode(&f, screenit_codecs::Format::Png, 100)?,
    )?;
    Ok(())
}
