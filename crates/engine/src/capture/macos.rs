//! macOS capture (Core Audio, macOS 14.4+).
//!
//! - Microphone: an IOProc on the chosen input device, or on the default
//!   input, reopened when the default changes or the device goes away.
//! - The call: a Core Audio process tap on everything the Mac plays, read
//!   through a private aggregate device (Apple's "Capturing system audio with
//!   Core Audio taps"). It needs the "System Audio Recording" permission
//!   (NSAudioCaptureUsageDescription), not screen recording. The tap is
//!   independent of the output device; the aggregate is rebuilt when the
//!   default output changes, because it is clocked by that device.

use std::ffi::{c_void, CStr};
use std::mem::{size_of, MaybeUninit};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AllocAnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey, kAudioAggregateDeviceMainSubDeviceKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyStreamFormat,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectSystemObject, kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, AudioDeviceCreateIOProcID,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress, CATapDescription,
};
use objc2_core_audio_types::{kAudioFormatFlagIsFloat, kAudioFormatLinearPCM, AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use super::{sleep_unless, Capture, InputDevice, ThreadCapture};
use crate::error::Error;
use crate::recorder::{Events, RecorderEvent};
use crate::source::Source;

const CHECK_EVERY: Duration = Duration::from_secs(1);
/// No callback for this long means the device stopped delivering.
const STALLED_AFTER: Duration = Duration::from_secs(2);

type OSStatus = i32;

fn system() -> AudioObjectID {
    kAudioObjectSystemObject as AudioObjectID
}

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain }
}

fn check(status: OSStatus, what: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{what} failed (Core Audio error {status})"))
    }
}

/// Read a fixed-size property.
fn get<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Result<T, String> {
    let addr = address(selector, scope);
    let mut size = size_of::<T>() as u32;
    let mut out = MaybeUninit::<T>::uninit();
    // SAFETY: `out` has room for `size` bytes; Core Audio writes at most that.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(out.as_mut_ptr().cast()),
        )
    };
    check(status, "reading an audio property")?;
    // SAFETY: the call succeeded and filled the value.
    Ok(unsafe { out.assume_init() })
}

/// Read a CFString property (name, UID).
fn get_string(object: AudioObjectID, selector: u32, scope: u32) -> Result<String, String> {
    let raw: *const CFString = get(object, selector, scope)?;
    let raw = NonNull::new(raw as *mut CFString).ok_or("empty string property")?;
    // SAFETY: Core Audio returns a +1 CFString for these properties.
    let s = unsafe { CFRetained::from_raw(raw) };
    Ok(s.to_string())
}

/// Read a variable-size property as raw bytes (8-byte aligned).
fn get_bytes(object: AudioObjectID, selector: u32, scope: u32) -> Result<Vec<u64>, String> {
    let addr = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: plain out-parameter.
    check(unsafe { AudioObjectGetPropertyDataSize(object, NonNull::from(&addr), 0, ptr::null(), NonNull::from(&mut size)) }, "reading a property size")?;
    let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
    // SAFETY: `buf` has room for `size` bytes.
    check(
        unsafe {
            AudioObjectGetPropertyData(object, NonNull::from(&addr), 0, ptr::null(), NonNull::from(&mut size), NonNull::new_unchecked(buf.as_mut_ptr().cast()))
        },
        "reading a property",
    )?;
    buf.truncate((size as usize).div_ceil(8));
    Ok(buf)
}

fn devices() -> Vec<AudioObjectID> {
    let Ok(buf) = get_bytes(system(), kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal) else { return Vec::new() };
    // SAFETY: the property is an array of AudioObjectID (u32).
    let ids = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<AudioObjectID>(), buf.len() * 2) };
    ids.iter().copied().filter(|id| *id != 0).collect()
}

fn input_channels(device: AudioObjectID) -> u32 {
    let Ok(buf) = get_bytes(device, kAudioDevicePropertyStreamConfiguration, kAudioObjectPropertyScopeInput) else { return 0 };
    // SAFETY: the property is an AudioBufferList; `buf` is aligned and sized by Core Audio.
    unsafe {
        let list = &*(buf.as_ptr().cast::<AudioBufferList>());
        let n = list.mNumberBuffers as usize;
        if n == 0 || size_of::<u32>() * 2 + n * size_of::<AudioBuffer>() > buf.len() * 8 {
            return 0;
        }
        std::slice::from_raw_parts(list.mBuffers.as_ptr(), n).iter().map(|b| b.mNumberChannels).sum()
    }
}

fn device_uid(device: AudioObjectID) -> Result<String, String> {
    get_string(device, kAudioDevicePropertyDeviceUID, kAudioObjectPropertyScopeGlobal)
}

fn device_name(device: AudioObjectID) -> String {
    get_string(device, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal).unwrap_or_else(|_| "Audio device".into())
}

fn default_device(selector: u32) -> Option<AudioObjectID> {
    get::<AudioObjectID>(system(), selector, kAudioObjectPropertyScopeGlobal).ok().filter(|id| *id != 0)
}

fn is_alive(device: AudioObjectID) -> bool {
    get::<u32>(device, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal).map(|v| v != 0).unwrap_or(false)
}

/// Microphone permission: the system prompt appears on first use of any
/// input, but asking explicitly makes sure it shows before recording starts
/// and tells us the answer.
pub fn request_microphone_access(timeout: Duration) -> Result<bool, Error> {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    // SAFETY: reading an AVFoundation constant.
    let Some(media) = (unsafe { AVMediaTypeAudio }) else { return Ok(true) };
    // SAFETY: documented class method, valid media type.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    if status == AVAuthorizationStatus::Authorized {
        return Ok(true);
    }
    if status == AVAuthorizationStatus::Denied || status == AVAuthorizationStatus::Restricted {
        return Ok(false);
    }
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let handler = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = tx.send(granted.as_bool());
    });
    // SAFETY: the handler is copied by AVFoundation and called once.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler) };
    Ok(rx.recv_timeout(timeout).unwrap_or(false))
}

pub fn input_devices() -> Result<Vec<InputDevice>, Error> {
    let default = default_device(kAudioHardwarePropertyDefaultInputDevice);
    Ok(devices()
        .into_iter()
        .filter(|d| input_channels(*d) > 0)
        .filter_map(|d| {
            let id = device_uid(d).ok()?;
            Some(InputDevice { id, name: device_name(d), is_default: Some(d) == default })
        })
        .collect())
}

// ---- IOProc ----------------------------------------------------------------

struct IoContext {
    source: Arc<Source>,
    rate: u32,
    /// Aggregate devices list their sub-devices' inputs first; the tap's
    /// stream is the last buffer.
    last_buffer_only: bool,
    mono: Mutex<Vec<f32>>,
    callbacks: AtomicU64,
}

unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> OSStatus {
    // SAFETY: `client` is the IoContext registered with this IOProc; it outlives it.
    let ctx = unsafe { &*(client as *const IoContext) };
    ctx.callbacks.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Core Audio passes a valid AudioBufferList with mNumberBuffers entries.
    let buffers = unsafe {
        let list = input.as_ref();
        std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize)
    };
    let buffers = match (ctx.last_buffer_only, buffers.len()) {
        (_, 0) => return 0,
        (true, n) => &buffers[n - 1..],
        (false, _) => buffers,
    };
    let frames = buffers
        .iter()
        .filter(|b| b.mNumberChannels > 0 && !b.mData.is_null())
        .map(|b| b.mDataByteSize as usize / (4 * b.mNumberChannels as usize))
        .min()
        .unwrap_or(0);
    let channels: usize = buffers.iter().filter(|b| !b.mData.is_null()).map(|b| b.mNumberChannels as usize).sum();
    if frames == 0 || channels == 0 {
        return 0;
    }
    let Ok(mut mono) = ctx.mono.try_lock() else { return 0 };
    mono.clear();
    mono.resize(frames, 0.0);
    for b in buffers.iter().filter(|b| !b.mData.is_null()) {
        let ch = b.mNumberChannels as usize;
        // SAFETY: float32 samples, `frames * ch` of them fit in mDataByteSize.
        let data = unsafe { std::slice::from_raw_parts(b.mData as *const f32, frames * ch) };
        for (f, out) in mono.iter_mut().enumerate() {
            for c in 0..ch {
                *out += data[f * ch + c];
            }
        }
    }
    let scale = 1.0 / channels as f32;
    mono.iter_mut().for_each(|x| *x *= scale);
    ctx.source.push(&mono, ctx.rate);
    0
}

/// An IOProc running on a device until dropped.
struct IoRun {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    ctx: *mut IoContext,
}

impl IoRun {
    fn start(device: AudioObjectID, ctx: IoContext) -> Result<IoRun, String> {
        let ctx = Box::into_raw(Box::new(ctx));
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: `ctx` stays valid until the IOProc is destroyed in Drop.
        let status = unsafe { AudioDeviceCreateIOProcID(device, Some(io_proc), ctx.cast(), NonNull::from(&mut proc_id)) };
        if let Err(e) = check(status, "creating the audio callback") {
            // SAFETY: never registered.
            drop(unsafe { Box::from_raw(ctx) });
            return Err(e);
        }
        let run = IoRun { device, proc_id, ctx };
        // SAFETY: valid device and IOProc.
        check(unsafe { AudioDeviceStart(device, proc_id) }, "starting the audio device")?;
        Ok(run)
    }

    fn callbacks(&self) -> u64 {
        // SAFETY: alive until Drop.
        unsafe { (*self.ctx).callbacks.load(Ordering::Relaxed) }
    }
}

impl Drop for IoRun {
    fn drop(&mut self) {
        // SAFETY: stopping and destroying our own IOProc; after that Core Audio
        // no longer calls it, so the context can be freed.
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            drop(Box::from_raw(self.ctx));
        }
    }
}

/// Watches an IoRun's callback count to notice a device that went quiet.
struct StallWatch {
    last_count: u64,
    last_change: Instant,
}

impl StallWatch {
    fn new() -> Self {
        StallWatch { last_count: 0, last_change: Instant::now() }
    }

    fn stalled(&mut self, count: u64) -> bool {
        if count != self.last_count {
            self.last_count = count;
            self.last_change = Instant::now();
        }
        self.last_change.elapsed() > STALLED_AFTER
    }
}

// ---- microphone ------------------------------------------------------------

pub fn start_mic(device: Option<String>, source: Arc<Source>, events: Events) -> Result<Box<dyn Capture>, Error> {
    let cap = ThreadCapture::spawn("notizli-mic", Duration::from_secs(5), move |stop, ready| mic_thread(device, source, &events, stop, ready))
        .map_err(Error::Microphone)?;
    Ok(Box::new(cap))
}

/// The chosen device if present, else the default input; and whether the
/// default is being followed.
fn pick_mic(chosen: Option<&str>) -> Result<(AudioObjectID, bool), String> {
    if let Some(uid) = chosen {
        if let Some(d) = devices().into_iter().find(|d| device_uid(*d).ok().as_deref() == Some(uid)) {
            if is_alive(d) {
                return Ok((d, false));
            }
        }
    }
    let d = default_device(kAudioHardwarePropertyDefaultInputDevice).ok_or("no microphone is connected")?;
    Ok((d, true))
}

fn open_mic(device: AudioObjectID, source: &Arc<Source>) -> Result<IoRun, String> {
    let format: AudioStreamBasicDescription = get(device, kAudioDevicePropertyStreamFormat, kAudioObjectPropertyScopeInput)?;
    if format.mFormatID != kAudioFormatLinearPCM || format.mFormatFlags & kAudioFormatFlagIsFloat == 0 || format.mBitsPerChannel != 32 {
        return Err("the microphone uses an unsupported sample format".into());
    }
    let rate: f64 = get(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal).unwrap_or(format.mSampleRate);
    IoRun::start(
        device,
        IoContext { source: source.clone(), rate: rate.round() as u32, last_buffer_only: false, mono: Mutex::new(Vec::new()), callbacks: AtomicU64::new(0) },
    )
}

fn mic_thread(chosen: Option<String>, source: Arc<Source>, events: &Events, stop: &AtomicBool, ready: &dyn Fn(Result<(), String>)) {
    let mut started = false;
    while !stop.load(Ordering::SeqCst) {
        let opened = pick_mic(chosen.as_deref()).and_then(|(d, follows)| {
            let name = device_name(d);
            source.opened(&name);
            open_mic(d, &source).map(|run| (d, follows, name, run))
        });
        let (device, follows_default, name, run) = match opened {
            Ok(x) => x,
            Err(e) => {
                if !started {
                    ready(Err(e));
                    return;
                }
                sleep_unless(stop, Duration::from_secs(1));
                continue;
            }
        };
        events(RecorderEvent::Microphone { label: name.clone() });
        if started {
            events(RecorderEvent::Notice { text: format!("Microphone: {name}") });
        } else {
            started = true;
            ready(Ok(()));
        }
        let mut watch = StallWatch::new();
        while !stop.load(Ordering::SeqCst) {
            sleep_unless(stop, CHECK_EVERY);
            let moved_default = follows_default && default_device(kAudioHardwarePropertyDefaultInputDevice).is_some_and(|d| d != device);
            let chosen_back = follows_default && chosen.is_some() && pick_mic(chosen.as_deref()).map(|(_, f)| !f).unwrap_or(false);
            if moved_default || chosen_back || !is_alive(device) || watch.stalled(run.callbacks()) {
                log::info!("reopening the microphone");
                break;
            }
        }
        drop(run);
    }
}

// ---- the call --------------------------------------------------------------

pub fn start_other_side(source: Arc<Source>, events: Events) -> Result<Box<dyn Capture>, Error> {
    let cap = ThreadCapture::spawn("notizli-call", Duration::from_secs(5), move |stop, ready| call_thread(source, &events, stop, ready))
        .map_err(Error::OtherSide)?;
    Ok(Box::new(cap))
}

/// A process tap on all system audio, read through a private aggregate device.
struct Tap {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    run: Option<IoRun>,
    output: AudioObjectID,
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.run = None;
        // SAFETY: destroying the objects this Tap created.
        unsafe {
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

fn ns(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().unwrap_or_default())
}

fn any<T: objc2::Message>(obj: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(obj) }
}

fn dict(pairs: Vec<(&CStr, Retained<AnyObject>)>) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = pairs.iter().map(|(k, _)| ns(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<Retained<AnyObject>> = pairs.into_iter().map(|(_, v)| v).collect();
    NSDictionary::from_retained_objects(&key_refs, &values)
}

fn create_tap(source: &Arc<Source>) -> Result<Tap, String> {
    let output = default_device(kAudioHardwarePropertyDefaultOutputDevice).ok_or("no speaker or headphones are connected")?;
    let output_uid = device_uid(output)?;

    // SAFETY: plain Objective-C object construction and property setters.
    let (tap, tap_uid) = unsafe {
        let exclude: Retained<NSArray<NSNumber>> = NSArray::new();
        let desc = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &exclude);
        desc.setName(&NSString::from_str("Notizli meeting audio"));
        desc.setPrivate(true);
        let mut tap: AudioObjectID = 0;
        check(AudioHardwareCreateProcessTap(Some(&desc), &mut tap), "creating the system audio tap")?;
        (tap, desc.UUID().UUIDString().to_string())
    };

    let description = dict(vec![
        (kAudioAggregateDeviceNameKey, any(NSString::from_str("Notizli meeting audio"))),
        (kAudioAggregateDeviceUIDKey, any(NSUUID::UUID().UUIDString())),
        (kAudioAggregateDeviceMainSubDeviceKey, any(NSString::from_str(&output_uid))),
        (kAudioAggregateDeviceIsPrivateKey, any(NSNumber::numberWithBool(true))),
        (kAudioAggregateDeviceIsStackedKey, any(NSNumber::numberWithBool(false))),
        (kAudioAggregateDeviceTapAutoStartKey, any(NSNumber::numberWithBool(true))),
        (
            kAudioAggregateDeviceSubDeviceListKey,
            any(NSArray::from_retained_slice(&[dict(vec![(kAudioSubDeviceUIDKey, any(NSString::from_str(&output_uid)))])])),
        ),
        (
            kAudioAggregateDeviceTapListKey,
            any(NSArray::from_retained_slice(&[dict(vec![
                (kAudioSubTapUIDKey, any(NSString::from_str(&tap_uid))),
                (kAudioSubTapDriftCompensationKey, any(NSNumber::numberWithBool(true))),
            ])])),
        ),
    ]);
    // SAFETY: NSDictionary is toll-free bridged to CFDictionary.
    let cf: &CFDictionary = unsafe { &*(Retained::as_ptr(&description) as *const CFDictionary) };
    let mut aggregate: AudioObjectID = 0;
    // SAFETY: valid dictionary and out-parameter.
    if let Err(e) = check(unsafe { AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate)) }, "creating the capture device") {
        // SAFETY: destroying the tap created above.
        unsafe { AudioHardwareDestroyProcessTap(tap) };
        return Err(e);
    }
    let mut t = Tap { tap, aggregate, run: None, output };
    let rate: f64 = get(aggregate, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal).unwrap_or(48_000.0);
    t.run = Some(IoRun::start(
        aggregate,
        IoContext { source: source.clone(), rate: rate.round() as u32, last_buffer_only: true, mono: Mutex::new(Vec::new()), callbacks: AtomicU64::new(0) },
    )?);
    Ok(t)
}

fn call_thread(source: Arc<Source>, events: &Events, stop: &AtomicBool, ready: &dyn Fn(Result<(), String>)) {
    let label = "All sound on this Mac";
    let mut started = false;
    while !stop.load(Ordering::SeqCst) {
        source.opened(label);
        let tap = match create_tap(&source) {
            Ok(t) => t,
            Err(e) => {
                if !started {
                    ready(Err(e));
                    return;
                }
                log::warn!("system audio tap: {e}");
                sleep_unless(stop, Duration::from_secs(1));
                continue;
            }
        };
        if !started {
            started = true;
            events(RecorderEvent::Hearing { label: label.into() });
            ready(Ok(()));
        }
        let mut watch = StallWatch::new();
        while !stop.load(Ordering::SeqCst) {
            sleep_unless(stop, CHECK_EVERY);
            let output_changed = default_device(kAudioHardwarePropertyDefaultOutputDevice).is_some_and(|d| d != tap.output);
            let stalled = tap.run.as_ref().is_some_and(|r| watch.stalled(r.callbacks()));
            if output_changed || stalled {
                log::info!("rebuilding the system audio tap");
                break;
            }
        }
        drop(tap);
    }
}
