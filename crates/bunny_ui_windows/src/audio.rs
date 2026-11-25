//! Windows speakers: [`bunny_ui::audio`] played through WASAPI.
//!
//! One shared-mode, event-driven `IAudioClient` per stream on the default
//! render endpoint. It is opened with `AUTOCONVERTPCM`, so the audio
//! engine takes the app's rate and channel count as they are and converts
//! to the mix format itself. A thread of its own waits for the engine's
//! event and fills what the endpoint buffer has free from
//! [`Stream::render`].
//!
//! The latency the clock subtracts is the frames still queued in the
//! endpoint buffer plus the stream latency the engine reports. An
//! endpoint that goes away (`AUDCLNT_E_DEVICE_INVALIDATED`) reports
//! [`DeviceEvent::Lost`]; the thread then opens the default endpoint
//! again every half second and reports [`DeviceEvent::Changed`] once it
//! plays again.
//!
//! ```ignore
//! let mut output = AudioOutput::<Wasapi>::open(AudioFormat::new(48_000, 2)?)?;
//! output.push(&samples);
//! ```

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use bunny_ui::audio::{AudioError, AudioFormat, DeviceEvent, Stream};

use crate::ffi::{Guid, Hresult, UnknownVtbl};

type Handle = *mut c_void;
/// A kernel object, as the shell's other declarations of the same calls spell it.
type Event = isize;

const COINIT_MULTITHREADED: u32 = 0;
const CLSCTX_ALL: u32 = 0x17;
const E_RENDER: u32 = 0;
const E_CONSOLE: u32 = 0;
const SHARED: u32 = 0;
const EVENT_CALLBACK: u32 = 0x0004_0000;
const AUTOCONVERT_PCM: u32 = 0x8000_0000;
const SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const WAIT_OBJECT_0: u32 = 0;
const HNS_PER_SECOND: i64 = 10_000_000;

const BUFFER: Duration = Duration::from_millis(40);
const WAIT_MS: u32 = 200;
const RETRY: Duration = Duration::from_millis(500);

const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid {
    d1: 0xBCDE_0395,
    d2: 0xE52F,
    d3: 0x467C,
    d4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
};
const IID_IMM_DEVICE_ENUMERATOR: Guid = Guid {
    d1: 0xA956_64D2,
    d2: 0x9614,
    d3: 0x4F35,
    d4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
};
const IID_IAUDIO_CLIENT: Guid = Guid {
    d1: 0x1CB9_AD4C,
    d2: 0xDBFA,
    d3: 0x4C32,
    d4: [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2],
};
const IID_IAUDIO_RENDER_CLIENT: Guid = Guid {
    d1: 0xF294_ACFC,
    d2: 0x3146,
    d3: 0x4483,
    d4: [0xA7, 0xBF, 0xAD, 0xDC, 0xA7, 0xC2, 0x60, 0xE2],
};
const SUBTYPE_IEEE_FLOAT: Guid = Guid {
    d1: 0x0000_0003,
    d2: 0x0000,
    d3: 0x0010,
    d4: [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
};

#[repr(C)]
struct WaveFormat {
    tag: u16,
    channels: u16,
    rate: u32,
    bytes_per_second: u32,
    block_align: u16,
    bits: u16,
    extra: u16,
    valid_bits: u16,
    channel_mask: u32,
    sub_format: Guid,
}

#[repr(C)]
struct EnumeratorVtbl {
    unknown: UnknownVtbl,
    enum_endpoints: usize,
    default_endpoint: unsafe extern "system" fn(Handle, u32, u32, *mut Handle) -> Hresult,
}

#[repr(C)]
struct DeviceVtbl {
    unknown: UnknownVtbl,
    activate:
        unsafe extern "system" fn(Handle, *const Guid, u32, *const c_void, *mut Handle) -> Hresult,
}

#[repr(C)]
struct ClientVtbl {
    unknown: UnknownVtbl,
    initialize: unsafe extern "system" fn(
        Handle,
        u32,
        u32,
        i64,
        i64,
        *const WaveFormat,
        *const Guid,
    ) -> Hresult,
    buffer_size: unsafe extern "system" fn(Handle, *mut u32) -> Hresult,
    stream_latency: unsafe extern "system" fn(Handle, *mut i64) -> Hresult,
    padding: unsafe extern "system" fn(Handle, *mut u32) -> Hresult,
    is_format_supported: usize,
    mix_format: unsafe extern "system" fn(Handle, *mut *mut WaveFormat) -> Hresult,
    device_period: usize,
    start: unsafe extern "system" fn(Handle) -> Hresult,
    stop: unsafe extern "system" fn(Handle) -> Hresult,
    reset: usize,
    set_event: unsafe extern "system" fn(Handle, Event) -> Hresult,
    service: unsafe extern "system" fn(Handle, *const Guid, *mut Handle) -> Hresult,
}

#[repr(C)]
struct RenderVtbl {
    unknown: UnknownVtbl,
    buffer: unsafe extern "system" fn(Handle, u32, *mut *mut u8) -> Hresult,
    release_buffer: unsafe extern "system" fn(Handle, u32, u32) -> Hresult,
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *const c_void, model: u32) -> Hresult;
    fn CoUninitialize();
    fn CoCreateInstance(
        clsid: *const Guid,
        aggregate: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut Handle,
    ) -> Hresult;
    fn CoTaskMemFree(block: *mut c_void);
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateEventW(
        attributes: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Event;
    fn WaitForSingleObject(handle: Event, millis: u32) -> u32;
    fn CloseHandle(handle: Event) -> i32;
}

fn check(status: Hresult) -> Result<(), AudioError> {
    if status < 0 {
        Err(AudioError::Native(status))
    } else {
        Ok(())
    }
}

/// The first slot of every COM object is its vtable.
unsafe fn vtbl<T>(object: Handle) -> &'static T {
    unsafe { &**object.cast::<*const T>() }
}

/// One owned interface pointer, released when dropped.
struct Object(Handle);

impl Drop for Object {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (vtbl::<UnknownVtbl>(self.0).release)(self.0) };
        }
    }
}

/// The speaker layout WASAPI assumes for a channel count.
const fn channel_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        3 => 0x7,
        4 => 0x33,
        5 => 0x37,
        6 => 0x3F,
        7 => 0x13F,
        8 => 0x63F,
        _ => 0,
    }
}

fn wave_format(format: AudioFormat) -> WaveFormat {
    let channels = format.channels();
    let block_align = channels * 4;
    WaveFormat {
        tag: WAVE_FORMAT_EXTENSIBLE,
        channels,
        rate: format.rate(),
        bytes_per_second: format.rate() * u32::from(block_align),
        block_align,
        bits: 32,
        extra: 22,
        valid_bits: 32,
        channel_mask: channel_mask(channels),
        sub_format: SUBTYPE_IEEE_FLOAT,
    }
}

/// Joins the multithreaded apartment for the life of a thread.
struct Apartment;

impl Apartment {
    fn join() -> Result<Apartment, AudioError> {
        check(unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED) })?;
        Ok(Apartment)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn default_endpoint() -> Result<Object, AudioError> {
    let mut enumerator = std::ptr::null_mut();
    check(unsafe {
        CoCreateInstance(
            &CLSID_MM_DEVICE_ENUMERATOR,
            std::ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IMM_DEVICE_ENUMERATOR,
            &mut enumerator,
        )
    })?;
    let enumerator = Object(enumerator);
    let mut device = std::ptr::null_mut();
    let status = unsafe {
        (vtbl::<EnumeratorVtbl>(enumerator.0).default_endpoint)(
            enumerator.0,
            E_RENDER,
            E_CONSOLE,
            &mut device,
        )
    };
    if status < 0 || device.is_null() {
        return Err(AudioError::Unavailable);
    }
    Ok(Object(device))
}

fn activate(device: &Object) -> Result<Object, AudioError> {
    let mut client = std::ptr::null_mut();
    check(unsafe {
        (vtbl::<DeviceVtbl>(device.0).activate)(
            device.0,
            &IID_IAUDIO_CLIENT,
            CLSCTX_ALL,
            std::ptr::null(),
            &mut client,
        )
    })?;
    Ok(Object(client))
}

/// One started client on the default endpoint.
struct Endpoint {
    client: Object,
    render: Object,
    event: Event,
    frames: u32,
    latency: u64,
}

impl Endpoint {
    fn open(format: AudioFormat) -> Result<Endpoint, AudioError> {
        let client = activate(&default_endpoint()?)?;
        let calls = unsafe { vtbl::<ClientVtbl>(client.0) };
        let wire = wave_format(format);
        let duration = i64::try_from(BUFFER.as_nanos() / 100).unwrap_or(i64::MAX);
        check(unsafe {
            (calls.initialize)(
                client.0,
                SHARED,
                EVENT_CALLBACK | AUTOCONVERT_PCM | SRC_DEFAULT_QUALITY,
                duration,
                0,
                &wire,
                std::ptr::null(),
            )
        })?;
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if event == 0 {
            return Err(AudioError::Unavailable);
        }
        let mut endpoint = Endpoint {
            client,
            render: Object(std::ptr::null_mut()),
            event,
            frames: 0,
            latency: 0,
        };
        let mut hns = 0;
        unsafe {
            let client = endpoint.client.0;
            check((calls.set_event)(client, event))?;
            check((calls.buffer_size)(client, &mut endpoint.frames))?;
            check((calls.stream_latency)(client, &mut hns))?;
            check((calls.service)(
                client,
                &IID_IAUDIO_RENDER_CLIENT,
                &mut endpoint.render.0,
            ))?;
        }
        endpoint.latency = u64::try_from(hns).unwrap_or(0) * u64::from(format.rate())
            / HNS_PER_SECOND.unsigned_abs();
        check(unsafe { (calls.start)(endpoint.client.0) })?;
        Ok(endpoint)
    }

    /// Waits for the engine and fills what its buffer has free.
    fn fill(&self, stream: &Stream) -> Result<(), AudioError> {
        if unsafe { WaitForSingleObject(self.event, WAIT_MS) } != WAIT_OBJECT_0 {
            return Ok(());
        }
        let calls = unsafe { vtbl::<ClientVtbl>(self.client.0) };
        let mut queued = 0;
        check(unsafe { (calls.padding)(self.client.0, &mut queued) })?;
        let free = self.frames.saturating_sub(queued);
        if free == 0 {
            return Ok(());
        }
        let render = unsafe { vtbl::<RenderVtbl>(self.render.0) };
        let mut data = std::ptr::null_mut();
        check(unsafe { (render.buffer)(self.render.0, free, &mut data) })?;
        let channels = usize::from(stream.format().channels());
        let samples =
            unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), free as usize * channels) };
        stream.render(samples);
        check(unsafe { (render.release_buffer)(self.render.0, free, 0) })?;
        stream.set_latency(u64::from(queued) + self.latency);
        Ok(())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        unsafe {
            (vtbl::<ClientVtbl>(self.client.0).stop)(self.client.0);
            CloseHandle(self.event);
        }
    }
}

/// The WASAPI [`bunny_ui::audio::Device`]: a thread that plays the
/// stream on the default endpoint, stopped when dropped.
pub struct Wasapi {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn play(stream: &Stream, endpoint: &mut Option<Endpoint>, alive: &AtomicBool) {
    while alive.load(Ordering::Acquire) {
        let Some(open) = endpoint.as_ref() else {
            std::thread::park_timeout(RETRY);
            if let Ok(again) = Endpoint::open(stream.format()) {
                *endpoint = Some(again);
                stream.notify(DeviceEvent::Changed);
            }
            continue;
        };
        if open.fill(stream).is_err() {
            *endpoint = None;
            stream.notify(DeviceEvent::Lost);
        }
    }
}

impl bunny_ui::audio::Device for Wasapi {
    fn open(stream: Arc<Stream>) -> Result<Wasapi, AudioError> {
        let running = Arc::new(AtomicBool::new(true));
        let alive = Arc::clone(&running);
        let (opened, outcome) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("bunny-audio".into())
            .spawn(move || {
                let started = Apartment::join().and_then(|apartment| {
                    Endpoint::open(stream.format()).map(|endpoint| (apartment, endpoint))
                });
                let (_apartment, endpoint) = match started {
                    Ok(started) => started,
                    Err(error) => {
                        let _ = opened.send(Err(error));
                        return;
                    }
                };
                let _ = opened.send(Ok(()));
                play(&stream, &mut Some(endpoint), &alive);
            })
            .map_err(|_| AudioError::Unavailable)?;
        match outcome.recv() {
            Ok(Ok(())) => Ok(Wasapi {
                running,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(AudioError::Unavailable)
            }
        }
    }

    /// The default endpoint's mix rate, asked on a thread of its own so
    /// the caller's apartment stays as it was.
    fn preferred_rate() -> Option<u32> {
        std::thread::spawn(|| {
            let _apartment = Apartment::join().ok()?;
            let client = activate(&default_endpoint().ok()?).ok()?;
            let mut mix = std::ptr::null_mut();
            let status = unsafe { (vtbl::<ClientVtbl>(client.0).mix_format)(client.0, &mut mix) };
            if status < 0 || mix.is_null() {
                return None;
            }
            let rate = unsafe { (*mix).rate };
            unsafe { CoTaskMemFree(mix.cast()) };
            Some(rate)
        })
        .join()
        .ok()
        .flatten()
    }
}

impl Drop for Wasapi {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wave_format_is_the_extensible_layout_wasapi_reads() {
        assert_eq!(size_of::<WaveFormat>(), 40);
        let wire = wave_format(AudioFormat::new(44_100, 2).expect("format"));
        assert_eq!((wire.block_align, wire.bytes_per_second), (8, 352_800));
        assert_eq!(wire.channel_mask, 0x3);
    }

    #[test]
    fn a_running_client_plays_the_queue_and_moves_the_clock() {
        type Output = bunny_ui::audio::AudioOutput<Wasapi>;
        let Ok(mut output) = Output::open(AudioFormat::new(48_000, 2).expect("format")) else {
            eprintln!("skipped: no audio endpoint on this machine");
            return;
        };
        output.push(&vec![0.0; 2 * 24_000]);
        let start = std::time::Instant::now();
        while output.frames_played() < 4_800 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(output.frames_played() >= 4_800);
    }
}
