//! Manual probe: reads a few native (unconverted) webcam frames and prints their byte statistics,
//! to tell a black sensor (privacy shutter / camera kill switch) from a conversion problem.
#![allow(unsafe_code)]

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaSource, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, MF_MT_SUBTYPE,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION, MFCreateAttributes,
    MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFSTARTUP_FULL, MFStartup,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

fn main() -> windows::core::Result<()> {
    // SAFETY: probe-only COM/MF calls; the activator array is read within its reported length.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let Some(attrs) = attrs else { return Ok(()) };
        attrs.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        MFEnumDeviceSources(&attrs, &mut arr, &mut n)?;
        if n == 0 || arr.is_null() {
            println!("no camera");
            return Ok(());
        }
        let Some(act) = std::ptr::read(arr) else {
            return Ok(());
        };
        let src: IMFMediaSource = act.ActivateObject()?;
        let reader = MFCreateSourceReaderFromMediaSource(&src, None)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let mt = reader.GetCurrentMediaType(stream)?;
        let sub = mt.GetGUID(&MF_MT_SUBTYPE)?;
        println!("native subtype {sub:?}");
        for i in 0..40 {
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
            let Some(s) = sample else { continue };
            if i % 10 != 9 {
                continue;
            }
            let buf = s.ConvertToContiguousBuffer()?;
            let (mut p, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
            buf.Lock(&mut p, None, Some(&mut len))?;
            let d = std::slice::from_raw_parts(p, len as usize);
            let mean = d.iter().map(|&v| u64::from(v)).sum::<u64>() / d.len().max(1) as u64;
            let distinct = {
                let mut seen = [false; 256];
                d.iter().for_each(|&v| seen[v as usize] = true);
                seen.iter().filter(|&&b| b).count()
            };
            println!("frame {i}: {len} bytes, mean {mean}, distinct byte values {distinct}");
            buf.Unlock()?;
        }
        let _ = src.Shutdown();
    }
    Ok(())
}
