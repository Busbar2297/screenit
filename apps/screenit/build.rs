//! Embed the screenIt icon in the executable so Windows shows it on the taskbar, in Alt-Tab
//! and on shortcuts. The picture is drawn by the same code the window uses.
use std::path::PathBuf;
use std::process::Command;

#[path = "../../crates/platform/src/icon_glyph.rs"]
mod icon_glyph;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../crates/platform/src/icon_glyph.rs");
    let Ok(out) = std::env::var("OUT_DIR") else {
        return;
    };
    let out = PathBuf::from(out);
    let ico = out.join("screenit.ico");
    let rc = out.join("screenit.rc");
    let res = out.join("screenit.res");
    if std::fs::write(&ico, ico_file(&[16, 32, 48, 256])).is_err() {
        return;
    }
    let Some(rc_exe) = rc_exe() else {
        println!("cargo:warning=rc.exe not found; screenit.exe will have no icon");
        return;
    };
    let rc_text = format!(
        "1 ICON \"{}\"\n",
        ico.display().to_string().replace('\\', "\\\\")
    );
    if std::fs::write(&rc, rc_text).is_err() {
        return;
    }
    let ok = Command::new(rc_exe)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&rc)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg-bin=screenit={}", res.display());
    }
}

/// Newest Windows SDK resource compiler, x64.
fn rc_exe() -> Option<PathBuf> {
    let root = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<_> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    versions.sort();
    versions.into_iter().rev().find_map(|v| {
        let p = v.join("x64").join("rc.exe");
        p.is_file().then_some(p)
    })
}

/// A Windows .ico holding one 32-bit BMP image per size.
fn ico_file(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().copied().map(dib).collect();
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + images.len() * 16;
    for (n, img) in sizes.iter().zip(&images) {
        let dim = if *n >= 256 { 0 } else { *n as u8 };
        buf.push(dim);
        buf.push(dim);
        buf.push(0);
        buf.push(0);
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&32u16.to_le_bytes());
        buf.extend_from_slice(&(img.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in &images {
        buf.extend_from_slice(img);
    }
    buf
}

/// BITMAPINFOHEADER + bottom-up BGRA pixels + an empty 1-bit mask.
fn dib(n: u32) -> Vec<u8> {
    let px = icon_glyph::rgba(n);
    let stride = n.div_ceil(32) * 4;
    let mask = (stride * n) as usize;
    let xor = (n * n * 4) as usize;
    let mut buf = Vec::with_capacity(40 + xor + mask);
    // biPlanes = 1, biBitCount = 32, packed into the header's one 32-bit slot.
    for v in [40u32, n, n * 2, 1 | (32 << 16), 0, xor as u32, 0, 0, 0, 0] {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    for y in (0..n).rev() {
        for x in 0..n {
            let i = ((y * n + x) * 4) as usize;
            buf.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    buf.resize(buf.len() + mask, 0);
    buf
}
