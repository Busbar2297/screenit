//! Text recognition through Windows.Media.Ocr (offline, uses the user's installed languages).

use screenit_capture::{CaptureError, Ocr, OcrLine};
use screenit_geom::Rect;
use screenit_raster::Raster;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

pub struct WinOcr;

fn plat<E: std::fmt::Display>(e: E) -> CaptureError {
    CaptureError::Platform(format!("ocr: {e}"))
}

impl Ocr for WinOcr {
    fn recognize(&self, image: &Raster) -> Result<Vec<OcrLine>, CaptureError> {
        // The engine has a maximum image dimension; refuse rather than silently clip.
        let max = OcrEngine::MaxImageDimension().map_err(plat)?;
        if image.width() > max || image.height() > max {
            return Err(plat(format!("image larger than {max}px")));
        }
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|_| plat("no OCR language installed"))?;
        // Raster is RGBA; the bitmap wants BGRA.
        let mut bgra = Vec::with_capacity(image.data().len());
        for p in image.data().chunks_exact(4) {
            bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        let writer = DataWriter::new().map_err(plat)?;
        writer.WriteBytes(&bgra).map_err(plat)?;
        let buffer = writer.DetachBuffer().map_err(plat)?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            image.width() as i32,
            image.height() as i32,
        )
        .map_err(plat)?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(plat)?
            .join()
            .map_err(plat)?;
        let mut out = Vec::new();
        for line in result.Lines().map_err(plat)? {
            let text = line.Text().map_err(plat)?.to_string();
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, 0f64, 0f64);
            for w in line.Words().map_err(plat)? {
                let r = w.BoundingRect().map_err(plat)?;
                x0 = x0.min(f64::from(r.X));
                y0 = y0.min(f64::from(r.Y));
                x1 = x1.max(f64::from(r.X + r.Width));
                y1 = y1.max(f64::from(r.Y + r.Height));
            }
            let rect = if x0 <= x1 {
                Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
            } else {
                Rect::default()
            };
            out.push(OcrLine { text, rect });
        }
        Ok(out)
    }
}
