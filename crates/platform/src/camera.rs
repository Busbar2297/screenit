#![allow(clippy::collapsible_if)]
//! Webcam via the Media Foundation source reader. The device is opened lazily on the first
//! `frame()` call (so the camera light only comes on while it is used) and released by `stop()`.
//! A background thread keeps the most recent frame as RGBA.

use screenit_raster::Raster;
use screenit_record::{CameraSource, RecordError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaSource, IMFSourceReader,
    MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFMediaType_Video,
    MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::PWSTR;

fn err<E: std::fmt::Display>(e: E) -> RecordError {
    RecordError::Platform(e.to_string())
}

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
/// Widest native mode we ask the camera for; PiP never needs more and it keeps conversion cheap.
const MAX_WIDTH: u32 = 1280;

/// Video capture devices as (friendly name, activator).
fn devices() -> Result<Vec<(String, IMFActivate)>, RecordError> {
    crate::mf::init_mf()?;
    // SAFETY: MFEnumDeviceSources returns a CoTaskMem array of `count` activators; each element
    // is moved out exactly once and the array itself is freed afterwards.
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1).map_err(err)?;
        let attrs = attrs.ok_or_else(|| err("no attributes"))?;
        attrs
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(err)?;
        let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut arr, &mut count).map_err(err)?;
        let mut out = Vec::new();
        if !arr.is_null() {
            for i in 0..count as usize {
                if let Some(act) = std::ptr::read(arr.add(i)) {
                    let mut name = PWSTR::null();
                    let mut len = 0u32;
                    let label = if act
                        .GetAllocatedString(
                            &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                            &mut name,
                            &mut len,
                        )
                        .is_ok()
                    {
                        let s = name.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(name.0 as *const _));
                        s
                    } else {
                        format!("Camera {}", i + 1)
                    };
                    out.push((label, act));
                }
            }
            CoTaskMemFree(Some(arr as *const _));
        }
        Ok(out)
    }
}

/// Friendly names of the attached cameras.
pub fn cameras() -> Vec<String> {
    devices()
        .map(|v| v.into_iter().map(|(n, _)| n).collect())
        .unwrap_or_default()
}

fn open_reader(want: Option<&str>) -> Result<(IMFSourceReader, IMFMediaSource), RecordError> {
    let devs = devices()?;
    let pick = want
        .map(str::to_lowercase)
        .and_then(|w| devs.iter().position(|(n, _)| n.to_lowercase().contains(&w)))
        .unwrap_or(0);
    let (_, act) = devs
        .get(pick)
        .ok_or_else(|| err("no camera is connected"))?;
    // SAFETY: COM calls on live objects; media types are reference-counted by the wrappers.
    unsafe {
        let source: IMFMediaSource = act.ActivateObject().map_err(err)?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1).map_err(err)?;
        let attrs = attrs.ok_or_else(|| err("no attributes"))?;
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .map_err(err)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs).map_err(err)?;
        // Largest native mode no wider than MAX_WIDTH (the first such entry wins ties).
        let mut best: Option<(u32, u32)> = None;
        for i in 0..256 {
            let Ok(t) = reader.GetNativeMediaType(STREAM, i) else {
                break;
            };
            let Ok(packed) = t.GetUINT64(&MF_MT_FRAME_SIZE) else {
                continue;
            };
            let w = (packed >> 32) as u32;
            if w <= MAX_WIDTH && best.is_none_or(|(_, bw)| w > bw) {
                best = Some((i, w));
            }
        }
        if let Some((i, _)) = best {
            if let Ok(t) = reader.GetNativeMediaType(STREAM, i) {
                let _ = reader.SetCurrentMediaType(STREAM, None, &t);
            }
        }
        let want = MFCreateMediaType().map_err(err)?;
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(err)?;
        want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(err)?;
        reader
            .SetCurrentMediaType(STREAM, None, &want)
            .map_err(err)?;
        Ok((reader, source))
    }
}

fn read_frame(reader: &IMFSourceReader) -> Result<Option<Raster>, RecordError> {
    // SAFETY: synchronous ReadSample; the buffer is locked only while it is copied, and every
    // row slice is bounds-checked against the locked length.
    unsafe {
        let mt = reader.GetCurrentMediaType(STREAM).map_err(err)?;
        let packed = mt.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
        let (w, h) = ((packed >> 32) as u32, packed as u32);
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return Err(err("unsupported camera frame size"));
        }
        // RGB32 from the video processor is top-down unless a negative stride says otherwise.
        let stride = mt
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|s| s as i32)
            .unwrap_or((w * 4) as i32);
        let (mut flags, mut ts) = (0u32, 0i64);
        let mut sample = None;
        reader
            .ReadSample(
                STREAM,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )
            .map_err(err)?;
        let Some(sample) = sample else {
            return Ok(None);
        };
        let buf = sample.ConvertToContiguousBuffer().map_err(err)?;
        let (mut ptr, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
        buf.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
        if ptr.is_null() {
            let _ = buf.Unlock();
            return Err(err("camera buffer lock returned null"));
        }
        let data = std::slice::from_raw_parts(ptr, len as usize);
        let row = (stride.unsigned_abs() as usize).max(w as usize * 4);
        let bottom_up = stride < 0;
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        let mut short = false;
        for y in 0..h as usize {
            let sy = if bottom_up { h as usize - 1 - y } else { y };
            let Some(line) = data.get(sy * row..sy * row + w as usize * 4) else {
                short = true;
                break;
            };
            for px in line.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        let _ = buf.Unlock();
        if short {
            return Err(err("short camera frame"));
        }
        Raster::from_rgba(w, h, rgba).map(Some).map_err(err)
    }
}

struct Running {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

/// The system webcam (first device, or the one whose name contains `want`).
pub struct MfCamera {
    want: Option<String>,
    latest: Arc<Mutex<Option<Raster>>>,
    running: Mutex<Option<Running>>,
    /// Back off after a failed open so a missing camera is not re-probed every frame.
    failed_at: Mutex<Option<Instant>>,
    name: Arc<Mutex<String>>,
}

impl MfCamera {
    pub fn new(want: Option<String>) -> Self {
        Self {
            want,
            latest: Arc::new(Mutex::new(None)),
            running: Mutex::new(None),
            failed_at: Mutex::new(None),
            name: Arc::new(Mutex::new(String::new())),
        }
    }

    fn ensure_running(&self) {
        let Ok(mut running) = self.running.lock() else {
            return;
        };
        if running.as_ref().is_some_and(|r| !r.worker.is_finished()) {
            return;
        }
        if let Ok(f) = self.failed_at.lock() {
            if f.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
                return;
            }
        }
        if let Ok(mut f) = self.failed_at.lock() {
            *f = None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (st, latest, want) = (stop.clone(), self.latest.clone(), self.want.clone());
        let worker = std::thread::spawn(move || {
            let opened = crate::mf::init_mf().and_then(|()| open_reader(want.as_deref()));
            let Ok((reader, source)) = opened else {
                return;
            };
            let mut errors = 0;
            while !st.load(Ordering::SeqCst) && errors < 30 {
                match read_frame(&reader) {
                    Ok(Some(f)) => {
                        errors = 0;
                        if let Ok(mut l) = latest.lock() {
                            *l = Some(f);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        errors += 1;
                        std::thread::sleep(Duration::from_millis(30));
                    }
                }
            }
            // SAFETY: shutting down the media source releases the device (camera light off).
            unsafe {
                let _ = source.Shutdown();
            }
        });
        *running = Some(Running { stop, worker });
    }
}

impl CameraSource for MfCamera {
    fn name(&self) -> String {
        if let Ok(mut n) = self.name.lock() {
            if n.is_empty() {
                let all = cameras();
                let want = self.want.as_deref().map(str::to_lowercase);
                *n = want
                    .and_then(|w| all.iter().find(|c| c.to_lowercase().contains(&w)).cloned())
                    .or_else(|| all.first().cloned())
                    .unwrap_or_else(|| "No camera".into());
            }
            return n.clone();
        }
        String::new()
    }

    fn frame(&self) -> Option<Raster> {
        self.ensure_running();
        let running = self
            .running
            .lock()
            .ok()
            .is_some_and(|r| r.as_ref().is_some_and(|r| !r.worker.is_finished()));
        if !running {
            if let Ok(mut f) = self.failed_at.lock() {
                f.get_or_insert_with(Instant::now);
            }
        }
        self.latest.lock().ok()?.clone()
    }

    fn stop(&self) {
        let r = self.running.lock().ok().and_then(|mut r| r.take());
        if let Some(r) = r {
            r.stop.store(true, Ordering::SeqCst);
            let _ = r.worker.join();
        }
        if let Ok(mut l) = self.latest.lock() {
            *l = None;
        }
        if let Ok(mut f) = self.failed_at.lock() {
            *f = None;
        }
    }
}

impl Drop for MfCamera {
    fn drop(&mut self) {
        CameraSource::stop(self);
    }
}
