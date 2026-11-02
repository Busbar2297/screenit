//! Compare GDI and DXGI desktop-duplication grab speed: `cargo run --release -p screenit-platform --example dup_probe`.
use screenit_capture::ScreenSource;
use screenit_platform::Win32Screen;
use std::time::Instant;

fn main() {
    screenit_platform::init_dpi_awareness();
    let screen = Win32Screen::new();
    let Ok(monitors) = screen.monitors() else {
        eprintln!("no monitors");
        return;
    };
    for m in monitors {
        println!(
            "monitor {} {}x{} at {},{}",
            m.name, m.rect.w, m.rect.h, m.rect.x, m.rect.y
        );
        for (label, stream) in [("gdi", false), ("dxgi", true)] {
            let n = 30;
            let t = Instant::now();
            let mut ok = 0;
            for _ in 0..n {
                let r = if stream {
                    screen.grab_stream(m.rect)
                } else {
                    screen.grab(m.rect)
                };
                ok += usize::from(r.is_ok());
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            println!("  {label}: {ms:.1} ms/frame ({ok}/{n} ok)");
        }
    }
}
