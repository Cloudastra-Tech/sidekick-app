//! WASAPI capture: the mic chosen in the app (else the default communications capture
//! device), and system audio via shared-mode loopback. Each stream runs on its own thread with its own COM
//! apartment.

use super::{AudioChunk, CaptureEvent, Channel, LoopbackTarget, CHUNK_MS, SAMPLE_RATE};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use wasapi::{
    deinitialize, initialize_mta, AudioClient, DeviceEnumerator, Direction, Role, SampleType,
    StreamMode, WasapiError, WaveFormat,
};

const AUDCLNT_E_DEVICE_INVALIDATED: u32 = 0x8889_0004;
const AUDCLNT_E_DEVICE_IN_USE: u32 = 0x8889_000A;
const E_NOTFOUND: u32 = 0x8007_0490;

const WAIT_TIMEOUT_MS: u32 = 200;
const BYTES_PER_SAMPLE: usize = 2;

#[derive(Clone, Serialize)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    /// The Windows default communications mic, used when no mic is chosen.
    pub is_default: bool,
}

/// Active capture devices, for the mic picker. Runs on its own thread so COM can be set up
/// in a multithreaded apartment whatever thread the caller is on.
pub fn list_input_devices() -> Result<Vec<InputDevice>, String> {
    thread::spawn(|| {
        initialize_mta()
            .ok()
            .map_err(|e| format!("Could not initialize COM: {e}"))?;
        let result = (|| -> Result<Vec<InputDevice>, WasapiError> {
            let enumerator = DeviceEnumerator::new()?;
            let default_id = enumerator
                .get_default_device_for_role(&Direction::Capture, &Role::Communications)
                .and_then(|d| d.get_id())
                .ok();
            let mut devices = Vec::new();
            for device in &enumerator.get_device_collection(&Direction::Capture)? {
                let device = device?;
                let id = device.get_id()?;
                devices.push(InputDevice {
                    is_default: default_id.as_deref() == Some(id.as_str()),
                    name: device.get_friendlyname()?,
                    id,
                });
            }
            Ok(devices)
        })();
        deinitialize();
        result.map_err(|e| format!("Could not list microphones: {e}"))
    })
    .join()
    .map_err(|_| "Listing microphones failed".to_string())?
}

/// The name of the default speaker or headset, which system-audio capture records, for the
/// call's log. On its own thread for COM, like `list_input_devices`.
pub fn default_output_name() -> Option<String> {
    thread::spawn(|| {
        initialize_mta().ok().ok()?;
        let name = DeviceEnumerator::new()
            .and_then(|e| e.get_default_device(&Direction::Render))
            .and_then(|d| d.get_friendlyname())
            .ok();
        deinitialize();
        name
    })
    .join()
    .ok()
    .flatten()
}

/// The mic chosen in the app, if any.
fn chosen_mic() -> Option<String> {
    crate::config::env_or_dotenv(crate::settings::MIC_DEVICE_SETTING)
        .filter(|id| !id.trim().is_empty())
}

pub fn spawn(
    channel: Channel,
    target: LoopbackTarget,
    stop: Arc<AtomicBool>,
    events: Sender<CaptureEvent>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("capture-{channel:?}"))
        .spawn(move || {
            if let Err(message) = run(channel, target, &stop, &events) {
                let _ = events.send(CaptureEvent::Failed { channel, message });
            }
        })
}

fn run(
    channel: Channel,
    target: LoopbackTarget,
    stop: &AtomicBool,
    events: &Sender<CaptureEvent>,
) -> Result<(), String> {
    initialize_mta()
        .ok()
        .map_err(|e| format!("Could not initialize COM: {e}"))?;

    let mic = if channel == Channel::Me {
        chosen_mic()
    } else {
        None
    };

    // Echo-cancelled mic when enabled; if the DSP can't start, fall back to the plain mic.
    // The DSP records the Windows default mic, so a mic chosen in the app skips it.
    if channel == Channel::Me && mic.is_some() && super::echo_cancel::enabled() {
        eprintln!(
            "Echo cancellation only works with the default mic; using the chosen mic without it"
        );
    } else if channel == Channel::Me && super::echo_cancel::enabled() {
        match super::echo_cancel::capture(stop, events) {
            Ok(()) => {
                deinitialize();
                return Ok(());
            }
            Err(message) => eprintln!("{message}; using the mic without echo cancellation"),
        }
    }

    let mut result = Ok(());
    while !stop.load(Ordering::Relaxed) {
        match capture_stream(channel, target, mic.as_deref(), stop, events) {
            Ok(()) => break,
            // Device unplugged, default device switched, or sleep/wake: reopen and continue.
            Err(e) if hresult(&e) == Some(AUDCLNT_E_DEVICE_INVALIDATED) => {
                thread::sleep(Duration::from_millis(250));
            }
            Err(e) => {
                result = Err(describe(channel, &e));
                break;
            }
        }
    }

    deinitialize();
    result
}

fn open_client(
    channel: Channel,
    target: LoopbackTarget,
    mic: Option<&str>,
) -> Result<AudioClient, WasapiError> {
    match (channel, target) {
        (Channel::Me, _) => {
            let enumerator = DeviceEnumerator::new()?;
            // A chosen mic that's gone (unplugged) falls back to the default.
            if let Some(device) = mic.and_then(|id| enumerator.get_device(id).ok()) {
                return device.get_iaudioclient();
            }
            if mic.is_some() {
                eprintln!("The chosen mic isn't available; using the default mic");
            }
            // Call apps use the communications role, which can differ from the console default.
            enumerator
                .get_default_device_for_role(&Direction::Capture, &Role::Communications)?
                .get_iaudioclient()
        }
        (Channel::Them, LoopbackTarget::DefaultOutput) => DeviceEnumerator::new()?
            .get_default_device(&Direction::Render)?
            .get_iaudioclient(),
        (Channel::Them, LoopbackTarget::Process(pid)) => {
            AudioClient::new_application_loopback_client(pid, true)
        }
    }
}

/// Captures until `stop` is set (Ok) or the stream errors (Err). The stream is always
/// stopped explicitly before returning.
fn capture_stream(
    channel: Channel,
    target: LoopbackTarget,
    mic: Option<&str>,
    stop: &AtomicBool,
    events: &Sender<CaptureEvent>,
) -> Result<(), WasapiError> {
    let mut client = open_client(channel, target, mic)?;
    let format = WaveFormat::new(16, 16, &SampleType::Int, SAMPLE_RATE as usize, 1, None);
    // Shared mode only: loopback does not work in exclusive mode. Autoconvert lets
    // Windows resample the device mix format down to 16 kHz mono.
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 1_000_000, // 100 ms
    };
    // Initializing a render device with Direction::Capture sets the loopback flag.
    client.initialize_client(&format, &Direction::Capture, &mode)?;
    let ready = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;

    let chunk_bytes = (SAMPLE_RATE * CHUNK_MS / 1000) as usize * BYTES_PER_SAMPLE;
    let mut queue: VecDeque<u8> = VecDeque::with_capacity(chunk_bytes * 4);

    client.start_stream()?;
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        match ready.wait_for_event(WAIT_TIMEOUT_MS) {
            Ok(()) => {
                if let Err(e) = drain_packets(&capture, &mut queue) {
                    break Err(e);
                }
            }
            // Loopback delivers no packets while nothing is playing. Fill the gap with
            // silence so downstream timing (silence/turn-end detection) stays continuous.
            Err(WasapiError::EventTimeout) if channel == Channel::Them => {
                let gap = (SAMPLE_RATE * WAIT_TIMEOUT_MS / 1000) as usize * BYTES_PER_SAMPLE;
                queue.extend(std::iter::repeat_n(0u8, gap));
            }
            Err(WasapiError::EventTimeout) => {}
            Err(e) => break Err(e),
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
                channel,
                samples,
                captured_at: super::captured_before(queue.len()),
            };
            if events.send(CaptureEvent::Chunk(chunk)).is_err() {
                // Receiver gone: the session was torn down.
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    };

    let _ = client.stop_stream();
    result
}

fn drain_packets(
    capture: &wasapi::AudioCaptureClient,
    queue: &mut VecDeque<u8>,
) -> Result<(), WasapiError> {
    while capture.get_next_packet_size()?.unwrap_or(0) > 0 {
        let start = queue.len();
        let info = capture.read_from_device_to_deque(queue)?;
        if info.flags.silent {
            // Buffer contents are undefined when flagged silent.
            queue.range_mut(start..).for_each(|b| *b = 0);
        }
    }
    Ok(())
}

fn hresult(err: &WasapiError) -> Option<u32> {
    match err {
        WasapiError::Windows(e) => Some(e.code().0 as u32),
        _ => None,
    }
}

fn describe(channel: Channel, err: &WasapiError) -> String {
    let device = match channel {
        Channel::Me => "microphone",
        Channel::Them => "audio output device",
    };
    match hresult(err) {
        Some(AUDCLNT_E_DEVICE_IN_USE) => format!(
            "The {device} is being used in exclusive mode by another app (e.g. DAW or \
             pro-audio software). Capture only works in shared mode: close that app or turn \
             off \"Allow applications to take exclusive control\" in Windows Sound settings."
        ),
        Some(E_NOTFOUND) => format!("No default {device} found."),
        _ => format!("Could not capture from the {device}: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Diagnostic: lists output devices, which ones are the Windows defaults, and each
    /// device's peak level over a few seconds — shows where audio is actually playing.
    /// `cargo test list_output_devices -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn list_output_devices() {
        initialize_mta().ok().unwrap();
        let enumerator = DeviceEnumerator::new().unwrap();
        let id_of = |role: &Role| {
            enumerator
                .get_default_device_for_role(&Direction::Render, role)
                .and_then(|d| d.get_id())
                .ok()
        };
        let console = id_of(&Role::Console);
        let communications = id_of(&Role::Communications);
        let devices = enumerator
            .get_device_collection(&Direction::Render)
            .unwrap();
        let count = devices.get_nbr_devices().unwrap();
        let mut peaks = vec![0f32; count as usize];
        for _ in 0..30 {
            for (i, peak) in peaks.iter_mut().enumerate() {
                if let Ok(level) = devices
                    .get_device_at_index(i as u32)
                    .and_then(|d| d.get_audiometerinformation())
                    .and_then(|m| m.get_peak_value())
                {
                    *peak = peak.max(level);
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        for (i, peak) in peaks.iter().enumerate() {
            let device = devices.get_device_at_index(i as u32).unwrap();
            let id = device.get_id().ok();
            println!(
                "output {i}: {:?} | default: {} | communications default: {} | peak over 3s: {:.3}",
                device.get_friendlyname().unwrap_or_default(),
                id == console,
                id == communications,
                peak
            );
        }
    }

    /// Needs real audio hardware: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn captures_both_channels_separately() {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let handles = [Channel::Me, Channel::Them].map(|channel| {
            spawn(
                channel,
                LoopbackTarget::DefaultOutput,
                stop.clone(),
                tx.clone(),
            )
            .unwrap()
        });
        drop(tx);
        thread::sleep(Duration::from_secs(3));
        stop.store(true, Ordering::Relaxed);
        handles.into_iter().for_each(|h| h.join().unwrap());

        let (mut me, mut them, mut peak_them) = (0, 0, 0i16);
        for event in rx {
            match event {
                CaptureEvent::Chunk(c) if c.channel == Channel::Me => me += c.samples.len(),
                CaptureEvent::Chunk(c) => {
                    them += c.samples.len();
                    peak_them = peak_them.max(
                        c.samples
                            .iter()
                            .map(|s| s.saturating_abs())
                            .max()
                            .unwrap_or(0),
                    );
                }
                CaptureEvent::Failed { channel, message } => panic!("{channel:?}: {message}"),
            }
        }
        println!("me: {me} samples, them: {them} samples, them peak: {peak_them}");
        assert!(me > 0 && them > 0);
    }
}
