//! The mac's speakers: [`bunny_ui::audio`] rendered through CoreAudio.
//!
//! One `DefaultOutput` audio unit per stream. It follows the system's
//! output device by itself — headphones in, headphones out — and its own
//! converter takes the app's rate and channel count to whatever the device
//! runs at, so the app pushes in the format it decoded. The unit pulls on
//! CoreAudio's real-time thread, straight into [`Stream::render`]: no lock,
//! no allocation, no message to the main thread.
//!
//! The latency the clock subtracts is the device's own (its latency, its
//! safety offset and one buffer) plus the unit's, measured when the stream
//! opens and again whenever the default device changes. A system left with
//! no output device reports [`DeviceEvent::Lost`].
//!
//! ```ignore
//! let mut output = AudioOutput::<CoreAudio>::open(AudioFormat::new(48_000, 2)?)?;
//! output.push(&samples);
//! ```

use std::ffi::c_void;
use std::sync::Arc;

use bunny_ui::audio::{AudioError, AudioFormat, DeviceEvent, Stream};

type OsStatus = i32;
type AudioUnit = *mut c_void;
type AudioComponent = *mut c_void;
type AudioObjectId = u32;

type RenderProc = extern "C" fn(
    refcon: *mut c_void,
    flags: *mut u32,
    timestamp: *const c_void,
    bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> OsStatus;

type ListenerProc = extern "C" fn(
    object: AudioObjectId,
    count: u32,
    addresses: *const PropertyAddress,
    client: *mut c_void,
) -> OsStatus;

#[repr(C)]
struct ComponentDescription {
    kind: u32,
    sub_kind: u32,
    manufacturer: u32,
    flags: u32,
    flags_mask: u32,
}

#[repr(C)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct RenderCallback {
    proc_: RenderProc,
    refcon: *mut c_void,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        after: AudioComponent,
        description: *const ComponentDescription,
    ) -> AudioComponent;
    fn AudioComponentInstanceNew(component: AudioComponent, unit: *mut AudioUnit) -> OsStatus;
    fn AudioComponentInstanceDispose(unit: AudioUnit) -> OsStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OsStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OsStatus;
    fn AudioUnitSetProperty(
        unit: AudioUnit,
        property: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> OsStatus;
    fn AudioUnitGetProperty(
        unit: AudioUnit,
        property: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> OsStatus;
    fn AudioOutputUnitStart(unit: AudioUnit) -> OsStatus;
    fn AudioOutputUnitStop(unit: AudioUnit) -> OsStatus;
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectAddPropertyListener(
        object: AudioObjectId,
        address: *const PropertyAddress,
        listener: ListenerProc,
        client: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectRemovePropertyListener(
        object: AudioObjectId,
        address: *const PropertyAddress,
        listener: ListenerProc,
        client: *mut c_void,
    ) -> OsStatus;
}

const fn code(name: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*name)
}

const OUTPUT_UNIT: u32 = code(b"auou");
const DEFAULT_OUTPUT: u32 = code(b"def ");
const APPLE: u32 = code(b"appl");
const LINEAR_PCM: u32 = code(b"lpcm");
const FLOAT_PACKED: u32 = 1 | 8;

const STREAM_FORMAT: u32 = 8;
const UNIT_LATENCY: u32 = 12;
const SET_RENDER_CALLBACK: u32 = 23;
const SCOPE_GLOBAL: u32 = 0;
const SCOPE_INPUT: u32 = 1;

const SYSTEM_OBJECT: AudioObjectId = 1;
const UNKNOWN_OBJECT: AudioObjectId = 0;
const DEFAULT_OUTPUT_DEVICE: u32 = code(b"dOut");
const NOMINAL_RATE: u32 = code(b"nsrt");
const DEVICE_LATENCY: u32 = code(b"ltnc");
const SAFETY_OFFSET: u32 = code(b"saft");
const BUFFER_FRAMES: u32 = code(b"fsiz");
const OBJECT_GLOBAL: u32 = code(b"glob");
const OBJECT_OUTPUT: u32 = code(b"outp");
const ELEMENT_MAIN: u32 = 0;

const DEFAULT_DEVICE_ADDRESS: PropertyAddress = PropertyAddress {
    selector: DEFAULT_OUTPUT_DEVICE,
    scope: OBJECT_GLOBAL,
    element: ELEMENT_MAIN,
};

fn check(status: OsStatus) -> Result<(), AudioError> {
    if status == 0 {
        Ok(())
    } else {
        Err(AudioError::Native(status))
    }
}

fn property<T: Copy>(object: AudioObjectId, selector: u32, scope: u32, empty: T) -> Option<T> {
    let address = PropertyAddress {
        selector,
        scope,
        element: ELEMENT_MAIN,
    };
    let mut value = empty;
    let mut size = size_of::<T>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    (status == 0).then_some(value)
}

fn default_device() -> Option<AudioObjectId> {
    property(
        SYSTEM_OBJECT,
        DEFAULT_OUTPUT_DEVICE,
        OBJECT_GLOBAL,
        UNKNOWN_OBJECT,
    )
    .filter(|&device| device != UNKNOWN_OBJECT)
}

/// The device's delay and the unit's, in frames of `format`.
fn latency_frames(unit: AudioUnit, format: AudioFormat) -> u64 {
    let Some(device) = default_device() else {
        return 0;
    };
    let rate = property(device, NOMINAL_RATE, OBJECT_GLOBAL, 0.0f64).filter(|rate| *rate > 0.0);
    let device_seconds = rate.map_or(0.0, |rate| {
        let frames = [DEVICE_LATENCY, SAFETY_OFFSET]
            .into_iter()
            .map(|selector| property(device, selector, OBJECT_OUTPUT, 0u32).unwrap_or(0))
            .chain([property(device, BUFFER_FRAMES, OBJECT_GLOBAL, 0u32).unwrap_or(0)])
            .map(f64::from)
            .sum::<f64>();
        frames / rate
    });
    let mut unit_seconds = 0.0f64;
    let mut size = size_of::<f64>() as u32;
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            UNIT_LATENCY,
            SCOPE_GLOBAL,
            0,
            (&raw mut unit_seconds).cast(),
            &mut size,
        )
    };
    if status != 0 {
        unit_seconds = 0.0;
    }
    ((device_seconds + unit_seconds) * f64::from(format.rate())).round() as u64
}

/// What the HAL's threads reach: the stream, and the unit to measure.
struct Shared {
    stream: Arc<Stream>,
    unit: AudioUnit,
}

extern "C" fn render(
    refcon: *mut c_void,
    _flags: *mut u32,
    _timestamp: *const c_void,
    _bus: u32,
    _frames: u32,
    data: *mut AudioBufferList,
) -> OsStatus {
    // SAFETY: `refcon` is the `Shared` the `CoreAudio` boxes and keeps
    // until the unit is disposed, and the unit hands one interleaved
    // buffer of the float format it was given.
    unsafe {
        let shared = &*refcon.cast::<Shared>();
        let buffer = &(*data).buffers[0];
        if buffer.data.is_null() {
            return 0;
        }
        let samples =
            std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), buffer.bytes as usize / 4);
        shared.stream.render(samples);
    }
    0
}

extern "C" fn default_device_changed(
    _object: AudioObjectId,
    _count: u32,
    _addresses: *const PropertyAddress,
    client: *mut c_void,
) -> OsStatus {
    // SAFETY: the listener is removed before the `Shared` is freed.
    let shared = unsafe { &*client.cast::<Shared>() };
    if default_device().is_some() {
        shared
            .stream
            .set_latency(latency_frames(shared.unit, shared.stream.format()));
        shared.stream.notify(DeviceEvent::Changed);
    } else {
        shared.stream.notify(DeviceEvent::Lost);
    }
    0
}

/// The CoreAudio [`bunny_ui::audio::Device`]: a running default-output
/// unit, stopped and disposed when dropped.
pub struct CoreAudio {
    shared: *mut Shared,
}

// SAFETY: the unit's start, stop and dispose may be called from any
// thread, and `Shared` holds only the stream, which is `Sync`.
unsafe impl Send for CoreAudio {}

impl CoreAudio {
    fn build(stream: Arc<Stream>) -> Result<CoreAudio, AudioError> {
        let description = ComponentDescription {
            kind: OUTPUT_UNIT,
            sub_kind: DEFAULT_OUTPUT,
            manufacturer: APPLE,
            flags: 0,
            flags_mask: 0,
        };
        let component = unsafe { AudioComponentFindNext(std::ptr::null_mut(), &description) };
        if component.is_null() {
            return Err(AudioError::Unavailable);
        }
        let mut unit: AudioUnit = std::ptr::null_mut();
        check(unsafe { AudioComponentInstanceNew(component, &mut unit) })?;
        let format = stream.format();
        let shared = Box::into_raw(Box::new(Shared { stream, unit }));
        let device = CoreAudio { shared };
        let channels = u32::from(format.channels());
        let wire = StreamDescription {
            sample_rate: f64::from(format.rate()),
            format_id: LINEAR_PCM,
            format_flags: FLOAT_PACKED,
            bytes_per_packet: 4 * channels,
            frames_per_packet: 1,
            bytes_per_frame: 4 * channels,
            channels_per_frame: channels,
            bits_per_channel: 32,
            reserved: 0,
        };
        let callback = RenderCallback {
            proc_: render,
            refcon: shared.cast(),
        };
        unsafe {
            check(AudioUnitSetProperty(
                unit,
                STREAM_FORMAT,
                SCOPE_INPUT,
                0,
                (&raw const wire).cast(),
                size_of::<StreamDescription>() as u32,
            ))?;
            check(AudioUnitSetProperty(
                unit,
                SET_RENDER_CALLBACK,
                SCOPE_INPUT,
                0,
                (&raw const callback).cast(),
                size_of::<RenderCallback>() as u32,
            ))?;
            check(AudioUnitInitialize(unit))?;
            (*shared).stream.set_latency(latency_frames(unit, format));
            check(AudioObjectAddPropertyListener(
                SYSTEM_OBJECT,
                &DEFAULT_DEVICE_ADDRESS,
                default_device_changed,
                shared.cast(),
            ))?;
            check(AudioOutputUnitStart(unit))?;
        }
        Ok(device)
    }
}

impl bunny_ui::audio::Device for CoreAudio {
    fn open(stream: Arc<Stream>) -> Result<CoreAudio, AudioError> {
        if default_device().is_none() {
            return Err(AudioError::Unavailable);
        }
        CoreAudio::build(stream)
    }

    fn preferred_rate() -> Option<u32> {
        let device = default_device()?;
        property(device, NOMINAL_RATE, OBJECT_GLOBAL, 0.0f64)
            .filter(|rate| *rate >= 1.0)
            .map(|rate| rate.round() as u32)
    }
}

impl Drop for CoreAudio {
    fn drop(&mut self) {
        // SAFETY: the unit stops and the listener leaves before the box
        // the HAL threads read is freed. Each step tolerates a unit that
        // never got that far: `build` drops a half-made device on error.
        unsafe {
            let unit = (*self.shared).unit;
            AudioOutputUnitStop(unit);
            AudioObjectRemovePropertyListener(
                SYSTEM_OBJECT,
                &DEFAULT_DEVICE_ADDRESS,
                default_device_changed,
                self.shared.cast(),
            );
            AudioUnitUninitialize(unit);
            AudioComponentInstanceDispose(unit);
            drop(Box::from_raw(self.shared));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_character_codes_read_big_endian() {
        assert_eq!(OUTPUT_UNIT, 0x6175_6F75);
        assert_eq!(LINEAR_PCM, 0x6C70_636D);
        assert_eq!(OBJECT_OUTPUT, 0x6F75_7470);
    }

    #[test]
    fn the_wire_structs_have_the_sizes_coreaudio_reads() {
        assert_eq!(size_of::<StreamDescription>(), 40);
        assert_eq!(size_of::<ComponentDescription>(), 20);
        assert_eq!(size_of::<PropertyAddress>(), 12);
        assert_eq!(size_of::<RenderCallback>(), 16);
        assert_eq!(size_of::<AudioBuffer>(), 16);
    }

    /// A real unit, muted: the device pulls what was pushed and the clock
    /// moves. A machine with no output device has nothing to test.
    #[test]
    fn a_running_unit_plays_the_queue_and_moves_the_clock() {
        use bunny_ui::audio::AudioOutput;
        use std::time::{Duration, Instant};
        let format = AudioFormat::new(48_000, 2).expect("format");
        let mut output = match AudioOutput::<CoreAudio>::open(format) {
            Ok(output) => output,
            Err(AudioError::Unavailable) => return,
            Err(error) => panic!("the default output refused: {error}"),
        };
        output.set_muted(true);
        assert_eq!(output.push(&vec![0.25; 2 * 4_800]), 2 * 4_800);
        let start = Instant::now();
        while output.frames_played() < 4_800 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(output.frames_played(), 4_800);
        assert_eq!(output.queued_frames(), 0);
        assert!(
            output.latency() < Duration::from_secs(1),
            "{:?}",
            output.latency()
        );
        assert!(AudioOutput::<CoreAudio>::preferred_rate().is_some_and(|rate| rate >= 8_000));
    }
}
