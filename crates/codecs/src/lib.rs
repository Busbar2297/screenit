//! Image encode/decode for raster surfaces (PNG, JPEG, GIF, BMP, TIFF, WebP).
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use image::{ImageEncoder, ImageFormat};
use screenit_raster::Raster;
use std::io::Cursor;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unsupported format `{0}`")]
    Unsupported(String),
    #[error("image error: {0}")]
    Image(String),
    #[error(transparent)]
    Raster(#[from] screenit_raster::RasterError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Tiff,
    WebP,
}

impl Format {
    pub fn from_extension(ext: &str) -> Result<Format, CodecError> {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "png" => Ok(Format::Png),
            "jpg" | "jpeg" => Ok(Format::Jpeg),
            "gif" => Ok(Format::Gif),
            "bmp" => Ok(Format::Bmp),
            "tif" | "tiff" => Ok(Format::Tiff),
            "webp" => Ok(Format::WebP),
            other => Err(CodecError::Unsupported(other.to_string())),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Tiff => "tiff",
            Format::WebP => "webp",
        }
    }
}

fn to_image(r: &Raster) -> Result<image::RgbaImage, CodecError> {
    image::RgbaImage::from_raw(r.width(), r.height(), r.data().to_vec())
        .ok_or_else(|| CodecError::Image("buffer size mismatch".into()))
}

/// Encode a raster. `quality` (1..=100) applies to JPEG only.
pub fn encode(r: &Raster, fmt: Format, quality: u8) -> Result<Vec<u8>, CodecError> {
    if r.is_empty() {
        return Err(CodecError::Image("empty image".into()));
    }
    let img = to_image(r)?;
    let mut out = Vec::new();
    let map = |e: image::ImageError| CodecError::Image(e.to_string());
    match fmt {
        Format::Png => {
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(
                    img.as_raw(),
                    r.width(),
                    r.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(map)?;
        }
        Format::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            let q = quality.clamp(1, 100);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
                .write_image(
                    rgb.as_raw(),
                    r.width(),
                    r.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(map)?;
        }
        Format::Bmp | Format::Gif | Format::Tiff | Format::WebP => {
            let f = match fmt {
                Format::Bmp => ImageFormat::Bmp,
                Format::Gif => ImageFormat::Gif,
                Format::Tiff => ImageFormat::Tiff,
                _ => ImageFormat::WebP,
            };
            let dynimg = image::DynamicImage::ImageRgba8(img);
            let dynimg = if fmt == Format::Bmp {
                image::DynamicImage::ImageRgb8(dynimg.to_rgb8())
            } else {
                dynimg
            };
            dynimg
                .write_to(&mut Cursor::new(&mut out), f)
                .map_err(map)?;
        }
    }
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<Raster, CodecError> {
    let img = image::load_from_memory(bytes).map_err(|e| CodecError::Image(e.to_string()))?;
    let rgba = img.to_rgba8();
    Ok(Raster::from_rgba(
        rgba.width(),
        rgba.height(),
        rgba.into_raw(),
    )?)
}

/// Encode pages as a PDF (one JPEG-compressed image per page; 96 dpi => 0.75 pt per pixel).
pub fn encode_pdf(pages: &[Raster], quality: u8) -> Result<Vec<u8>, CodecError> {
    if pages.is_empty() {
        return Err(CodecError::Image("a PDF needs at least one page".into()));
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let n = pages.len();
    // Object ids: 1 catalog, 2 pages, then per page i: 3+3i page, 4+3i image, 5+3i content.
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 3 + 3 * i)).collect();
    obj(
        &mut out,
        format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").as_bytes(),
    );
    for (i, p) in pages.iter().enumerate() {
        let (w, h) = (p.width(), p.height());
        let (pw, ph) = (w as f32 * 0.75, h as f32 * 0.75);
        let jpg = encode(p, Format::Jpeg, quality)?;
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw} {ph}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                4 + 3 * i,
                5 + 3 * i
            )
            .as_bytes(),
        );
        let mut img = format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            jpg.len()
        )
        .into_bytes();
        img.extend_from_slice(&jpg);
        img.extend_from_slice(b"\nendstream");
        obj(&mut out, &img);
        let content = format!("q {pw} 0 0 {ph} 0 0 cm /Im0 Do Q");
        obj(
            &mut out,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .as_bytes(),
        );
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    Ok(out)
}
/// Animated GIF from frames (all must share one size), `delay_ms` per frame, looping forever.
pub fn encode_gif_animation(frames: &[Raster], delay_ms: u32) -> Result<Vec<u8>, CodecError> {
    use image::codecs::gif::{GifEncoder, Repeat};
    if frames.is_empty() {
        return Err(CodecError::Image("a GIF needs at least one frame".into()));
    }
    let mut buf = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut buf, 20);
        enc.set_repeat(Repeat::Infinite)
            .map_err(|e| CodecError::Image(e.to_string()))?;
        for f in frames {
            let delay = image::Delay::from_numer_denom_ms(delay_ms.max(10), 1);
            let frame = image::Frame::from_parts(to_image(f)?, 0, 0, delay);
            enc.encode_frame(frame)
                .map_err(|e| CodecError::Image(e.to_string()))?;
        }
    }
    Ok(buf)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Raster {
        let mut r = Raster::new(8, 8, [10, 20, 30, 255]).unwrap();
        r.put(3, 3, [200, 100, 50, 255]);
        r
    }

    #[test]
    fn png_roundtrip_is_lossless() {
        let r = sample();
        let back = decode(&encode(&r, Format::Png, 90).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn pdf_has_structure() {
        let r = Raster::new(8, 4, [10, 20, 30, 255]).unwrap();
        let b = encode_pdf(&[r.clone(), r], 80).unwrap();
        let s = String::from_utf8_lossy(&b);
        assert!(s.starts_with("%PDF-1.4"));
        assert!(s.contains("/Count 2"));
        assert!(s.trim_end().ends_with("%%EOF"));
        let xref: usize = s
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(b[xref..].starts_with(b"xref"));
        assert!(encode_pdf(&[], 80).is_err());
    }

    #[test]
    fn all_formats_encode_and_decode() {
        let r = sample();
        for f in [
            Format::Jpeg,
            Format::Gif,
            Format::Bmp,
            Format::Tiff,
            Format::WebP,
        ] {
            let bytes = encode(&r, f, 90).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let back = decode(&bytes).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_eq!((back.width(), back.height()), (8, 8), "{f:?}");
        }
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode(&[1, 2, 3, 4]).is_err());
        assert!(Format::from_extension("xyz").is_err());
        assert!(encode(&Raster::default(), Format::Png, 90).is_err());
    }
}
