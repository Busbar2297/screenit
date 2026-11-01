#![allow(clippy::collapsible_if)]
//! WASAPI capture for recordings: a microphone endpoint or the default render endpoint in
//! loopback (system audio). The shared-mode engine converts to 48 kHz stereo 16-bit PCM
//! (`AUTOCONVERTPCM`), so the mixer and the AAC encoder only ever see one format.

use screenit_record::RecordError;
use screenit_record::audio::{CHANNELS, RATE};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE,
    IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVE_FORMAT_PCM, WAVEFORMATEX, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, STGM_READ,
};

fn err<E: std::fmt::Display>(e: E) -> RecordError {
    RecordError::Platform(e.to_string())
}

#[derive(Clone, Debug)]
pub(crate) enum AudioKind {
    /// Microphone by friendly name (`None` or `"default"` = the default communications input).
    Mic(Option<String>),
    /// Whatever the default output device is playing.
    System,
}

fn enumerator() -> Result<IMMDeviceEnumerator, RecordError> {
    // SAFETY: COM initialisation (idempotent) and creation of the documented enumerator class.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)
    }
}

fn friendly_name(dev: &IMMDevice) -> Option<String> {
    // SAFETY: read-only property store access; the PROPVARIANT owns its string.
    unsafe {
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let v = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let s = v.to_string();
        (!s.is_empty()).then_some(s)
    }
}

fn active_mics() -> Result<Vec<(String, IMMDevice)>, RecordError> {
    let en = enumerator()?;
    // SAFETY: enumeration of active capture endpoints; indices are bounded by GetCount.
    unsafe {
        let col = en
            .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
            .map_err(err)?;
        let n = col.GetCount().map_err(err)?;
        let mut out = Vec::new();
        for i in 0..n.min(64) {
            if let Ok(d) = col.Item(i) {
                if let Some(name) = friendly_name(&d) {
                    out.push((name, d));
                }
            }
        }
        Ok(out)
    }
}

/// Friendly names of the active microphones.
pub fn microphones() -> Vec<String> {
    active_mics()
        .map(|v| v.into_iter().map(|(n, _)| n).collect())
        .unwrap_or_default()
}

fn device_for(kind: &AudioKind) -> Result<IMMDevice, RecordError> {
    let en = enumerator()?;
    if let AudioKind::Mic(Some(want)) = kind {
        if !want.eq_ignore_ascii_case("default") {
            let want = want.to_lowercase();
            if let Some((_, d)) = active_mics()?
                .into_iter()
                .find(|(n, _)| n.to_lowercase().contains(&want))
            {
                return Ok(d);
            }
        }
    }
    let flow = match kind {
        AudioKind::Mic(_) => eCapture,
        AudioKind::System => eRender,
    };
    // SAFETY: plain COM call on a live enumerator.
    unsafe { en.GetDefaultAudioEndpoint(flow, eConsole) }.map_err(|e| match kind {
        AudioKind::Mic(_) => err(format!("no microphone: {e}")),
        AudioKind::System => err(format!("no audio output device: {e}")),
    })
}

fn open_client(kind: &AudioKind) -> Result<(IAudioClient, IAudioCaptureClient), RecordError> {
    let dev = device_for(kind)?;
    let block = CHANNELS * 2;
    let fmt = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if matches!(kind, AudioKind::System) {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    // SAFETY: `fmt` outlives Initialize (WASAPI copies it); 200 ms shared-mode buffer.
    unsafe {
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(err)?;
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None)
            .map_err(err)?;
        let cap: IAudioCaptureClient = client.GetService().map_err(err)?;
        client.Start().map_err(err)?;
        Ok((client, cap))
    }
}

/// A running capture thread delivering interleaved stereo i16 packets.
pub(crate) struct AudioCapture {
    pub rx: Receiver<Vec<i16>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl AudioCapture {
    pub fn open(kind: AudioKind) -> Result<Self, RecordError> {
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let worker = std::thread::spawn(move || pump(kind, tx, ready_tx, st));
        let mut me = Self {
            rx,
            stop,
            worker: Some(worker),
        };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(me),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                me.stop.store(true, Ordering::SeqCst);
                me.worker = None; // do not join a hung driver call
                Err(err("audio device did not start"))
            }
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn pump(
    kind: AudioKind,
    tx: Sender<Vec<i16>>,
    ready: Sender<Result<(), RecordError>>,
    stop: Arc<AtomicBool>,
) {
    let (client, cap) = match open_client(&kind) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let per_frame = usize::from(CHANNELS);
    'outer: while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));
        loop {
            // SAFETY: GetBuffer/ReleaseBuffer are paired; the slice is read before release and
            // its length is the frame count WASAPI reported for our 16-bit stereo format.
            let packet = unsafe {
                match cap.GetNextPacketSize() {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break 'outer,
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                if cap
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break 'outer;
                }
                let n = (frames as usize).min(RATE as usize) * per_frame;
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                let v = if silent || data.is_null() {
                    vec![0i16; n]
                } else {
                    std::slice::from_raw_parts(data.cast::<i16>(), n).to_vec()
                };
                let _ = cap.ReleaseBuffer(frames);
                v
            };
            if tx.send(packet).is_err() {
                break 'outer;
            }
        }
    }
    // SAFETY: stopping a started client; errors are irrelevant at teardown.
    unsafe {
        let _ = client.Stop();
    }
}
