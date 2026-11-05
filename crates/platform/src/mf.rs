#![allow(clippy::collapsible_if)]
//! Media Foundation: H.264/H.265 MP4 screen recorder (`Recorder`) and a frame reader
//! (`VideoReader`). Capture is GDI-polled at the target frame rate; no external binaries.

use crate::audio::{AudioCapture, AudioKind};
use screenit_capture::ScreenSource;
use screenit_geom::{Point, Rect};
use screenit_raster::Raster;
use screenit_record::audio::{AudioChunk, AudioMixer, CHANNELS, RATE};
use screenit_record::{
    CameraSource, Codec, OutputSize, PipLayout, RecordConfig, RecordError, Recorder,
    RecordingResult, VideoInfo, VideoReader, VideoWriter, composite_webcam, draw_cursor_effects,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer2, IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter, IMFSourceReader,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_MPEG2_LEVEL, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_PD_DURATION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
    MF2DBuffer_LockFlags_Read, MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
    MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_FULL, MFStartup,
    MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
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

/// Raises the system timer resolution to 1 ms while alive so frame pacing sleeps are accurate
/// (the default 15.6 ms tick makes a 30 fps loop drift to ~24 fps).
struct FineTimer;

impl FineTimer {
    fn start() -> Self {
        // SAFETY: plain winmm call, undone by 	imeEndPeriod in Drop.
        unsafe { windows::Win32::Media::timeBeginPeriod(1) };
        Self
    }
}

impl Drop for FineTimer {
    fn drop(&mut self) {
        // SAFETY: matches the 	imeBeginPeriod(1) in start.
        unsafe { windows::Win32::Media::timeEndPeriod(1) };
    }
}

/// The sink writer is free-threaded: the video thread writes stream 0 while the capture thread
/// writes the audio stream and finalises only after the video thread has been joined.
struct SharedWriter(IMFSinkWriter);

// SAFETY: see above; MF sink writers serialise calls internally and are not thread-affine.
unsafe impl Send for SharedWriter {}

type Webcam = Option<(Arc<dyn CameraSource>, Arc<Mutex<PipLayout>>)>;

struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
    /// Top-left of the captured area. The capture thread reads it every frame.
    origin_x: AtomicI32,
    origin_y: AtomicI32,
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
                origin_x: AtomicI32::new(0),
                origin_y: AtomicI32::new(0),
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

/// `eAVEncH264VProfile_High`.
const H264_PROFILE_HIGH: u32 = 100;

/// Average bitrate: 0.125 bits per pixel per frame (H.265 needs about 60% of that), clamped to
/// 0.5..=100 Mbit/s so 4K60 stays within what the encoders and MP4 players accept.
fn bitrate_for(w: u32, h: u32, fps: u32, codec: Codec) -> u32 {
    let bits = u64::from(w) * u64::from(h) * u64::from(fps) / 8;
    let bits = if codec == Codec::H265 {
        bits * 3 / 5
    } else {
        bits
    };
    bits.clamp(500_000, 100_000_000) as u32
}

/// Smallest H.264 level (`eAVEncH264VLevel*`, level × 10) that covers the frame size and the
/// macroblock rate: 4.0 for 1080p30 up to 5.2 for 4K60.
fn h264_level(w: u32, h: u32, fps: u32) -> u32 {
    let mbs = u64::from(w.div_ceil(16)) * u64::from(h.div_ceil(16));
    let rate = mbs * u64::from(fps);
    // (level, max frame macroblocks, max macroblocks per second)
    const LEVELS: [(u32, u64, u64); 6] = [
        (40, 8_192, 245_760),
        (42, 8_704, 522_240),
        (50, 22_080, 589_824),
        (51, 36_864, 983_040),
        (52, 36_864, 2_073_600),
        (62, 139_264, 16_711_680),
    ];
    LEVELS
        .iter()
        .find(|(_, frame, per_s)| mbs <= *frame && rate <= *per_s)
        .map_or(62, |l| l.0)
}

/// Writer for the frame size. Above 1080p the software encoder struggles to keep up, so a
/// hardware encoder is tried first; any failure falls back to the software encoder.
fn open_writer(
    cfg: &RecordConfig,
    w: u32,
    h: u32,
    audio: bool,
) -> Result<(IMFSinkWriter, Option<u32>), RecordError> {
    if u64::from(w) * u64::from(h) > 1920 * 1088 {
        match make_writer(cfg, w, h, audio, true) {
            Ok(v) => return Ok(v),
            Err(e) => eprintln!("hardware encoder unavailable ({e}); using software"),
        }
    }
    // A failed hardware attempt can hold the output file for a moment: retry a few times.
    let mut last = err("no attempt");
    for _ in 0..10 {
        match make_writer(cfg, w, h, audio, false) {
            Ok(v) => return Ok(v),
            Err(e) if e.to_string().contains("0x80070020") => {
                last = e;
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

fn make_writer(
    cfg: &RecordConfig,
    w: u32,
    h: u32,
    audio: bool,
    hardware: bool,
) -> Result<(IMFSinkWriter, Option<u32>), RecordError> {
    // SAFETY: straightforward MF calls; every COM object is reference-counted by the wrappers.
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2).map_err(err)?;
        let attrs = attrs.ok_or_else(|| err("no attributes"))?;
        attrs
            .SetUINT32(
                &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                u32::from(hardware),
            )
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
        out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_for(w, h, cfg.fps, cfg.codec))
            .map_err(err)?;
        if cfg.codec == Codec::H264 {
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, H264_PROFILE_HIGH)
                .map_err(err)?;
            out.SetUINT32(&MF_MT_MPEG2_LEVEL, h264_level(w, h, cfg.fps))
                .map_err(err)?;
        }
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

/// Bilinear-scale an RGBA frame into BGRA bytes of `dw`×`dh` (alpha forced opaque). Rows are
/// split across threads so a 4K output stays within a frame interval.
fn scale_to_bgra(frame: &Raster, dst: &mut [u8], dw: usize, dh: usize) {
    let (sw, sh) = (frame.width() as usize, frame.height() as usize);
    let src = frame.data();
    if sw == 0
        || sh == 0
        || dw == 0
        || dh == 0
        || dst.len() < dw * dh * 4
        || src.len() < sw * sh * 4
    {
        return;
    }
    // Per output column: left/right source columns and the right weight (0..=256).
    let axis = |dn: usize, sn: usize| -> Vec<(usize, usize, u32)> {
        (0..dn)
            .map(|i| {
                let f =
                    ((i as f64 + 0.5) * sn as f64 / dn as f64 - 0.5).clamp(0.0, (sn - 1) as f64);
                let a = f.floor() as usize;
                let b = (a + 1).min(sn - 1);
                (a, b, ((f - a as f64) * 256.0) as u32)
            })
            .collect()
    };
    let (xs, ys) = (axis(dw, sw), axis(dh, sh));
    let band = dh.div_ceil(4).max(1);
    std::thread::scope(|scope| {
        for (bi, rows) in dst[..dw * dh * 4].chunks_mut(band * dw * 4).enumerate() {
            let (xs, ys) = (&xs, &ys);
            scope.spawn(move || {
                for (ri, row) in rows.chunks_exact_mut(dw * 4).enumerate() {
                    let (y0, y1, wy) = ys[bi * band + ri];
                    let (r0, r1) = (&src[y0 * sw * 4..], &src[y1 * sw * 4..]);
                    for (px, &(x0, x1, wx)) in row.chunks_exact_mut(4).zip(xs.iter()) {
                        let mix = |c: usize| {
                            let top = u32::from(r0[x0 * 4 + c]) * (256 - wx)
                                + u32::from(r0[x1 * 4 + c]) * wx;
                            let bot = u32::from(r1[x0 * 4 + c]) * (256 - wx)
                                + u32::from(r1[x1 * 4 + c]) * wx;
                            ((top * (256 - wy) + bot * wy) >> 16) as u8
                        };
                        px.copy_from_slice(&[mix(2), mix(1), mix(0), 255]);
                    }
                }
            });
        }
    });
}

fn make_sample(frame: &Raster, ts_100ns: i64, dur_100ns: i64) -> Result<IMFSample, RecordError> {
    make_sample_sized(frame, frame.width(), frame.height(), ts_100ns, dur_100ns)
}

/// Sample of `w`×`h` BGRA pixels; the frame is scaled when its size differs.
fn make_sample_sized(
    frame: &Raster,
    w: u32,
    h: u32,
    ts_100ns: i64,
    dur_100ns: i64,
) -> Result<IMFSample, RecordError> {
    let scaled = frame.width() != w || frame.height() != h;
    let (w, h) = (w as usize, h as usize);
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
        if scaled {
            scale_to_bgra(frame, dst, w, h);
        } else {
            for (dp, sp) in dst.chunks_exact_mut(4).zip(frame.data().chunks_exact(4)) {
                dp.copy_from_slice(&[sp[2], sp[1], sp[0], 255]);
            }
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
        let (writer, stream) = open_writer(&cfg, w, h, sources.is_some())?;
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
    let _timer = FineTimer::start();
    let frame_dt = Duration::from_secs_f64(1.0 / f64::from(cfg.fps));
    let dur_100ns = (10_000_000.0 / f64::from(cfg.fps)) as i64;
    let start = Instant::now();
    let mut paused_total = Duration::ZERO;
    let mut paused_since: Option<Instant> = None;
    let mut next = start;
    let mut frames = 0u64;
    // Stage two of the pipeline: pixel conversion (and scaling) plus the encoder hand-off.
    let (tx, rx) = std::sync::mpsc::sync_channel::<(Raster, i64)>(2);
    let video = {
        let shared = SharedWriter(writer.clone());
        std::thread::spawn(move || -> Result<(), RecordError> {
            let writer = shared;
            for (frame, ts) in rx {
                let sample = make_sample_sized(&frame, w, h, ts, dur_100ns)?;
                // SAFETY: stream 0 exists; the sink writer accepts samples from any thread.
                unsafe { writer.0.WriteSample(0, &sample).map_err(err)? };
            }
            Ok(())
        })
    };
    let (sw, sh) = cfg.source_size();
    let mut region = Rect::new(
        shared.origin_x.load(Ordering::Relaxed),
        shared.origin_y.load(Ordering::Relaxed),
        sw,
        sh,
    );
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
        // A drag of the on-screen frame moves the area; the size stays fixed.
        region.x = shared.origin_x.load(Ordering::Relaxed);
        region.y = shared.origin_y.load(Ordering::Relaxed);
        // Follow a window as it moves (size stays fixed so the encoder keeps its geometry).
        if let Some(id) = cfg.window {
            if let Ok(wins) = screen.windows() {
                if let Some(win) = wins.iter().find(|x| x.id == id) {
                    region = Rect::new(win.rect.x, win.rect.y, sw, sh);
                }
            }
        }
        let Ok(mut frame) = screen.grab_stream(region) else {
            continue;
        };
        if frame.width() != sw || frame.height() != sh {
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
        // Converting/scaling and encoding overlap with grabbing the next frame.
        if tx.send((frame, ts)).is_err() {
            break; // the video thread stopped on an error; reported when it is joined
        }
        frames += 1;
    }
    drop(tx);
    video.join().map_err(|_| err("video thread panicked"))??;
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
        self.shared.origin_x.store(cfg.region.x, Ordering::SeqCst);
        self.shared.origin_y.store(cfg.region.y, Ordering::SeqCst);
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

    fn move_region(&mut self, x: i32, y: i32) -> Result<(), RecordError> {
        self.shared.origin_x.store(x, Ordering::SeqCst);
        self.shared.origin_y.store(y, Ordering::SeqCst);
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

struct Cached {
    path: std::path::PathBuf,
    reader: IMFSourceReader,
    /// Timestamp (100 ns) of `frame`. The reader sits just after that sample.
    at: Option<i64>,
    frame: Option<Raster>,
}
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
        if cache.as_ref().is_none_or(|c| c.path != path) {
            *cache = Some(Cached {
                path: path.to_path_buf(),
                reader: open(path)?,
                at: None,
                frame: None,
            });
        }
        let target = (ms as i64).saturating_mul(10_000);
        // A time we already passed: hand back the frame we decoded, do not seek to the start.
        if let Some(c) = cache.as_ref()
            && c.at.is_some_and(|at| target <= at)
            && let Some(frame) = c.frame.clone()
        {
            return Ok(frame);
        }
        let (reader, forward) = {
            let c = cache.as_ref().ok_or_else(|| err("no reader"))?;
            (c.reader.clone(), c.at.is_some_and(|at| target > at))
        };
        drop(cache);
        let (nw, nh, _) = frame_size(&reader)?;
        // SAFETY: seek + read loop; buffers are locked only while copying.
        let (raster, delivered) = unsafe {
            if !forward {
                let pos = PROPVARIANT::from(target);
                reader
                    .SetCurrentPosition(&GUID::zeroed(), &pos)
                    .map_err(err)?;
            }
            let mut last: Option<IMFSample> = None;
            let mut last_ts = target;
            for _ in 0..8_000 {
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
                    last_ts = ts;
                    last = Some(s);
                    if ts >= target {
                        break;
                    }
                }
            }
            let Some(sample) = last else {
                return self.held_frame(path);
            };
            // The decoder renegotiates its output type (padded size, stride) once it sees the
            // stream, so the format is read again after decoding.
            let (cw, ch, stride) = frame_size(&reader)?;
            let (ox, oy, w, h) = display_area(&reader).unwrap_or((0, 0, cw.min(nw), ch.min(nh)));
            let buf = sample.GetBufferByIndex(0).map_err(err)?;
            let raster = if let Ok(b2) = buf.cast::<IMF2DBuffer2>() {
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
                r?
            } else {
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
                r?
            };
            (raster, last_ts)
        };
        if let Ok(mut cache) = self.cache.lock()
            && let Some(c) = cache.as_mut()
            && c.path == path
        {
            c.at = Some(delivered);
            c.frame = Some(raster.clone());
        }
        Ok(raster)
    }
}

impl MfVideoReader {
    fn held_frame(&self, path: &Path) -> Result<Raster, RecordError> {
        self.cache
            .lock()
            .map_err(|_| err("cache lock poisoned"))?
            .as_ref()
            .filter(|c| c.path == path)
            .and_then(|c| c.frame.clone())
            .ok_or_else(|| err("no frame at that position"))
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
            size: OutputSize::Native,
            mic: None,
            system_audio: false,
            cursor_highlight: false,
            click_ripples: false,
            output: out.to_path_buf(),
        };
        let (writer, _) = open_writer(&cfg, width, height, false)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_capture::{CaptureError, CursorInfo, MonitorInfo, WindowInfo};
    use screenit_record::OutputSize;

    #[test]
    fn h264_level_and_bitrate_cover_1080p_to_4k60() {
        assert_eq!(h264_level(1920, 1080, 30), 40);
        assert_eq!(h264_level(1920, 1080, 60), 42);
        assert_eq!(h264_level(2560, 1440, 30), 50);
        assert_eq!(h264_level(3840, 2160, 30), 51);
        assert_eq!(h264_level(3840, 2160, 60), 52);
        assert_eq!(bitrate_for(1920, 1080, 30, Codec::H264), 7_776_000);
        assert_eq!(bitrate_for(3840, 2160, 60, Codec::H264), 62_208_000);
        assert_eq!(bitrate_for(7680, 4320, 120, Codec::H264), 100_000_000);
        assert_eq!(bitrate_for(2, 2, 1, Codec::H264), 500_000);
        assert!(
            bitrate_for(3840, 2160, 30, Codec::H265) < bitrate_for(3840, 2160, 30, Codec::H264)
        );
    }

    #[test]
    fn scaling_keeps_flat_colour_and_maps_edges() {
        let src = Raster::new(4, 4, [10, 20, 30, 255]).unwrap();
        let mut dst = vec![0u8; 8 * 6 * 4];
        scale_to_bgra(&src, &mut dst, 8, 6);
        assert!(dst.chunks_exact(4).all(|p| p == [30, 20, 10, 255]));
        // Left half black, right half white stays split down the middle after upscaling.
        let mut half = Raster::new(4, 2, [0, 0, 0, 255]).unwrap();
        for y in 0..2 {
            for x in 2..4 {
                half.put(x, y, [255, 255, 255, 255]);
            }
        }
        let mut out = vec![0u8; 8 * 4 * 4];
        scale_to_bgra(&half, &mut out, 8, 4);
        assert_eq!(out[0], 0);
        assert_eq!(out[(8 * 4 - 1) * 4], 255);
        // Degenerate sizes are ignored rather than panicking.
        scale_to_bgra(&half, &mut out, 0, 4);
        scale_to_bgra(&half, &mut [], 8, 4);
    }

    /// Frame with a bright vertical bar at x = w/4 and a bright horizontal bar at y = h/4 on a
    /// dark gradient: a sheared or mis-strided decode would move or smear them.
    fn bars(w: u32, h: u32, shift: u32) -> Raster {
        let mut data = vec![0u8; w as usize * h as usize * 4];
        let (vx, hy) = (w / 4 + shift, h / 4);
        for y in 0..h {
            for x in 0..w {
                let on = (vx..vx + 24).contains(&x) || (hy..hy + 24).contains(&y);
                let v = if on { 255 } else { 20 + (x * 40 / w) as u8 };
                let i = (y as usize * w as usize + x as usize) * 4;
                data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        Raster::from_rgba(w, h, data).unwrap()
    }

    fn assert_bars(frame: &Raster, w: u32, h: u32) {
        assert_eq!((frame.width(), frame.height()), (w, h));
        let lum = |x: u32, y: u32| u32::from(frame.get(x as i32, y as i32).unwrap()[0]);
        let (vx, hy) = (w / 4, h / 4);
        // Bar centres bright, a clear margin either side dark.
        assert!(lum(vx + 12, h / 2) > 200, "vertical bar missing");
        assert!(lum(vx - 40, h / 2) < 90, "vertical bar smeared left");
        assert!(lum(vx + 70, h / 2) < 90, "vertical bar smeared right");
        assert!(lum(w / 2, hy + 12) > 200, "horizontal bar missing");
        assert!(lum(w / 2, hy - 40) < 90, "horizontal bar smeared up");
        assert!(lum(w / 2, hy + 70) < 90, "horizontal bar smeared down");
    }

    fn temp_mp4(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("screenit-{name}-{}.mp4", std::process::id()))
    }

    fn write_and_check(w: u32, h: u32) {
        let out = temp_mp4(&format!("enc{w}x{h}"));
        let mut frames = (0..12).map(|i| Ok(bars(w, h, i)));
        let res = MfVideoWriter
            .write(&out, w, h, 30, &mut frames)
            .expect("encode");
        assert_eq!((res.width, res.height, res.frames), (w, h, 12));
        let frame = MfVideoReader::new().frame_at(&out, 0).expect("decode");
        assert_bars(&frame, w, h);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn encodes_and_decodes_2560x1440_without_shear() {
        write_and_check(2560, 1440);
    }

    #[test]
    fn encodes_and_decodes_3840x2160_without_shear() {
        write_and_check(3840, 2160);
    }

    /// A screen that always shows the same synthetic 1920x1080 picture.
    struct Synthetic(Raster);

    impl ScreenSource for Synthetic {
        fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
            Ok(vec![MonitorInfo {
                id: 0,
                name: "synthetic".into(),
                rect: Rect::new(0, 0, self.0.width(), self.0.height()),
                dpi: 96,
                primary: true,
            }])
        }
        fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
            Ok(Vec::new())
        }
        fn grab(&self, rect: Rect) -> Result<Raster, CaptureError> {
            self.0.crop(rect).ok_or(CaptureError::Empty)
        }
        fn grab_window(&self, id: u64) -> Result<Raster, CaptureError> {
            Err(CaptureError::NoWindow(id))
        }
        fn cursor(&self) -> Option<CursorInfo> {
            None
        }
        fn foreground(&self) -> Option<WindowInfo> {
            None
        }
    }

    #[test]
    fn recorder_scales_a_1080p_region_to_each_output_preset() {
        for (size, (w, h)) in [
            (OutputSize::P1440, (2560, 1440)),
            (OutputSize::P2160, (3840, 2160)),
            (OutputSize::P1080, (1920, 1080)),
        ] {
            let mut rec = MfRecorder::new(Arc::new(Synthetic(bars(1920, 1080, 0))));
            let out = temp_mp4(&format!("scale{}", size.as_str()));
            let cfg = RecordConfig {
                region: Rect::new(0, 0, 1920, 1080),
                window: None,
                fps: 15,
                codec: Codec::H264,
                size,
                mic: None,
                system_audio: false,
                cursor_highlight: false,
                click_ripples: false,
                output: out.clone(),
            };
            rec.start(&cfg).expect("start");
            std::thread::sleep(Duration::from_millis(1200));
            let res = rec.stop().expect("stop");
            assert_eq!((res.width, res.height), (w, h), "{size:?}");
            assert!(res.frames >= 5, "{size:?}: only {} frames", res.frames);
            let frame = MfVideoReader::new().frame_at(&out, 100).expect("decode");
            assert_bars(&frame, w, h);
            let _ = std::fs::remove_file(&out);
        }
    }

    #[test]
    fn scaling_1080p_to_4k_fits_a_frame_interval() {
        let src = bars(1920, 1080, 0);
        let mut dst = vec![0u8; 3840 * 2160 * 4];
        let t = Instant::now();
        for _ in 0..5 {
            scale_to_bgra(&src, &mut dst, 3840, 2160);
        }
        let ms = t.elapsed().as_secs_f64() * 200.0; // per frame, in ms
        eprintln!("scale 1080p -> 4K: {ms:.1} ms/frame");
        // Budget in perf/budgets.toml is 60 ms; allow noise on shared machines.
        assert!(ms < 150.0, "scaling took {ms:.1} ms per frame");
    }
}
