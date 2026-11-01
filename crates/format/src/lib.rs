//! Native `.scit` project format (legacy `.sxs` files still open): a zip with `manifest.json` (objects + metadata) and `base.png`.
//! Saves are atomic (temp file + rename) so a crash never corrupts an existing project.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use screenit_codecs::{self as codecs, Format};
use screenit_doc::CaptureDoc;
use std::io::{Cursor, Read, Write};
use std::path::Path;

pub const FORMAT_VERSION: u32 = 1;
/// Extension for new project files.
pub const EXT: &str = "scit";
const LEGACY_EXT: &str = "sxs";

/// True for project files: `.scit`, or `.sxs` written by earlier versions.
pub fn is_project(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXT) || e.eq_ignore_ascii_case(LEGACY_EXT))
}
const MAX_ENTRY: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("io: {0}")]
    Io(String),
    #[error("zip: {0}")]
    Zip(String),
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("unsupported format version {0}")]
    Version(u32),
    #[error(transparent)]
    Codec(#[from] codecs::CodecError),
}

impl From<std::io::Error> for FormatError {
    fn from(e: std::io::Error) -> Self {
        FormatError::Io(e.to_string())
    }
}
impl From<zip::result::ZipError> for FormatError {
    fn from(e: zip::result::ZipError) -> Self {
        FormatError::Zip(e.to_string())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Manifest {
    format_version: u32,
    doc: serde_json::Value,
}

pub fn to_bytes(doc: &CaptureDoc) -> Result<Vec<u8>, FormatError> {
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        doc: serde_json::to_value(doc).map_err(|e| FormatError::Manifest(e.to_string()))?,
    };
    let json =
        serde_json::to_vec_pretty(&manifest).map_err(|e| FormatError::Manifest(e.to_string()))?;
    let png = codecs::encode(doc.base(), Format::Png, 100)?;
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        z.start_file("manifest.json", opts)?;
        z.write_all(&json)?;
        z.start_file("base.png", opts)?;
        z.write_all(&png)?;
        z.finish()?;
    }
    Ok(buf.into_inner())
}

fn read_entry<R: Read + std::io::Seek>(
    z: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, FormatError> {
    let f = z.by_name(name)?;
    if f.size() > MAX_ENTRY {
        return Err(FormatError::Zip(format!("entry {name} too large")));
    }
    let mut out = Vec::new();
    f.take(MAX_ENTRY).read_to_end(&mut out)?;
    Ok(out)
}

pub fn from_bytes(bytes: &[u8]) -> Result<CaptureDoc, FormatError> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes))?;
    let manifest: Manifest = serde_json::from_slice(&read_entry(&mut z, "manifest.json")?)
        .map_err(|e| FormatError::Manifest(e.to_string()))?;
    if manifest.format_version > FORMAT_VERSION {
        return Err(FormatError::Version(manifest.format_version));
    }
    let doc: CaptureDoc =
        serde_json::from_value(manifest.doc).map_err(|e| FormatError::Manifest(e.to_string()))?;
    let base = codecs::decode(&read_entry(&mut z, "base.png")?)?;
    Ok(doc.with_base(base))
}

pub fn save(doc: &CaptureDoc, path: &Path) -> Result<(), FormatError> {
    let bytes = to_bytes(doc)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

pub fn load(path: &Path) -> Result<CaptureDoc, FormatError> {
    from_bytes(&std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_doc::{ObjectKind, Style};
    use screenit_geom::{PointF, RectF};
    use screenit_raster::Raster;

    fn sample() -> CaptureDoc {
        let mut base = Raster::new(30, 20, [1, 2, 3, 255]).unwrap();
        base.put(4, 4, [200, 0, 0, 255]);
        let mut d = CaptureDoc::new(base);
        d.name = "shot".into();
        d.metadata.title = "Window".into();
        d.add(
            ObjectKind::Rect {
                rect: RectF::new(1.0, 1.0, 5.0, 5.0),
                radius: 0.0,
            },
            Style::default(),
        )
        .unwrap();
        d.add(
            ObjectKind::Text {
                pos: PointF::new(2.0, 2.0),
                text: "héllo".into(),
                size: 12.0,
            },
            Style::default(),
        )
        .unwrap();
        d
    }

    #[test]
    fn bytes_roundtrip() {
        let d = sample();
        let back = from_bytes(&to_bytes(&d).unwrap()).unwrap();
        assert_eq!(back.base(), d.base());
        assert_eq!(back.objects, d.objects);
        assert_eq!(back.metadata, d.metadata);
        let mut back = back;
        let id = back
            .add(
                ObjectKind::Pen {
                    points: vec![PointF::new(1.0, 1.0)],
                },
                Style::default(),
            )
            .unwrap();
        assert_eq!(id, 3, "ids continue after reload");
    }

    #[test]
    fn file_save_is_atomic_and_loads() {
        let dir = std::env::temp_dir().join(format!("scit-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.scit");
        save(&sample(), &p).unwrap();
        assert!(!dir.join("a.scit.tmp").exists());
        assert_eq!(load(&p).unwrap().objects.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn project_extensions() {
        assert!(is_project(Path::new("a.scit")));
        assert!(is_project(Path::new("C:\\x\\old.SXS")));
        assert!(!is_project(Path::new("a.png")));
        assert!(!is_project(Path::new("scit")));
    }

    #[test]
    fn malformed_input_errors() {
        assert!(from_bytes(b"not a zip").is_err());
        assert!(from_bytes(&[]).is_err());
        assert!(load(Path::new("Z:\\definitely\\missing.scit")).is_err());
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file("manifest.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(br#"{"format_version":99,"doc":{}}"#).unwrap();
            z.finish().unwrap();
        }
        assert!(matches!(
            from_bytes(&buf.into_inner()),
            Err(FormatError::Version(99))
        ));
    }
}
