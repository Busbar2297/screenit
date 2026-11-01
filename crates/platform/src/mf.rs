#![allow(clippy::collapsible_if)]
//! Media Foundation: H.264/H.265 MP4 screen recorder (`Recorder`) and a frame reader
//! (`VideoReader`). Capture is GDI-polled at the target frame rate; no external binaries.

use crate::audio::{AudioCapture, AudioKind};
use screenit_capture::ScreenSource;
use screenit_geom::{Point, Rect};
use screenit_raster::Raster;
use screenit_record::audio::{AudioChunk, AudioMixer, CHANNELS, RATE};
use screenit_record::{
    CameraSource, Codec, PipLayout, RecordConfig, RecordError, Recorder, RecordingResult,
    VideoInfo, VideoReader, VideoWriter, composite_webcam, draw_cursor_effects,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer2, IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter, IMFSourceReader,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION, MF2DBuffer_LockFlags_Read, MFAudioFormat_AAC,
    MFAudioFormat_PCM, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video,
    MFSTARTUP_FULL, MFStartup, MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::{GUID, Interface, PCWSTR};

fn err<E: std::fmt::Display>(e: E) -> RecordError {
    RecordError::Platform(e.to_string())
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(crate) fn init_mf() -> Result<(), RecordError> {
    // SAFETY: COM/MF initialisation; repeated calls are reference-counted and harmless.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(err)
    }
}

fn set_size(t: &IMFMediaType, key: &GUID, a: u32, b: u32) -> windows::core::Result<()> {
    // SAFETY: MF packs two u32 into one u64 attribute (MFSetAttributeSize/Ratio).
    unsafe { t.SetUINT64(key, (u64::from(a) << 32) | u64::from(b)) }
}

type Webcam = Option<(Arc<dyn CameraSource>, Arc<Mutex<PipLayout>>)>;

struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
}

pub struct MfRecorder {
    screen: Arc<dyn ScreenSource>,
    shared: Arc<Shared>,
    webcam: Webcam,
    worker: Option<JoinHandle<Result<RecordingResult, RecordError>>>,
}

impl MfRecorder {
    pub fn new(screen: Arc<dyn ScreenSource>) -> Self {
        Self {
            screen,
            shared: Arc::new(Shared {
                stop: AtomicBool::new(false),
                paused: AtomicBool::new(false),
            }),
            webcam: None,
            worker: None,
        }
    }
}

/// Adds the AAC output stream (fed with 48 kHz stereo 16-bit PCM) and returns its index.
fn add_audio_stream(writer: &IMFSinkWriter) -> Result<u32, RecordError> {
    let block = u32::from(CHANNELS) * 2;
    // SAFETY: media type construction on fresh COM objects.
    unsafe {
        let out = MFCreateMediaType().map_err(err)?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(err)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
            .map_err(err)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(err)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)
            .map_err(err)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(CHANNELS))
            .map_err(err)?;
        // 24000 bytes/s = 192 kbit/s, one of the rates the AAC encoder accepts.
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)
            .map_err(err)?;
        let stream = writer.AddStream(&out).map_err(err)?;
        let input = MFCreateMediaType().map_err(err)?;
        input
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(err)?;
        input
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(err)?;
        input
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(err)?;
        input
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)
            .map_err(err)?;
        input
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(CHANNELS))
            .map_err(err)?;
        input
            .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)
            .map_err(err)?;
        input
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * block)
            .map_err(err)?;
        writer
            .SetInputMediaType(stream, &input, None)
            .map_err(err)?;
        Ok(stream)
    }
}

fn make_audio_sample(c: &AudioChunk) -> Result<IMFSample, RecordError> {
    let len = c.pcm.len() * 2;
    // SAFETY: the buffer is locked for the copy and sized exactly `len` bytes.
    unsafe {
        let buf = MFCreateMemoryBuffer(len as u32).map_err(err)?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        buf.Lock(&mut ptr, None, None).map_err(err)?;
        if ptr.is_null() {
            return Err(err("buffer lock returned null"));
        }
        let dst = std::slice::from_raw_parts_mut(ptr, len);
        for (d, s) in dst.chunks_exact_mut(2).zip(&c.pcm) {
            d.copy_from_slice(&s.to_le_bytes());
        }
        buf.Unlock().map_err(err)?;
        buf.SetCurrentLength(len as u32).map_err(err)?;
        let sample = MFCreateSample().map_err(err)?;
        sample.AddBuffer(&buf).map_err(err)?;
        sample.SetSampleTime(c.ts_100ns).map_err(err)?;
        sample.SetSampleDuration(c.dur_100ns).map_err(err)?;
        Ok(sample)
    }
}

/// Live audio sources for one recording, mixed onto the video clock.
struct Audio {
    sources: Vec<AudioCapture>,
    mixer: AudioMixer,
    stream: u32,
}

impl Audio {
    fn open(cfg: &RecordConfig) -> Result<Option<Vec<AudioCapture>>, RecordError> {
        let mut sources = Vec::new();
        if let Some(name) = &cfg.mic {
            sources.push(
                AudioCapture::open(AudioKind::Mic(Some(name.clone())))
                    .map_err(|e| err(format!("microphone: {e}")))?,
            );
        }
        if cfg.system_audio {
            sources.push(
                AudioCapture::open(AudioKind::System)
                    .map_err(|e| err(format!("system audio: {e}")))?,
            );
        }
        Ok((!sources.is_empty()).then_some(sources))
    }

    fn collect(&mut self) {
        for (i, s) in self.sources.iter().enumerate() {
            while let Ok(p) = s.rx.try_recv() {
                self.mixer.push(i, &p);
            }
        }
    }

    fn write(&self, writer: &IMFSinkWriter, chunks: Vec<AudioChunk>) -> Result<(), RecordError> {
        for c in chunks {
            let sample = make_audio_sample(&c)?;
            // SAFETY: the audio stream was added before BeginWriting.
            unsafe { writer.WriteSample(self.stream, &sample).map_err(err)? };
        }
        Ok(())
    }

    fn pump(&mut self, writer: &IMFSinkWriter, media_ms: u64) -> Result<(), RecordError> {
        self.collect();
        let mut chunks = Vec::new();
        while let Some(c) = self.mixer.take_due(media_ms) {
            chunks.push(c);
        }
        self.write(writer, chunks)
    }
}

fn make_writer(
    cfg: &RecordConfig,
    w: u32,
    h: u32,
    audio: bool,
) -> Result<(IMFSinkWriter, Option<u32>), RecordError> {
    // SAFETY: straightforward MF calls; every COM object is reference-counted by the wrappers.
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2).map_err(err)?;
        let attrs = attrs.ok_or_else(|| err("no attributes"))?;
        attrs
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 0)
            .map_err(err)?;
        let path = wide(&cfg.output);
        let writer = MFCreateSinkWriterFromURL(PCWSTR(path.as_ptr()), None, &attrs).map_err(err)?;

        let out = MFCreateMediaType().map_err(err)?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(err)?;
        let codec = match cfg.codec {
            Codec::H264 => MFVideoFormat_H264,
            Codec::H265 => MFVideoFormat_HEVC,
        };
        out.SetGUID(&MF_MT_SUBTYPE, &codec).map_err(err)?;
        let bitrate = ((w as u64 * h as u64 * u64::from(cfg.fps)) / 8).clamp(500_000, 40_000_000);
        out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate as u32)
            .map_err(err)?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(err)?;
        set_size(&out, &MF_MT_FRAME_SIZE, w, h).map_err(err)?;
        set_size(&out, &MF_MT_FRAME_RATE, cfg.fps, 1).map_err(err)?;
        set_size(&out, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1).map_err(err)?;
        let stream = writer.AddStream(&out).map_err(err)?;

        let input = MFCreateMediaType().map_err(err)?;
        input
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(err)?;
        input
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(err)?;
        input
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(err)?;
        set_size(&input, &MF_MT_FRAME_SIZE, w, h).map_err(err)?;
        set_size(&input, &MF_MT_FRAME_RATE, cfg.fps, 1).map_err(err)?;
        set_size(&input, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1).map_err(err)?;
        // Positive default stride = top-down rows, the Raster layout.
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4).map_err(err)?;
        writer
            .SetInputMediaType(stream, &input, None)
            .map_err(err)?;
        let audio_stream = if audio {
            Some(add_audio_stream(&writer)?)
        } else {
            None
        };
        writer.BeginWriting().map_err(err)?;
        Ok((writer, audio_stream))
    }
}

fn make_sample(frame: &Raster, ts_100ns: i64, dur_100ns: i64) -> Result<IMFSample, RecordError> {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let len = w * h * 4;
    // SAFETY: the memory buffer is locked for the duration of the copy and sized exactly `len`.
    unsafe {
        let buf = MFCreateMemoryBuffer(len as u32).map_err(err)?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        buf.Lock(&mut ptr, None, None).map_err(err)?;
        if ptr.is_null() {
            return Err(err("buffer lock returned null"));
        }
        let dst = std::slice::from_raw_parts_mut(ptr, len);
        for (dp, sp) in dst.chunks_exact_mut(4).zip(frame.data().chunks_exact(4)) {
            dp.copy_from_slice(&[sp[2], sp[1], sp[0], 255]);
        }
        buf.Unlock().map_err(err)?;
        buf.SetCurrentLength(len as u32).map_err(err)?;
        let sample = MFCreateSample().map_err(err)?;
        sample.AddBuffer(&buf).map_err(err)?;
        sample.SetSampleTime(ts_100ns).map_err(err)?;
        sample.SetSampleDuration(dur_100ns).map_err(err)?;
        Ok(sample)
    }
}

fn run_capture(
    screen: Arc<dyn ScreenSource>,
    cfg: RecordConfig,
    shared: Arc<Shared>,
    webcam: Webcam,
    ready: std::sync::mpsc::Sender<Result<(), RecordError>>,
) -> Result<RecordingResult, RecordError> {
    let setup = (|| {
        init_mf()?;
        let (w, h) = cfg.encode_size();
        let sources = Audio::open(&cfg)?;
        let (writer, stream) = make_writer(&cfg, w, h, sources.is_some())?;
        let audio = match (sources, stream) {
            (Some(sources), Some(stream)) => Some(Audio {
                mixer: AudioMixer::new(sources.len()),
                sources,
                stream,
            }),
            _ => None,
        };
        Ok::<_, RecordError>((writer, w, h, audio))
    })();
    let (writer, w, h, mut audio) = match setup {
        Ok(v) => {
            let _ = ready.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = ready.send(Err(RecordError::Platform(e.to_string())));
            return Err(e);
        }
    };
    let frame_dt = Duration::from_secs_f64(1.0 / f64::from(cfg.fps));
    let dur_100ns = (10_000_000.0 / f64::from(cfg.fps)) as i64;
    let start = Instant::now();
    let mut paused_total = Duration::ZERO;
    let mut paused_since: Option<Instant> = None;
    let mut next = start;
    let mut frames = 0u64;
    let mut region = Rect::new(cfg.region.x, cfg.region.y, w, h);
    let mut ripples: Vec<(Point, u64)> = Vec::new();
    let mut was_down = false;
    if let Some(a) = &mut audio {
        a.collect();
        a.mixer.discard(); // packets captured while the encoder was being set up
    }

    while !shared.stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        if shared.paused.load(Ordering::SeqCst) {
            paused_since.get_or_insert(now);
            if let Some(a) = &mut audio {
                a.collect();
                a.mixer.discard();
            }
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if let Some(p) = paused_since.take() {
            paused_total += now - p;
            next = now;
        }
        if let Some(a) = &mut audio {
            a.pump(&writer, (now - start - paused_total).as_millis() as u64)?;
        }
        if now < next {
            std::thread::sleep((next - now).min(Duration::from_millis(5)));
            continue;
        }
        next += frame_dt;
        if next < now {
            next = now + frame_dt; // fell behind: do not burst
        }
        // Follow a window as it moves (size stays fixed so the encoder keeps its geometry).
        if let Some(id) = cfg.window {
            if let Ok(wins) = screen.windows() {
                if let Some(win) = wins.iter().find(|x| x.id == id) {
                    region = Rect::new(win.rect.x, win.rect.y, w, h);
                }
            }
        }
        let Ok(mut frame) = screen.grab(region) else {
            continue;
        };
        if frame.width() != w || frame.height() != h {
            continue; // region partly off-screen this tick
        }
        let t_ms = (now - start - paused_total).as_millis() as u64;
        let cursor = screen.cursor();
        if cfg.click_ripples {
            let down = crate::input::left_button_down();
            if down && !was_down {
                if let Some(c) = cursor {
                    ripples.push((c.pos, t_ms));
                }
            }
            was_down = down;
            ripples.retain(|(_, at)| t_ms.saturating_sub(*at) < 600);
        }
        if cfg.cursor_highlight || cfg.click_ripples {
            if let Some(c) = cursor {
                let local = Point::new(c.pos.x - region.x, c.pos.y - region.y);
                draw_cursor_effects(
                    &mut frame,
                    local,
                    cfg.cursor_highlight,
                    &ripples
                        .iter()
                        .map(|(p, at)| (Point::new(p.x - region.x, p.y - region.y), *at))
                        .collect::<Vec<_>>(),
                    t_ms,
                );
            }
        }
        if let Some((cam, layout)) = &webcam {
            if let (Some(cf), Ok(l)) = (cam.frame(), layout.lock()) {
                frame = composite_webcam(&frame, &cf, &l);
            }
        }
        let ts = (t_ms as i64) * 10_000;
        let sample = make_sample(&frame, ts, dur_100ns)?;
        // SAFETY: stream 0 exists; sample lifetime is managed by COM.
        unsafe { writer.WriteSample(0, &sample).map_err(err)? };
        frames += 1;
    }
    let elapsed = start.elapsed().saturating_sub(paused_total);
    let has_audio = audio.is_some();
    if let Some(mut a) = audio {
        a.collect();
        let tail = a.mixer.flush(elapsed.as_millis() as u64);
        a.write(&writer, tail)?;
    }
    // SAFETY: finalise writes the moov atom; required for a playable MP4.
    unsafe { writer.Finalize().map_err(err)? };
    Ok(RecordingResult {
        path: cfg.output.clone(),
        duration_ms: elapsed.as_millis() as u64,
        frames,
        width: w,
        height: h,
        has_audio,
    })
}

impl Recorder for MfRecorder {
    fn start(&mut self, cfg: &RecordConfig) -> Result<(), RecordError> {
        cfg.validate()?;
        if self.worker.is_some() {
            return Err(err("already recording"));
        }
        if let Some(dir) = cfg.output.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        self.shared.stop.store(false, Ordering::SeqCst);
        self.shared.paused.store(false, Ordering::SeqCst);
        let webcam = self.webcam.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let (screen, cfg, shared) = (self.screen.clone(), cfg.clone(), self.shared.clone());
        self.worker = Some(std::thread::spawn(move || {
            run_capture(screen, cfg, shared, webcam, tx)
        }));
        rx.recv_timeout(Duration::from_secs(20))
            .map_err(|_| err("recorder did not start"))?
    }

    fn attach_webcam(
        &mut self,
        cam: Arc<dyn CameraSource>,
        layout: PipLayout,
    ) -> Result<(), RecordError> {
        self.webcam = Some((cam, Arc::new(Mutex::new(layout))));
        Ok(())
    }

    fn set_pip_layout(&mut self, layout: PipLayout) -> Result<(), RecordError> {
        let (_, l) = self
            .webcam
            .as_ref()
            .ok_or_else(|| err("no webcam attached"))?;
        *l.lock().map_err(|_| err("layout lock poisoned"))? = layout;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), RecordError> {
        self.shared.paused.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn resume(&mut self) -> Result<(), RecordError> {
        self.shared.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) -> Result<RecordingResult, RecordError> {
        self.shared.stop.store(true, Ordering::SeqCst);
        let worker = self.worker.take().ok_or_else(|| err("not recording"))?;
        worker.join().map_err(|_| err("recorder thread panicked"))?
    }
}

// ------------------------------------------------------------------ reader

struct Cached(std::path::PathBuf, IMFSourceReader);
// SAFETY: the wrapped COM object is only touched while holding the reader's mutexes.
unsafe impl Send for Cached {}

#[derive(Default)]
pub struct MfVideoReader {
    lock: Mutex<()>,
    cache: Mutex<Option<Cached>>,
}

impl MfVideoReader {
    pub fn new() -> Self {
        Self::default()
    }
}

fn open(path: &Path) -> Result<IMFSourceReader, RecordError> {
    init_mf()?;
    // SAFETY: attributes and reader are COM objects owned by the wrappers.
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1).map_err(err)?;
        let attrs = attrs.ok_or_else(|| err("no attributes"))?;
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .map_err(err)?;
        let p = wide(path);
        let reader = MFCreateSourceReaderFromURL(PCWSTR(p.as_ptr()), &attrs).map_err(err)?;
        let want = MFCreateMediaType().map_err(err)?;
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(err)?;
        want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(err)?;
        reader
            .SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &want)
            .map_err(err)?;
        Ok(reader)
    }
}

fn frame_size(reader: &IMFSourceReader) -> Result<(u32, u32, i32), RecordError> {
    // SAFETY: attribute reads on a valid media type.
    unsafe {
        let mt = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            .map_err(err)?;
        let packed = mt.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
        let (w, h) = ((packed >> 32) as u32, packed as u32);
        let stride = mt
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|s| s as i32)
            .unwrap_or((w * 4) as i32);
        Ok((w, h, stride))
    }
}

impl VideoReader for MfVideoReader {
    fn probe(&self, path: &Path) -> Result<VideoInfo, RecordError> {
        let _g = self.lock.lock().map_err(|_| err("reader lock poisoned"))?;
        let reader = open(path)?;
        let (width, height, _) = frame_size(&reader)?;
        // SAFETY: PROPVARIANT read of a VT_UI8 duration (100 ns units) and a media type query.
        let (duration_100ns, has_audio) = unsafe {
            let v = reader
                .GetPresentationAttribute(0xFFFF_FFFF_u32, &MF_PD_DURATION)
                .map_err(err)?;
            let audio = reader
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, 0)
                .is_ok();
            (u64::try_from(&v).unwrap_or(0), audio)
        };
        Ok(VideoInfo {
            duration_ms: duration_100ns / 10_000,
            width,
            height,
            has_audio,
        })
    }

    fn frame_at(&self, path: &Path, ms: u64) -> Result<Raster, RecordError> {
        use windows::Win32::Media::MediaFoundation::MF_SOURCE_READER_FLAG;
        use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
        let _g = self.lock.lock().map_err(|_| err("reader lock poisoned"))?;
        let mut cache = self.cache.lock().map_err(|_| err("cache lock poisoned"))?;
        if cache.as_ref().is_none_or(|c| c.0 != path) {
            *cache = Some(Cached(path.to_path_buf(), open(path)?));
        }
        let reader = cache
            .as_ref()
            .map(|c| c.1.clone())
            .ok_or_else(|| err("no reader"))?;
        drop(cache);
        let (w, h, _) = frame_size(&reader)?;
        let target = (ms as i64) * 10_000;
        // SAFETY: seek + read loop; buffers are locked only while copying.
        unsafe {
            let pos = PROPVARIANT::from(target);
            reader
                .SetCurrentPosition(&GUID::zeroed(), &pos)
                .map_err(err)?;
            let mut last: Option<IMFSample> = None;
            for _ in 0..600 {
                let (mut flags, mut ts) = (0u32, 0i64);
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut ts),
                        Some(&mut sample),
                    )
                    .map_err(err)?;
                if flags & (MF_SOURCE_READER_FLAG(2).0 as u32) != 0 {
                    break; // end of stream
                }
                if let Some(s) = sample {
                    last = Some(s);
                    if ts >= target {
                        break;
                    }
                }
            }
            let sample = last.ok_or_else(|| err("no frame at that position"))?;
            // The decoder renegotiates its output type (padded size, stride) once it sees the
            // stream, so the format is read again after decoding.
            let (cw, ch, stride) = frame_size(&reader)?;
            let (ox, oy, w, h) = display_area(&reader).unwrap_or((0, 0, cw.min(w), ch.min(h)));
            let buf = sample.GetBufferByIndex(0).map_err(err)?;
            if let Ok(b2) = buf.cast::<IMF2DBuffer2>() {
                let (mut scan0, mut pitch) = (std::ptr::null_mut::<u8>(), 0i32);
                let (mut start, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
                b2.Lock2DSize(
                    MF2DBuffer_LockFlags_Read,
                    &mut scan0,
                    &mut pitch,
                    &mut start,
                    &mut len,
                )
                .map_err(err)?;
                let data = std::slice::from_raw_parts(start, len as usize);
                let top = scan0.offset_from(start);
                let r = copy_bgra(data, top, pitch as isize, ox, oy, w, h);
                b2.Unlock2D().map_err(err)?;
                return r;
            }
            let buf = sample.ConvertToContiguousBuffer().map_err(err)?;
            let (mut ptr, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
            buf.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
            let data = std::slice::from_raw_parts(ptr, len as usize);
            let row = (stride.unsigned_abs() as usize).max(cw as usize * 4) as isize;
            // Positive stride = top-down rows; negative = the last row comes first in memory.
            let (top, pitch) = if stride < 0 {
                (row * (ch as isize - 1), -row)
            } else {
                (0, row)
            };
            let r = copy_bgra(data, top, pitch, ox, oy, w, h);
            buf.Unlock().map_err(err)?;
            r
        }
    }
}

/// Visible area of the decoded frame (`MF_MT_MINIMUM_DISPLAY_APERTURE`): x, y, width, height.
fn display_area(reader: &IMFSourceReader) -> Option<(u32, u32, u32, u32)> {
    // SAFETY: blob read into a 16-byte buffer (sizeof MFVideoArea) on a valid media type.
    unsafe {
        let mt = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            .ok()?;
        let mut b = [0u8; 16];
        let mut n = 0u32;
        mt.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut b, Some(&mut n))
            .ok()?;
        if n != 16 {
            return None;
        }
        let ox = i16::from_le_bytes([b[2], b[3]]).max(0) as u32;
        let oy = i16::from_le_bytes([b[6], b[7]]).max(0) as u32;
        let w = i32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        let h = i32::from_le_bytes([b[12], b[13], b[14], b[15]]);
        (w > 0 && h > 0).then_some((ox, oy, w as u32, h as u32))
    }
}

/// Copy a `w`×`h` window at (`ox`, `oy`) out of a BGRA frame whose top row starts at byte `top`
/// of `data` and whose rows are `pitch` bytes apart (negative for bottom-up memory).
fn copy_bgra(
    data: &[u8],
    top: isize,
    pitch: isize,
    ox: u32,
    oy: u32,
    w: u32,
    h: u32,
) -> Result<Raster, RecordError> {
    let short = || err("short frame buffer");
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as isize {
        let at = top
            .checked_add(pitch.checked_mul(y + oy as isize).ok_or_else(short)?)
            .and_then(|v| v.checked_add(ox as isize * 4))
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(short)?;
        let line = data.get(at..at + w as usize * 4).ok_or_else(short)?;
        for px in line.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    Raster::from_rgba(w, h, rgba).map_err(err)
}

// ------------------------------------------------------------------ writer (video editing output)

pub struct MfVideoWriter;

impl VideoWriter for MfVideoWriter {
    fn write(
        &self,
        out: &Path,
        width: u32,
        height: u32,
        fps: u32,
        frames: &mut dyn Iterator<Item = Result<Raster, RecordError>>,
    ) -> Result<RecordingResult, RecordError> {
        init_mf()?;
        let cfg = RecordConfig {
            region: Rect::new(0, 0, width, height),
            window: None,
            fps,
            codec: Codec::H264,
            mic: None,
            system_audio: false,
            cursor_highlight: false,
            click_ripples: false,
            output: out.to_path_buf(),
        };
        let (writer, _) = make_writer(&cfg, width, height, false)?;
        let dur = (10_000_000.0 / f64::from(fps)) as i64;
        let mut n = 0u64;
        for f in frames {
            let f = f?;
            if f.width() != width || f.height() != height {
                return Err(err("frame size changed mid-stream"));
            }
            let sample = make_sample(&f, n as i64 * dur, dur)?;
            // SAFETY: stream 0 exists for the writer created above.
            unsafe { writer.WriteSample(0, &sample).map_err(err)? };
            n += 1;
        }
        // SAFETY: finalise so the MP4 is playable.
        unsafe { writer.Finalize().map_err(err)? };
        Ok(RecordingResult {
            path: out.to_path_buf(),
            duration_ms: n * 1000 / u64::from(fps),
            frames: n,
            width,
            height,
            has_audio: false,
        })
    }
}
