//! Plays the audio track of a recorded MP4 through the default speakers.
//!
//! The picture stays with `MfVideoReader` (a separate file reader, so seeking a frame does not
//! move the sound). This opens the audio stream, asks Media Foundation for 48 kHz stereo PCM and
//! writes it to a shared-mode WASAPI render client. A file with no audio track is not an error.

use screenit_record::{RecordError, TrackPlayer};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX, eConsole, eRender,
};
use windows::Win32::Media::MediaFoundation::{
    IMFSourceReader, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_FIRST_AUDIO_STREAM, MFAudioFormat_PCM,
    MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Audio,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    StructuredStorage::PROPVARIANT,
};
use windows::core::GUID;

fn err<E: std::fmt::Display>(e: E) -> RecordError {
    RecordError::Platform(e.to_string())
}

const RATE: u32 = screenit_record::audio::RATE;
const CHANNELS: u16 = screenit_record::audio::CHANNELS;
const BYTES_PER_FRAME: usize = 4;

enum Cmd {
    Play {
        path: PathBuf,
        ms: u64,
        ack: Sender<Result<(), RecordError>>,
    },
    Pause,
    Stop,
}

struct Output {
    client: IAudioClient,
    render: IAudioRenderClient,
    reader: IMFSourceReader,
    pending: Vec<u8>,
    eof: bool,
}

impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: this client was started on this thread; Stop is safe if it already stopped.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// One playback thread for the life of the app. Commands are answered in order, so a `play`
/// that follows a `pause` starts from the position it was given.
pub struct MfTrackPlayer {
    tx: Option<Sender<Cmd>>,
    worker: Option<JoinHandle<()>>,
}

impl MfTrackPlayer {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let worker = std::thread::spawn(move || play_loop(rx));
        Self {
            tx: Some(tx),
            worker: Some(worker),
        }
    }

    fn send(&self, cmd: Cmd) -> Result<(), RecordError> {
        self.tx
            .as_ref()
            .ok_or_else(|| err("player stopped"))?
            .send(cmd)
            .map_err(|_| err("player stopped"))
    }
}

impl Default for MfTrackPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MfTrackPlayer {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl TrackPlayer for MfTrackPlayer {
    fn play(&self, path: &Path, ms: u64) -> Result<(), RecordError> {
        let (ack_tx, ack_rx) = channel();
        self.send(Cmd::Play {
            path: path.to_path_buf(),
            ms,
            ack: ack_tx,
        })?;
        ack_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| err("player did not start"))?
    }

    fn pause(&self) {
        let _ = self.send(Cmd::Pause);
    }

    fn stop(&self) {
        let _ = self.send(Cmd::Stop);
    }
}

fn play_loop(rx: Receiver<Cmd>) {
    // SAFETY: this thread owns every COM object it creates and releases them before exiting.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let _ = crate::mf::init_mf();
    let mut out: Option<Output> = None;
    loop {
        let cmd = if out.is_some() {
            match rx.recv_timeout(Duration::from_millis(12)) {
                Ok(cmd) => Some(cmd),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => break,
            }
        };
        match cmd {
            Some(Cmd::Play { path, ms, ack }) => {
                drop(out.take());
                let (next, result) = match open_output(&path, ms) {
                    Ok(next) => (next, Ok(())),
                    Err(e) => (None, Err(e)),
                };
                if ack.send(result).is_err() {
                    break;
                }
                out = next;
            }
            Some(Cmd::Pause | Cmd::Stop) => out = None,
            None => {
                if let Some(playing) = out.as_mut() {
                    let failed = fill(playing).is_err();
                    let done = playing.eof && playing.pending.is_empty();
                    if failed || done {
                        out = None;
                    }
                }
            }
        }
    }
}

/// `Ok(None)` is a file with no audio track.
fn open_output(path: &Path, ms: u64) -> Result<Option<Output>, RecordError> {
    let reader = open_audio(path)?;
    let Some(reader) = reader else {
        return Ok(None);
    };
    seek(&reader, ms)?;
    let (client, render) = open_render()?;
    Ok(Some(Output {
        client,
        render,
        reader,
        pending: Vec::new(),
        eof: false,
    }))
}

fn open_audio(path: &Path) -> Result<Option<IMFSourceReader>, RecordError> {
    let wide = wide_path(path);
    // SAFETY: the URL is a null-terminated path and the media type outlives SetCurrentMediaType,
    // which copies it.
    unsafe {
        let reader =
            MFCreateSourceReaderFromURL(windows::core::PCWSTR(wide.as_ptr()), None).map_err(err)?;
        let mt = MFCreateMediaType().map_err(err)?;
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(err)?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(err)?;
        mt.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(CHANNELS))
            .map_err(err)?;
        mt.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)
            .map_err(err)?;
        mt.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(err)?;
        mt.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, BYTES_PER_FRAME as u32)
            .map_err(err)?;
        mt.SetUINT32(
            &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            RATE * BYTES_PER_FRAME as u32,
        )
        .map_err(err)?;
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        if reader.GetNativeMediaType(stream, 0).is_err() {
            return Ok(None);
        }
        reader.SetCurrentMediaType(stream, None, &mt).map_err(err)?;
        Ok(Some(reader))
    }
}

/// Longest track decoded whole (30 minutes of 48 kHz stereo).
const MAX_DECODE_SAMPLES: usize = 48_000 * 2 * 60 * 30;

/// Decode the whole audio track to interleaved 48 kHz stereo samples. No track gives an empty vec.
pub(crate) fn decode_all(path: &Path) -> Result<Vec<i16>, RecordError> {
    use windows::Win32::Media::MediaFoundation::MF_SOURCE_READER_FLAG;
    crate::mf::init_mf()?;
    let Some(reader) = open_audio(path)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<i16> = Vec::new();
    while out.len() < MAX_DECODE_SAMPLES {
        // SAFETY: ReadSample and the buffer lock use the reader owned here; the lock is
        // released before the sample is dropped.
        unsafe {
            let (mut flags, mut ts) = (0u32, 0i64);
            let mut sample = None;
            reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(err)?;
            if let Some(sample) = sample {
                let buf = sample.ConvertToContiguousBuffer().map_err(err)?;
                let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
                buf.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
                if !ptr.is_null() && len > 0 {
                    let n = (len as usize) / BYTES_PER_FRAME * BYTES_PER_FRAME;
                    let bytes = std::slice::from_raw_parts(ptr, n);
                    out.extend(
                        bytes
                            .chunks_exact(2)
                            .map(|b| i16::from_le_bytes([b[0], b[1]])),
                    );
                }
                buf.Unlock().map_err(err)?;
            }
            if flags & (MF_SOURCE_READER_FLAG(2).0 as u32) != 0 {
                break;
            }
        }
    }
    Ok(out)
}

fn seek(reader: &IMFSourceReader, ms: u64) -> Result<(), RecordError> {
    let hns = (ms.min(i64::MAX as u64 / 10_000) as i64).saturating_mul(10_000);
    // SAFETY: GUID_NULL selects 100-nanosecond presentation time; the PROPVARIANT is copied.
    unsafe {
        reader
            .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(hns))
            .map_err(err)
    }
}

fn open_render() -> Result<(IAudioClient, IAudioRenderClient), RecordError> {
    let fmt = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * BYTES_PER_FRAME as u32,
        nBlockAlign: BYTES_PER_FRAME as u16,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    // SAFETY: COM activation of the default render endpoint. Initialize copies `fmt`. The
    // client is started only after the render service exists.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).map_err(err)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(err)?;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                // 100 ms. A longer buffer would play ahead of the wall-clock picture.
                1_000_000,
                0,
                &fmt,
                None,
            )
            .map_err(err)?;
        let render: IAudioRenderClient = client.GetService().map_err(err)?;
        client.Start().map_err(err)?;
        Ok((client, render))
    }
}

fn fill(out: &mut Output) -> Result<(), RecordError> {
    // SAFETY: the client and render service belong to `out` and are only used on this thread.
    // GetBuffer's pointer is valid until the matching ReleaseBuffer.
    unsafe {
        let size = out.client.GetBufferSize().map_err(err)?;
        let pad = out.client.GetCurrentPadding().map_err(err)?;
        let want = size.saturating_sub(pad) as usize;
        if want < RATE as usize / 100 {
            return Ok(());
        }
        pull_audio(out, want * BYTES_PER_FRAME)?;
        let frames = (out.pending.len() / BYTES_PER_FRAME).min(want);
        if frames == 0 {
            return Ok(());
        }
        let dest = out.render.GetBuffer(frames as u32).map_err(err)?;
        if dest.is_null() {
            let _ = out.render.ReleaseBuffer(0, 0);
            return Err(err("render buffer"));
        }
        std::ptr::copy_nonoverlapping(out.pending.as_ptr(), dest, frames * BYTES_PER_FRAME);
        out.render.ReleaseBuffer(frames as u32, 0).map_err(err)?;
        out.pending.drain(..frames * BYTES_PER_FRAME);
        Ok(())
    }
}

fn pull_audio(out: &mut Output, want_bytes: usize) -> Result<(), RecordError> {
    use windows::Win32::Media::MediaFoundation::MF_SOURCE_READER_FLAG;
    for _ in 0..8 {
        if out.eof || out.pending.len() >= want_bytes {
            break;
        }
        // SAFETY: ReadSample and the buffer lock are on the reader this thread owns. The lock
        // is released before the sample is dropped.
        unsafe {
            let (mut flags, mut ts) = (0u32, 0i64);
            let mut sample = None;
            out.reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(err)?;
            if flags & (MF_SOURCE_READER_FLAG(2).0 as u32) != 0 {
                out.eof = true;
            }
            let Some(sample) = sample else { continue };
            let buf = sample.ConvertToContiguousBuffer().map_err(err)?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
            if !ptr.is_null() && len > 0 {
                let n = (len as usize) / BYTES_PER_FRAME * BYTES_PER_FRAME;
                out.pending
                    .extend_from_slice(std::slice::from_raw_parts(ptr, n));
            }
            buf.Unlock().map_err(err)?;
        }
    }
    Ok(())
}

fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
