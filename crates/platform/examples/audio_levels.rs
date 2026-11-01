//! Manual probe: decodes the audio track of an MP4 to PCM and prints the RMS level per 500 ms.
//! `cargo run -p screenit-platform --example audio_levels -- file.mp4`
#![allow(unsafe_code)]

use windows::Win32::Media::MediaFoundation::{
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_VERSION, MFAudioFormat_PCM, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Audio, MFSTARTUP_FULL, MFStartup,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

fn main() -> windows::core::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/av/av_probe.mp4".into());
    let path = std::fs::canonicalize(&path)
        .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
    // SAFETY: probe-only MF calls; buffers are read within their locked length.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let plain = path
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_string();
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(plain), None)?;
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        let want = MFCreateMediaType()?;
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        want.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        want.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        reader.SetCurrentMediaType(stream, None, &want)?;
        let mut samples: Vec<i16> = Vec::new();
        loop {
            let (mut flags, mut ts) = (0u32, 0i64);
            let mut sample = None;
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?;
            if flags & 2 != 0 {
                break;
            }
            let Some(s) = sample else { continue };
            let buf = s.ConvertToContiguousBuffer()?;
            let (mut p, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
            buf.Lock(&mut p, None, Some(&mut len))?;
            let d = std::slice::from_raw_parts(p, len as usize);
            samples.extend(d.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
            buf.Unlock()?;
        }
        let per = 48_000; // 500 ms of stereo samples
        for (i, c) in samples.chunks(per).enumerate() {
            let rms =
                (c.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / c.len() as f64).sqrt();
            println!("{:>5.1}s rms {:>7.1}", i as f64 * 0.5, rms);
        }
        println!("total {:.2}s", samples.len() as f64 / 96_000.0);
    }
    Ok(())
}
