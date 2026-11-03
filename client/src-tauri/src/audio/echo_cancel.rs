//! Mic capture with acoustic echo cancellation, via Windows' Voice Capture DSP
//! (`CWMAudioAEC`, part of Windows since Vista). In source mode it opens the default mic and
//! the default speakers itself, subtracts what the speakers play from what the mic hears, and
//! returns 16 kHz mono PCM. Without it, on speakers the prospect's voice leaks into the mic
//! and is transcribed as the rep.

use super::{AudioChunk, CaptureEvent, Channel, CHUNK_MS, SAMPLE_RATE};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;
use windows::core::{implement, Interface, GUID};
use windows::Win32::Foundation::{E_INVALIDARG, PROPERTYKEY, VARIANT_TRUE};
use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVE_FORMAT_PCM};
use windows::Win32::Media::DxMediaObjects::{
    IMediaBuffer, IMediaBuffer_Impl, IMediaObject, DMO_MEDIA_TYPE, DMO_OUTPUT_DATA_BUFFER,
    DMO_OUTPUT_DATA_BUFFERF_INCOMPLETE,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::{VT_BOOL, VT_I4};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

const CLSID_CWMAUDIOAEC: GUID = GUID::from_u128(0x745057c7_f353_4f2d_a7ee_58434477730e);
const MEDIATYPE_AUDIO: GUID = GUID::from_u128(0x73647561_0000_0010_8000_00aa00389b71);
const MEDIASUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const FORMAT_WAVEFORMATEX: GUID = GUID::from_u128(0x05589f81_c356_11ce_bf01_00aa0055595a);

/// `MFPKEY_WMAAECMA_*` property keys of the Voice Capture DSP.
const WMAAECMA: GUID = GUID::from_u128(0x6f52c567_0360_4bd2_9617_ccbf1421c939);
const SYSTEM_MODE: PROPERTYKEY = PROPERTYKEY {
    fmtid: WMAAECMA,
    pid: 2,
};
const DMO_SOURCE_MODE: PROPERTYKEY = PROPERTYKEY {
    fmtid: WMAAECMA,
    pid: 3,
};
/// `SINGLE_CHANNEL_AEC`: mono mic with echo cancellation.
const MODE_SINGLE_CHANNEL_AEC: i32 = 0;

/// How often the DSP is asked for output. It produces audio in 10 ms frames.
const POLL: Duration = Duration::from_millis(10);
const BYTES_PER_SAMPLE: usize = 2;

/// The output buffer the DSP writes into.
#[implement(IMediaBuffer)]
struct Buffer {
    data: RefCell<Vec<u8>>,
}

impl IMediaBuffer_Impl for Buffer_Impl {
    fn SetLength(&self, length: u32) -> windows::core::Result<()> {
        let mut data = self.data.borrow_mut();
        if length as usize > data.capacity() {
            return Err(E_INVALIDARG.into());
        }
        // The DSP has written `length` bytes through the pointer from GetBufferAndLength.
        unsafe { data.set_len(length as usize) };
        Ok(())
    }

    fn GetMaxLength(&self) -> windows::core::Result<u32> {
        Ok(self.data.borrow().capacity() as u32)
    }

    fn GetBufferAndLength(
        &self,
        buffer: *mut *mut u8,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        let mut data = self.data.borrow_mut();
        unsafe {
            if !buffer.is_null() {
                *buffer = data.as_mut_ptr();
            }
            if !length.is_null() {
                *length = data.len() as u32;
            }
        }
        Ok(())
    }
}

fn prop_bool(value: bool) -> PROPVARIANT {
    let mut prop = PROPVARIANT::default();
    unsafe {
        let inner = &mut prop.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if value {
            VARIANT_TRUE
        } else {
            Default::default()
        };
    }
    prop
}

fn prop_i32(value: i32) -> PROPVARIANT {
    let mut prop = PROPVARIANT::default();
    unsafe {
        let inner = &mut prop.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = value;
    }
    prop
}

/// Creates the DSP in source mode, producing 16 kHz mono 16-bit PCM. COM must already be
/// initialized on this thread.
fn open() -> windows::core::Result<IMediaObject> {
    let dmo: IMediaObject =
        unsafe { CoCreateInstance(&CLSID_CWMAUDIOAEC, None, CLSCTX_INPROC_SERVER)? };
    let props: IPropertyStore = dmo.cast()?;
    unsafe {
        props.SetValue(&DMO_SOURCE_MODE, &prop_bool(true))?;
        props.SetValue(&SYSTEM_MODE, &prop_i32(MODE_SINGLE_CHANNEL_AEC))?;
    }

    let mut format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * BYTES_PER_SAMPLE as u32,
        nBlockAlign: BYTES_PER_SAMPLE as u16,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let media_type = DMO_MEDIA_TYPE {
        majortype: MEDIATYPE_AUDIO,
        subtype: MEDIASUBTYPE_PCM,
        bFixedSizeSamples: true.into(),
        bTemporalCompression: false.into(),
        lSampleSize: BYTES_PER_SAMPLE as u32,
        formattype: FORMAT_WAVEFORMATEX,
        pUnk: ManuallyDrop::new(None),
        cbFormat: std::mem::size_of::<WAVEFORMATEX>() as u32,
        pbFormat: &mut format as *mut WAVEFORMATEX as *mut u8,
    };
    unsafe {
        dmo.SetOutputType(0, Some(&media_type), 0)?;
        dmo.AllocateStreamingResources()?;
    }
    Ok(dmo)
}

/// Off unless `MIC_ECHO_CANCELLATION=on`: its benefit hasn't been measured on speakers yet
/// (see `echo_cancellation_live`), and it changes the mic's levels.
pub fn enabled() -> bool {
    crate::config::env_or_dotenv("MIC_ECHO_CANCELLATION")
        .is_some_and(|v| v.eq_ignore_ascii_case("on"))
}

/// Captures the echo-cancelled mic until `stop` is set. COM must be initialized on this
/// thread. Returns an error without sending anything if the DSP can't start.
pub fn capture(stop: &AtomicBool, events: &Sender<CaptureEvent>) -> Result<(), String> {
    let dmo = open().map_err(|e| format!("Echo cancellation unavailable: {e}"))?;
    let result = pump(&dmo, stop, events);
    unsafe {
        let _ = dmo.FreeStreamingResources();
    }
    result
}

fn pump(
    dmo: &IMediaObject,
    stop: &AtomicBool,
    events: &Sender<CaptureEvent>,
) -> Result<(), String> {
    let buffer: IMediaBuffer = Buffer {
        data: RefCell::new(Vec::with_capacity(SAMPLE_RATE as usize * BYTES_PER_SAMPLE)),
    }
    .into();
    let chunk_bytes = (SAMPLE_RATE * CHUNK_MS / 1000) as usize * BYTES_PER_SAMPLE;
    let mut queue: VecDeque<u8> = VecDeque::with_capacity(chunk_bytes * 4);

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        loop {
            let mut outputs = [DMO_OUTPUT_DATA_BUFFER {
                pBuffer: ManuallyDrop::new(Some(buffer.clone())),
                dwStatus: 0,
                rtTimestamp: 0,
                rtTimelength: 0,
            }];
            let mut status = 0;
            let processed = unsafe { dmo.ProcessOutput(0, &mut outputs, &mut status) };
            let flags = outputs[0].dwStatus;
            unsafe { ManuallyDrop::drop(&mut outputs[0].pBuffer) };
            if let Err(e) = processed {
                return Err(format!("Echo-cancelled capture failed: {e}"));
            }
            let mut bytes = std::ptr::null_mut();
            let mut len = 0u32;
            unsafe {
                buffer
                    .GetBufferAndLength(Some(&mut bytes), Some(&mut len))
                    .map_err(|e| e.to_string())?;
                queue.extend(std::slice::from_raw_parts(bytes, len as usize));
                buffer.SetLength(0).map_err(|e| e.to_string())?;
            }
            if flags & DMO_OUTPUT_DATA_BUFFERF_INCOMPLETE.0 as u32 == 0 {
                break;
            }
        }

        while queue.len() >= chunk_bytes {
            let samples = queue
                .drain(..chunk_bytes)
                .collect::<Vec<u8>>()
                .as_chunks::<BYTES_PER_SAMPLE>()
                .0
                .iter()
                .map(|&b| i16::from_le_bytes(b))
                .collect();
            let chunk = AudioChunk {
                channel: Channel::Me,
                samples,
                captured_at: super::captured_before(queue.len()),
            };
            if events.send(CaptureEvent::Chunk(chunk)).is_err() {
                return Ok(());
            }
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::gate::level_db;
    use crate::audio::LoopbackTarget;
    use std::sync::{mpsc, Arc};
    use std::time::Instant;

    fn record_levels(echo_cancel: bool, secs: u64) -> (Vec<f64>, Vec<f64>) {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<CaptureEvent>();
        let loopback = crate::audio::wasapi_capture::spawn(
            Channel::Them,
            LoopbackTarget::DefaultOutput,
            stop.clone(),
            tx.clone(),
        )
        .unwrap();
        let mic = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                wasapi::initialize_mta().ok().unwrap();
                if echo_cancel {
                    capture(&stop, &tx).unwrap();
                } else {
                    drop(tx);
                }
                wasapi::deinitialize();
            })
        };
        let raw_mic = (!echo_cancel).then(|| {
            let (tx2, rx2) = mpsc::channel::<CaptureEvent>();
            (
                crate::audio::wasapi_capture::spawn(
                    Channel::Me,
                    LoopbackTarget::DefaultOutput,
                    stop.clone(),
                    tx2,
                )
                .unwrap(),
                rx2,
            )
        });
        let speaker = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Volume = 100; Start-Sleep -Milliseconds 1500; $s.Speak('Honestly our cloud bill has doubled this year. We talked to two agencies and neither could explain where the money goes. What would an audit actually cost us, and how long would it take?')",
            ])
            .spawn()
            .unwrap();
        let (mut mic_db, mut loop_db) = (Vec::new(), Vec::new());
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if let Ok(CaptureEvent::Chunk(chunk)) = rx.recv_timeout(Duration::from_millis(20)) {
                let level = level_db(&chunk.samples);
                match chunk.channel {
                    Channel::Me => mic_db.push(level),
                    Channel::Them => loop_db.push(level),
                }
            }
            if let Some((_, rx2)) = &raw_mic {
                while let Ok(CaptureEvent::Chunk(chunk)) = rx2.try_recv() {
                    mic_db.push(level_db(&chunk.samples));
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        let mut speaker = speaker;
        let _ = speaker.wait();
        loopback.join().unwrap();
        mic.join().unwrap();
        if let Some((handle, _)) = raw_mic {
            handle.join().unwrap();
        }
        (mic_db, loop_db)
    }

    /// How much of the speaker signal shows up in the mic: the mic level while the speakers
    /// play speech, minus while they're quiet, and the correlation of the two level curves.
    fn echo_report(label: &str, mic: &[f64], speaker: &[f64]) {
        let n = mic.len().min(speaker.len());
        let (mut loud, mut quiet) = (Vec::new(), Vec::new());
        for i in 0..n {
            if speaker[i] > -45.0 {
                loud.push(mic[i]);
            } else if speaker[i] < -80.0 {
                quiet.push(mic[i]);
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let (mx, sx) = (mean(&mic[..n]), mean(&speaker[..n]));
        let cov: f64 = (0..n).map(|i| (mic[i] - mx) * (speaker[i] - sx)).sum();
        let var_m: f64 = (0..n).map(|i| (mic[i] - mx).powi(2)).sum();
        let var_s: f64 = (0..n).map(|i| (speaker[i] - sx).powi(2)).sum();
        println!(
            "{label}: mic {:.1} dBFS while speakers play ({} chunks), {:.1} dBFS while quiet ({} chunks): +{:.1} dB; correlation {:.2}",
            mean(&loud),
            loud.len(),
            mean(&quiet),
            quiet.len(),
            mean(&loud) - mean(&quiet),
            cov / (var_m.sqrt() * var_s.sqrt()).max(1e-9),
        );
    }

    /// Live, on speakers (not headphones): plays speech and measures how much of it the mic
    /// picks up, without and with echo cancellation.
    /// `cargo test echo_cancellation_live -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore]
    fn echo_cancellation_live() {
        let (mic, speaker) = record_levels(false, 16);
        echo_report("plain mic      ", &mic, &speaker);
        let (mic, speaker) = record_levels(true, 16);
        echo_report("echo-cancelled ", &mic, &speaker);
    }
}
