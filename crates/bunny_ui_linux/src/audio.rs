//! Linux speakers: [`bunny_ui::audio`] played through PulseAudio, or
//! straight through ALSA where no sound server runs.
//!
//! Both libraries are opened at run time (`libpulse-simple.so.0`, then
//! `libasound.so.2`), so a build needs neither and a machine without
//! one still plays through the other. PipeWire answers the PulseAudio
//! protocol, so the first road covers it too.
//!
//! One thread per stream pulls ten milliseconds at a time from
//! [`Stream::render`] and hands them to a blocking write: the library's
//! own buffer paces the loop. Both take the app's rate and channel count
//! and convert on their side. The latency the clock subtracts is what the
//! library still holds, measured after every write. A device that stops
//! taking samples reports [`DeviceEvent::Lost`]; the thread then opens
//! the default output again every half second and reports
//! [`DeviceEvent::Changed`] once it plays again.
//!
//! ```ignore
//! let mut output = AudioOutput::<LinuxAudio>::open(AudioFormat::new(48_000, 2)?)?;
//! output.push(&samples);
//! ```

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use bunny_ui::audio::{AudioError, AudioFormat, DeviceEvent, Stream};

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;

const CHUNK: Duration = Duration::from_millis(10);
const TARGET: Duration = Duration::from_millis(40);
const RETRY: Duration = Duration::from_millis(500);

/// The function `name` of `library`, as the pointer type `F` it is declared with.
///
/// # Safety
/// `F` must be the C signature of `name`.
unsafe fn function<F: Copy>(library: *mut c_void, name: &std::ffi::CStr) -> Option<F> {
    assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    let found = unsafe { dlsym(library, name.as_ptr()) };
    (!found.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&found) })
}

// MARK: - PulseAudio, the simple API

const PA_STREAM_PLAYBACK: c_int = 1;
const PA_SAMPLE_FLOAT32LE: c_int = 5;

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

struct Pulse {
    new: unsafe extern "C" fn(
        server: *const c_char,
        name: *const c_char,
        direction: c_int,
        device: *const c_char,
        stream_name: *const c_char,
        spec: *const SampleSpec,
        map: *const c_void,
        attr: *const BufferAttr,
        error: *mut c_int,
    ) -> *mut c_void,
    write: unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int,
    latency: unsafe extern "C" fn(*mut c_void, *mut c_int) -> u64,
    free: unsafe extern "C" fn(*mut c_void),
}

fn pulse() -> Option<&'static Pulse> {
    static PULSE: OnceLock<Option<Pulse>> = OnceLock::new();
    PULSE
        .get_or_init(|| {
            let library = unsafe { dlopen(c"libpulse-simple.so.0".as_ptr(), RTLD_NOW) };
            if library.is_null() {
                return None;
            }
            unsafe {
                Some(Pulse {
                    new: function(library, c"pa_simple_new")?,
                    write: function(library, c"pa_simple_write")?,
                    latency: function(library, c"pa_simple_get_latency")?,
                    free: function(library, c"pa_simple_free")?,
                })
            }
        })
        .as_ref()
}

// MARK: - ALSA

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

struct Alsa {
    open: unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int,
    set_params:
        unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    writei: unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long,
    recover: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
    delay: unsafe extern "C" fn(*mut c_void, *mut c_long) -> c_int,
    close: unsafe extern "C" fn(*mut c_void) -> c_int,
}

fn alsa() -> Option<&'static Alsa> {
    static ALSA: OnceLock<Option<Alsa>> = OnceLock::new();
    ALSA.get_or_init(|| {
        let library = unsafe { dlopen(c"libasound.so.2".as_ptr(), RTLD_NOW) };
        if library.is_null() {
            return None;
        }
        unsafe {
            Some(Alsa {
                open: function(library, c"snd_pcm_open")?,
                set_params: function(library, c"snd_pcm_set_params")?,
                writei: function(library, c"snd_pcm_writei")?,
                recover: function(library, c"snd_pcm_recover")?,
                delay: function(library, c"snd_pcm_delay")?,
                close: function(library, c"snd_pcm_close")?,
            })
        }
    })
    .as_ref()
}

// MARK: - One open connection

enum Sink {
    Pulse(&'static Pulse, *mut c_void),
    Alsa(&'static Alsa, *mut c_void),
}

fn bytes_of(format: AudioFormat, duration: Duration) -> u32 {
    let frames = format.frames_in(duration);
    u32::try_from(frames * u64::from(format.channels()) * 4).unwrap_or(u32::MAX)
}

impl Sink {
    fn open(format: AudioFormat) -> Result<Sink, AudioError> {
        if let Some(pulse) = pulse() {
            let spec = SampleSpec {
                format: PA_SAMPLE_FLOAT32LE,
                rate: format.rate(),
                channels: format.channels() as u8,
            };
            let attr = BufferAttr {
                maxlength: u32::MAX,
                tlength: bytes_of(format, TARGET),
                prebuf: u32::MAX,
                minreq: bytes_of(format, CHUNK),
                fragsize: u32::MAX,
            };
            let mut error = 0;
            let simple = unsafe {
                (pulse.new)(
                    std::ptr::null(),
                    c"bunny-ui".as_ptr(),
                    PA_STREAM_PLAYBACK,
                    std::ptr::null(),
                    c"playback".as_ptr(),
                    &spec,
                    std::ptr::null(),
                    &attr,
                    &mut error,
                )
            };
            if !simple.is_null() {
                return Ok(Sink::Pulse(pulse, simple));
            }
        }
        let alsa = alsa().ok_or(AudioError::Unavailable)?;
        let mut pcm = std::ptr::null_mut();
        let status =
            unsafe { (alsa.open)(&mut pcm, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
        if status < 0 || pcm.is_null() {
            return Err(AudioError::Unavailable);
        }
        let latency = u32::try_from(TARGET.as_micros()).unwrap_or(u32::MAX);
        let status = unsafe {
            (alsa.set_params)(
                pcm,
                SND_PCM_FORMAT_FLOAT_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                c_uint::from(format.channels()),
                format.rate(),
                1,
                latency,
            )
        };
        if status < 0 {
            unsafe { (alsa.close)(pcm) };
            return Err(AudioError::Native(status));
        }
        Ok(Sink::Alsa(alsa, pcm))
    }

    /// Hands one buffer of whole frames over, blocking while the library's
    /// own buffer is full, and answers the frames it still holds.
    fn write(&self, samples: &[f32], format: AudioFormat) -> Result<u64, AudioError> {
        let channels = usize::from(format.channels());
        match *self {
            Sink::Pulse(pulse, simple) => {
                let mut error = 0;
                let status = unsafe {
                    (pulse.write)(
                        simple,
                        samples.as_ptr().cast(),
                        size_of_val(samples),
                        &mut error,
                    )
                };
                if status < 0 {
                    return Err(AudioError::Native(error));
                }
                let micros = unsafe { (pulse.latency)(simple, &mut error) };
                Ok(format.frames_in(Duration::from_micros(micros)))
            }
            Sink::Alsa(alsa, pcm) => {
                let mut offset = 0;
                while offset < samples.len() {
                    let frames = ((samples.len() - offset) / channels) as c_ulong;
                    let written =
                        unsafe { (alsa.writei)(pcm, samples[offset..].as_ptr().cast(), frames) };
                    if written < 0 {
                        let code = c_int::try_from(written).unwrap_or(c_int::MIN);
                        if unsafe { (alsa.recover)(pcm, code, 1) } < 0 {
                            return Err(AudioError::Native(code));
                        }
                        continue;
                    }
                    offset += written as usize * channels;
                }
                let mut delay: c_long = 0;
                let held = if unsafe { (alsa.delay)(pcm, &mut delay) } < 0 {
                    0
                } else {
                    delay
                };
                Ok(u64::try_from(held).unwrap_or(0))
            }
        }
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        match *self {
            Sink::Pulse(pulse, simple) => unsafe { (pulse.free)(simple) },
            Sink::Alsa(alsa, pcm) => unsafe {
                (alsa.close)(pcm);
            },
        }
    }
}

// MARK: - The device

/// The Linux [`bunny_ui::audio::Device`]: a thread that plays the stream
/// through PulseAudio or ALSA, stopped and closed when dropped.
pub struct LinuxAudio {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn play(stream: &Stream, sink: &mut Option<Sink>, alive: &AtomicBool) {
    let format = stream.format();
    let mut buffer =
        vec![0.0f32; format.frames_in(CHUNK) as usize * usize::from(format.channels())];
    while alive.load(Ordering::Acquire) {
        let Some(open) = sink.as_ref() else {
            std::thread::park_timeout(RETRY);
            if let Ok(again) = Sink::open(format) {
                *sink = Some(again);
                stream.notify(DeviceEvent::Changed);
            }
            continue;
        };
        stream.render(&mut buffer);
        match open.write(&buffer, format) {
            Ok(held) => stream.set_latency(held),
            Err(_) => {
                *sink = None;
                stream.notify(DeviceEvent::Lost);
            }
        }
    }
}

impl bunny_ui::audio::Device for LinuxAudio {
    fn open(stream: Arc<Stream>) -> Result<LinuxAudio, AudioError> {
        let running = Arc::new(AtomicBool::new(true));
        let alive = Arc::clone(&running);
        let (opened, outcome) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("bunny-audio".into())
            .spawn(move || {
                let mut sink = match Sink::open(stream.format()) {
                    Ok(sink) => Some(sink),
                    Err(error) => {
                        let _ = opened.send(Err(error));
                        return;
                    }
                };
                let _ = opened.send(Ok(()));
                play(&stream, &mut sink, &alive);
            })
            .map_err(|_| AudioError::Unavailable)?;
        match outcome.recv() {
            Ok(Ok(())) => Ok(LinuxAudio {
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

    fn preferred_rate() -> Option<u32> {
        None
    }
}

impl Drop for LinuxAudio {
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
    fn the_wire_structs_have_the_sizes_the_libraries_read() {
        assert_eq!(size_of::<SampleSpec>(), 12);
        assert_eq!(size_of::<BufferAttr>(), 20);
    }

    #[test]
    fn the_buffer_sizes_are_bytes_of_interleaved_floats() {
        let format = AudioFormat::new(48_000, 2).expect("format");
        assert_eq!(bytes_of(format, CHUNK), 480 * 2 * 4);
        assert_eq!(bytes_of(format, TARGET), 1_920 * 2 * 4);
    }
}
