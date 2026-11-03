//! macOS capture through Core Audio: the mic chosen in the app (else the default input), and
//! system audio through a process tap (macOS 14.2+). The tap hears every app except Sidekick and
//! sits alone on a private aggregate device. That device only runs while some app is playing,
//! so gaps are filled with silence to keep downstream timing continuous, like the Windows
//! loopback path. Adding the output device to the aggregate stops it
//! running when that output is a Bluetooth headset, so it is left out.
//!
//! Each stream runs on its own thread, which owns the Core Audio objects and reopens them when
//! the device goes away or the default device changes.

use super::{AudioChunk, CaptureEvent, Channel, LoopbackTarget, CHUNK_MS, SAMPLE_RATE};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioHardwarePropertyProcessObjectList,
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioHardwarePropertyTranslateUIDToDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject, kAudioProcessPropertyBundleID,
    kAudioProcessPropertyIsRunningInput, kAudioStreamPropertyVirtualFormat,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, AudioDeviceCreateIOProcIDWithBlock,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, CATapDescription, CATapMuteBehavior,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};
use serde::Serialize;
use std::ffi::{c_void, CStr};
use std::mem::{size_of, MaybeUninit};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;
const CHUNK_SAMPLES: usize = (SAMPLE_RATE * CHUNK_MS / 1000) as usize;
/// How often the capture thread checks for a stop request or a device change.
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
/// How long the tap may deliver nothing before the gap is filled with silence.
const SILENCE_AFTER: Duration = Duration::from_millis(200);

#[derive(Clone, Serialize)]
pub struct InputDevice {
    /// The Core Audio device UID, stable across restarts.
    pub id: String,
    pub name: String,
    /// The macOS default input, used when no mic is chosen.
    pub is_default: bool,
}

// ---- Core Audio property helpers -------------------------------------------------------------

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Reads a fixed-size property, optionally with qualifier data.
fn get_qualified<T: Copy>(
    object: AudioObjectID,
    selector: u32,
    scope: u32,
    qualifier: Option<(*const c_void, u32)>,
) -> Result<T, i32> {
    let address = address(selector, scope);
    let mut value = MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    let (q_ptr, q_size) = qualifier.unwrap_or((std::ptr::null(), 0));
    // SAFETY: `value` has room for `size` bytes and every pointer outlives the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            q_size,
            q_ptr,
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast()),
        )
    };
    if status != 0 {
        return Err(status);
    }
    // SAFETY: Core Audio filled the value.
    Ok(unsafe { value.assume_init() })
}

fn get<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Result<T, i32> {
    get_qualified(object, selector, scope, None)
}

/// Reads an array property.
fn get_list<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Result<Vec<T>, i32> {
    let address = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: plain out-parameter.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 {
        return Err(status);
    }
    let count = size as usize / size_of::<T>();
    let mut items: Vec<T> = Vec::with_capacity(count);
    if count == 0 {
        return Ok(items);
    }
    // SAFETY: `items` has capacity for `size` bytes; the length is set to what was written.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(items.as_mut_ptr().cast()),
        )
    };
    if status != 0 {
        return Err(status);
    }
    // SAFETY: Core Audio wrote `size` bytes of `T`s.
    unsafe { items.set_len(size as usize / size_of::<T>()) };
    Ok(items)
}

/// Reads a CFString property (returned retained, so it is released when dropped).
fn get_string(object: AudioObjectID, selector: u32, scope: u32) -> Option<String> {
    let raw: *mut c_void = get(object, selector, scope).ok()?;
    // SAFETY: CFString is toll-free bridged with NSString and Core Audio hands us a +1 reference.
    let string = unsafe { Retained::from_raw(raw.cast::<NSString>()) }?;
    Some(string.to_string())
}

fn ns(s: &CStr) -> Retained<NSString> {
    NSString::from_str(s.to_str().unwrap_or_default())
}

fn device_for_uid(uid: &str) -> Option<AudioObjectID> {
    let uid = NSString::from_str(uid);
    let uid_ref: *const c_void = Retained::as_ptr(&uid).cast();
    let device: AudioObjectID = get_qualified(
        SYSTEM,
        kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal,
        Some((
            (&uid_ref as *const *const c_void).cast(),
            size_of::<*const c_void>() as u32,
        )),
    )
    .ok()?;
    (device != 0).then_some(device)
}

fn default_device(selector: u32) -> Option<AudioObjectID> {
    get::<AudioObjectID>(SYSTEM, selector, kAudioObjectPropertyScopeGlobal)
        .ok()
        .filter(|&id| id != 0)
}

fn is_alive(device: AudioObjectID) -> bool {
    get::<u32>(
        device,
        kAudioDevicePropertyDeviceIsAlive,
        kAudioObjectPropertyScopeGlobal,
    )
    .is_ok_and(|alive| alive != 0)
}

fn input_streams(device: AudioObjectID) -> Vec<AudioObjectID> {
    get_list(
        device,
        kAudioDevicePropertyStreams,
        kAudioObjectPropertyScopeInput,
    )
    .unwrap_or_default()
}

// ---- Device listing and call detection ----------------------------------------------------

/// Devices that can record, for the mic picker.
pub fn list_input_devices() -> Result<Vec<InputDevice>, String> {
    let devices: Vec<AudioObjectID> = get_list(
        SYSTEM,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    )
    .map_err(|e| format!("Could not list microphones (error {e})"))?;
    let default = default_device(kAudioHardwarePropertyDefaultInputDevice);
    Ok(devices
        .into_iter()
        .filter(|&d| !input_streams(d).is_empty())
        .filter_map(|d| {
            let id = get_string(
                d,
                kAudioDevicePropertyDeviceUID,
                kAudioObjectPropertyScopeGlobal,
            )?;
            // Our own tap devices are private, but skip them by name just in case.
            if id.starts_with("com.sidekick.") {
                return None;
            }
            Some(InputDevice {
                name: get_string(d, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal)
                    .unwrap_or_else(|| id.clone()),
                is_default: Some(d) == default,
                id,
            })
        })
        .collect())
}

/// The name of the default speaker or headset, whose output the tap records, for the call's log.
pub fn default_output_name() -> Option<String> {
    let device = default_device(kAudioHardwarePropertyDefaultOutputDevice)?;
    get_string(
        device,
        kAudioObjectPropertyName,
        kAudioObjectPropertyScopeGlobal,
    )
}

/// Bundle IDs of processes recording from any input right now (macOS 14.2+).
pub fn processes_using_mic() -> Vec<String> {
    let Ok(processes) = get_list::<AudioObjectID>(
        SYSTEM,
        kAudioHardwarePropertyProcessObjectList,
        kAudioObjectPropertyScopeGlobal,
    ) else {
        return Vec::new();
    };
    let mut bundles: Vec<String> = processes
        .into_iter()
        .filter(|&p| {
            get::<u32>(
                p,
                kAudioProcessPropertyIsRunningInput,
                kAudioObjectPropertyScopeGlobal,
            )
            .is_ok_and(|running| running != 0)
        })
        .filter_map(|p| {
            get_string(
                p,
                kAudioProcessPropertyBundleID,
                kAudioObjectPropertyScopeGlobal,
            )
        })
        .filter(|b| !b.is_empty())
        .collect();
    bundles.sort();
    bundles.dedup();
    bundles
}

// ---- Capture --------------------------------------------------------------------------------

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
    // The tap always covers every app but Sidekick: a per-app tap would miss audio that call
    // apps play from helper processes (browsers, Teams).
    let _ = target;
    thread::Builder::new()
        .name(format!("capture-{channel:?}"))
        .spawn(move || {
            if let Err(message) = run(channel, &stop, &events) {
                let _ = events.send(CaptureEvent::Failed { channel, message });
            }
        })
}

fn run(
    channel: Channel,
    stop: &Arc<AtomicBool>,
    events: &Sender<CaptureEvent>,
) -> Result<(), String> {
    let mic = (channel == Channel::Me).then(chosen_mic).flatten();
    while !stop.load(Ordering::Relaxed) {
        let stream = match channel {
            Channel::Me => open_mic(mic.as_deref(), stop, events)?,
            Channel::Them => open_tap(stop, events)?,
        };
        // Watch until asked to stop or the device changes underneath us, then reopen.
        while !stop.load(Ordering::Relaxed) && stream.still_valid() {
            thread::sleep(WATCH_INTERVAL);
            if channel == Channel::Them {
                stream.sink.fill_silence();
            }
        }
        drop(stream);
        if !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

/// Turns float frames at the device rate into 16 kHz mono i16 chunks.
struct Downsampler {
    /// Output samples per input sample.
    step: f64,
    phase: f64,
    sum: f64,
    count: u32,
    pending: Vec<i16>,
    /// When the device last delivered audio.
    last_input: Instant,
}

impl Downsampler {
    fn new(input_rate: f64) -> Self {
        Self {
            step: f64::from(SAMPLE_RATE) / input_rate.max(1.0),
            phase: 0.0,
            sum: 0.0,
            count: 0,
            pending: Vec::with_capacity(CHUNK_SAMPLES * 2),
            last_input: Instant::now(),
        }
    }

    /// Averages the input samples that fall within each output sample: a box low-pass that is
    /// plenty for speech and cheap enough for the audio thread.
    fn push(&mut self, sample: f32) {
        self.sum += f64::from(sample);
        self.count += 1;
        self.phase += self.step;
        if self.phase < 1.0 {
            return;
        }
        let value = (self.sum / f64::from(self.count)).clamp(-1.0, 1.0);
        let value = (value * f64::from(i16::MAX)) as i16;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.pending.push(value);
        }
        self.sum = 0.0;
        self.count = 0;
    }
}

/// Owned by the IOProc block, which runs on Core Audio's real-time thread.
struct Sink {
    channel: Channel,
    events: Sender<CaptureEvent>,
    stop: Arc<AtomicBool>,
    /// Read the last input buffer instead of the first: on the tap's aggregate device, the tap's
    /// stream comes after any input streams of the output device.
    last_buffer: bool,
    state: Mutex<Downsampler>,
}

impl Sink {
    /// Hands every full chunk on. Called with the state locked.
    fn flush(&self, state: &mut Downsampler) {
        while state.pending.len() >= CHUNK_SAMPLES {
            let chunk: Vec<i16> = state.pending.drain(..CHUNK_SAMPLES).collect();
            let event = CaptureEvent::Chunk(AudioChunk {
                channel: self.channel,
                samples: chunk,
                captured_at: super::captured_before(state.pending.len() * 2),
            });
            if self.events.send(event).is_err() {
                // Receiver gone: the session was torn down.
                self.stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    }

    /// When no audio has arrived for a while (the tap's device idles while nothing plays), adds
    /// the missing time as silence.
    fn fill_silence(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let idle = state.last_input.elapsed();
        if idle < SILENCE_AFTER {
            return;
        }
        let samples = (idle.as_micros() * u128::from(SAMPLE_RATE) / 1_000_000) as usize;
        state.pending.extend(std::iter::repeat_n(0i16, samples));
        state.last_input = Instant::now();
        self.flush(&mut state);
    }
}

/// Runs on Core Audio's real-time thread for every IO cycle.
fn handle_input(sink: &Sink, input: NonNull<AudioBufferList>) {
    // SAFETY: Core Audio passes a valid buffer list for the duration of the call.
    let list = unsafe { input.as_ref() };
    let count = list.mNumberBuffers as usize;
    if count == 0 {
        return;
    }
    // SAFETY: the list holds `mNumberBuffers` buffers inline.
    let buffers = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), count) };
    let buffer = if sink.last_buffer {
        &buffers[count - 1]
    } else {
        &buffers[0]
    };
    if buffer.mData.is_null() {
        return;
    }
    let channels = buffer.mNumberChannels.max(1) as usize;
    // SAFETY: the stream format was checked to be 32-bit float when the stream was opened.
    let samples = unsafe {
        std::slice::from_raw_parts(
            buffer.mData as *const f32,
            buffer.mDataByteSize as usize / size_of::<f32>(),
        )
    };
    let Ok(mut state) = sink.state.lock() else {
        return;
    };
    state.last_input = Instant::now();
    for frame in samples.chunks_exact(channels) {
        state.push(frame.iter().sum::<f32>() / channels as f32);
    }
    sink.flush(&mut state);
}

/// The IOProc block's signature: timestamps and the input and output buffers.
type IoBlock = dyn Fn(
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
);

/// A running IOProc and the Core Audio objects behind it; dropping it stops and frees them.
struct Stream {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    /// Kept alive while the IOProc can still call it.
    _block: RcBlock<IoBlock>,
    sink: Arc<Sink>,
    /// Set for the tap path: the aggregate device and tap to destroy.
    tap: Option<AudioObjectID>,
    /// What to watch: the default device this stream follows, if it follows one.
    follows_default: Option<(u32, AudioObjectID)>,
}

impl Stream {
    fn start(
        device: AudioObjectID,
        channel: Channel,
        last_buffer: bool,
        stop: &Arc<AtomicBool>,
        events: &Sender<CaptureEvent>,
    ) -> Result<Self, String> {
        let streams = input_streams(device);
        let stream = if last_buffer {
            streams.last()
        } else {
            streams.first()
        }
        .copied()
        .ok_or("The device has no input")?;
        let format: AudioStreamBasicDescription = get(
            stream,
            kAudioStreamPropertyVirtualFormat,
            kAudioObjectPropertyScopeGlobal,
        )
        .map_err(|e| format!("Could not read the audio format (error {e})"))?;
        if format.mFormatFlags & kAudioFormatFlagIsFloat == 0 || format.mBitsPerChannel != 32 {
            return Err("The device does not deliver 32-bit float audio".into());
        }
        let rate = get::<f64>(
            device,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )
        .unwrap_or(format.mSampleRate);

        let sink = Arc::new(Sink {
            channel,
            events: events.clone(),
            stop: stop.clone(),
            last_buffer,
            state: Mutex::new(Downsampler::new(rate)),
        });
        let block_sink = sink.clone();
        // A block IOProc: on recent macOS a tap's aggregate device never runs a plain C IOProc.
        let block = RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>,
                  input: NonNull<AudioBufferList>,
                  _input_time: NonNull<AudioTimeStamp>,
                  _output: NonNull<AudioBufferList>,
                  _output_time: NonNull<AudioTimeStamp>| {
                handle_input(&block_sink, input)
            },
        );
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: Core Audio copies the block; we also keep it alive until the IOProc is gone.
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut proc_id),
                device,
                None,
                &*block as *const _ as *mut _,
            )
        };
        if status != 0 {
            return Err(format!("Could not open the audio device (error {status})"));
        }
        // SAFETY: the IOProc was just created on this device.
        let status = unsafe { AudioDeviceStart(device, proc_id) };
        if status != 0 {
            unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
            return Err(format!("Could not start the audio device (error {status})"));
        }
        Ok(Self {
            device,
            proc_id,
            _block: block,
            sink,
            tap: None,
            follows_default: None,
        })
    }

    fn still_valid(&self) -> bool {
        if !is_alive(self.device) {
            return false;
        }
        match self.follows_default {
            Some((selector, device)) => default_device(selector) == Some(device),
            None => true,
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: stopping and destroying the IOProc first means the sink is no longer used.
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            if let Some(tap) = self.tap {
                AudioHardwareDestroyAggregateDevice(self.device);
                AudioHardwareDestroyProcessTap(tap);
            }
        }
    }
}

fn open_mic(
    chosen: Option<&str>,
    stop: &Arc<AtomicBool>,
    events: &Sender<CaptureEvent>,
) -> Result<Stream, String> {
    // A chosen mic that's gone (unplugged) falls back to the default.
    if let Some(device) = chosen.and_then(device_for_uid) {
        return Stream::start(device, Channel::Me, false, stop, events);
    }
    if chosen.is_some() {
        eprintln!("The chosen mic isn't available; using the default mic");
    }
    let device = default_device(kAudioHardwarePropertyDefaultInputDevice)
        .ok_or("No microphone found. Plug one in or pick one in Settings.")?;
    let mut stream = Stream::start(device, Channel::Me, false, stop, events)?;
    stream.follows_default = Some((kAudioHardwarePropertyDefaultInputDevice, device));
    Ok(stream)
}

fn open_tap(stop: &Arc<AtomicBool>, events: &Sender<CaptureEvent>) -> Result<Stream, String> {
    let output = default_device(kAudioHardwarePropertyDefaultOutputDevice)
        .ok_or("No audio output device found.")?;

    // Leave Sidekick's own sounds out of the tap, when it has an audio process object yet.
    let pid = std::process::id() as i32;
    let own = get_qualified::<AudioObjectID>(
        SYSTEM,
        kAudioHardwarePropertyTranslatePIDToProcessObject,
        kAudioObjectPropertyScopeGlobal,
        Some(((&pid as *const i32).cast(), size_of::<i32>() as u32)),
    )
    .ok()
    .filter(|&p| p != 0);
    let excluded: Vec<Retained<NSNumber>> = own.into_iter().map(NSNumber::new_u32).collect();
    let excluded = NSArray::from_retained_slice(&excluded);

    let tap_uuid = NSUUID::new();
    // SAFETY: plain Objective-C object setup.
    let description = unsafe {
        let d = CATapDescription::initStereoGlobalTapButExcludeProcesses(
            CATapDescription::alloc(),
            &excluded,
        );
        d.setName(&NSString::from_str("Sidekick call audio"));
        d.setUUID(&tap_uuid);
        d.setPrivate(true);
        d.setMuteBehavior(CATapMuteBehavior::Unmuted);
        d
    };
    let mut tap: AudioObjectID = 0;
    // SAFETY: valid description and out-pointer.
    let status = unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut tap) };
    if status != 0 || tap == 0 {
        return Err(format!(
            "Could not capture the other side of the call (error {status}). Sidekick needs \
             macOS 14.2 or later and permission to record system audio: allow it in System \
             Settings > Privacy & Security > Screen & System Audio Recording."
        ));
    }
    let tap_uid = tap_uuid.UUIDString().to_string();

    let uid = format!("com.sidekick.tap.{}", NSUUID::new().UUIDString());
    let sub_tap = dictionary(&[
        (
            kAudioSubTapUIDKey,
            &*NSString::from_str(&tap_uid) as &AnyObject,
        ),
        (kAudioSubTapDriftCompensationKey, &*NSNumber::new_bool(true)),
    ]);
    let description = dictionary(&[
        (
            kAudioAggregateDeviceNameKey,
            &*NSString::from_str("Sidekick call audio") as &AnyObject,
        ),
        (kAudioAggregateDeviceUIDKey, &*NSString::from_str(&uid)),
        (
            kAudioAggregateDeviceIsPrivateKey,
            &*NSNumber::new_bool(true),
        ),
        (
            kAudioAggregateDeviceIsStackedKey,
            &*NSNumber::new_bool(false),
        ),
        (
            kAudioAggregateDeviceTapAutoStartKey,
            &*NSNumber::new_bool(true),
        ),
        (
            kAudioAggregateDeviceTapListKey,
            &*NSArray::from_retained_slice(&[sub_tap]),
        ),
    ]);
    // SAFETY: NSDictionary is toll-free bridged with CFDictionary.
    let cf: &CFDictionary = unsafe { &*(Retained::as_ptr(&description) as *const CFDictionary) };
    let mut aggregate: AudioObjectID = 0;
    // SAFETY: valid dictionary and out-pointer.
    let status = unsafe { AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate)) };
    if status != 0 || aggregate == 0 {
        unsafe { AudioHardwareDestroyProcessTap(tap) };
        return Err(format!(
            "Could not set up system audio capture (error {status})."
        ));
    }

    match Stream::start(aggregate, Channel::Them, true, stop, events) {
        Ok(mut stream) => {
            stream.tap = Some(tap);
            stream.follows_default = Some((kAudioHardwarePropertyDefaultOutputDevice, output));
            Ok(stream)
        }
        Err(e) => {
            unsafe {
                AudioHardwareDestroyAggregateDevice(aggregate);
                AudioHardwareDestroyProcessTap(tap);
            }
            Err(e)
        }
    }
}

fn dictionary(entries: &[(&CStr, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = entries.iter().map(|(k, _)| ns(k)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = entries.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&keys, &values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsamples_48k_to_16k() {
        let mut d = Downsampler::new(48_000.0);
        for _ in 0..48_000 {
            d.push(0.5);
        }
        assert_eq!(d.pending.len(), 16_000);
        assert!(d.pending.iter().all(|&s| (s - 16_383).abs() <= 1));
    }

    #[test]
    fn downsamples_44k1_to_16k() {
        let mut d = Downsampler::new(44_100.0);
        for _ in 0..44_100 {
            d.push(0.0);
        }
        assert!((15_999..=16_001).contains(&d.pending.len()));
    }

    #[test]
    fn upsamples_8k_to_16k() {
        let mut d = Downsampler::new(8_000.0);
        for _ in 0..8_000 {
            d.push(0.0);
        }
        assert_eq!(d.pending.len(), 16_000);
    }

    #[test]
    #[ignore]
    fn tap_only_live() {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = spawn(
            Channel::Them,
            LoopbackTarget::DefaultOutput,
            stop.clone(),
            tx,
        )
        .unwrap();
        thread::sleep(Duration::from_secs(3));
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        let (mut chunks, mut peak) = (0, 0i16);
        for event in rx {
            if let CaptureEvent::Chunk(c) = event {
                chunks += 1;
                peak = peak.max(
                    c.samples
                        .iter()
                        .map(|s| s.saturating_abs())
                        .max()
                        .unwrap_or(0),
                );
            }
        }
        println!("tap chunks: {chunks}, peak level: {peak}");
        assert!(chunks > 0);
    }

    /// Needs real audio hardware and permissions: `cargo test coreaudio_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn coreaudio_live() {
        println!(
            "mics: {:?}",
            list_input_devices().map(|d| d.into_iter().map(|d| d.name).collect::<Vec<_>>())
        );
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let handles = [Channel::Me, Channel::Them]
            .map(|c| spawn(c, LoopbackTarget::DefaultOutput, stop.clone(), tx.clone()).unwrap());
        drop(tx);
        thread::sleep(Duration::from_secs(3));
        println!("using mic: {:?}", processes_using_mic());
        stop.store(true, Ordering::Relaxed);
        handles.into_iter().for_each(|h| h.join().unwrap());
        let (mut me, mut them) = (0, 0);
        for event in rx {
            match event {
                CaptureEvent::Chunk(c) if c.channel == Channel::Me => me += c.samples.len(),
                CaptureEvent::Chunk(c) => them += c.samples.len(),
                CaptureEvent::Failed { channel, message } => panic!("{channel:?}: {message}"),
            }
        }
        println!("me: {me} samples, them: {them} samples");
        assert!(me > 0 && them > 0);
    }
}
