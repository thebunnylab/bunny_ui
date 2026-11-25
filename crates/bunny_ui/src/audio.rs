//! Sound out of the speakers: a stream of samples the app pushes, and a
//! clock that says how much of it was heard.
//!
//! ```ignore
//! let format = AudioFormat::new(48_000, 2)?;
//! let mut output = bunny_ui::AudioOutput::open(format)?;
//! // the decoder's thread: interleaved f32, left then right
//! let taken = output.push(&samples);
//! // the picture's thread: what the listener hears now
//! let heard = output.handle().position();
//! ```
//!
//! The app owns the producing end ([`AudioOutput::push`], one thread at a
//! time) and the device pulls from the other end on its own real-time
//! thread. Between them sits a lock-free ring of interleaved `f32`
//! samples: neither side ever waits for the other, and a device that finds
//! the ring short plays silence for the rest of its buffer.
//!
//! [`AudioHandle`] is the shareable half: the clock, the queue, the volume,
//! mute and pause, from any thread. Every control is applied where the
//! device pulls, so they behave the same on every platform — a paused
//! stream plays silence and its clock stands still.
//!
//! A [`Device`] is what a shell plugs in: CoreAudio on macOS, WASAPI on
//! Windows, PulseAudio or ALSA on Linux, and [`Silent`] everywhere else,
//! which plays nothing and still advances the clock in real time, so a
//! picture synchronised to it keeps moving. The facade names the right one `bunny_ui::AudioOutput`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The shape of the samples the app pushes: frames per second, and the
/// samples in one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    rate: u32,
    channels: u16,
}

impl AudioFormat {
    pub const MIN_RATE: u32 = 8_000;
    pub const MAX_RATE: u32 = 384_000;
    pub const MAX_CHANNELS: u16 = 8;

    /// # Errors
    /// [`AudioError::InvalidFormat`] for a rate outside 8–384 kHz or a
    /// channel count outside 1–8.
    pub fn new(rate: u32, channels: u16) -> Result<AudioFormat, AudioError> {
        if !(Self::MIN_RATE..=Self::MAX_RATE).contains(&rate)
            || !(1..=Self::MAX_CHANNELS).contains(&channels)
        {
            return Err(AudioError::InvalidFormat);
        }
        Ok(AudioFormat { rate, channels })
    }

    #[must_use]
    pub const fn rate(self) -> u32 {
        self.rate
    }

    #[must_use]
    pub const fn channels(self) -> u16 {
        self.channels
    }

    /// How long `frames` last at this rate.
    #[must_use]
    pub fn duration_of(self, frames: u64) -> Duration {
        let rate = u64::from(self.rate);
        let nanos = (frames % rate) * 1_000_000_000 / rate;
        Duration::new(frames / rate, nanos as u32)
    }

    /// How many frames fit in `duration`, rounded down.
    #[must_use]
    pub fn frames_in(self, duration: Duration) -> u64 {
        let rate = u128::from(self.rate);
        (duration.as_nanos() * rate / 1_000_000_000) as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// The rate or the channel count is out of range.
    InvalidFormat,
    /// The system has no output device, or this target has no way to run one.
    Unavailable,
    /// The platform refused, with its own status code.
    Native(i32),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFormat => {
                f.write_str("the audio format must have 8-384 kHz and 1-8 channels")
            }
            Self::Unavailable => f.write_str("no audio output device is available"),
            Self::Native(status) => write!(f, "the audio device refused with status {status}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// What happened to the device under a stream, taken once by
/// [`AudioHandle::take_device_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceEvent {
    /// The output moved to another device (headphones in or out): the
    /// stream keeps playing there, and its latency was measured again.
    Changed,
    /// No output device is left. The stream keeps its queue and its clock
    /// stands still until a device returns, which reports `Changed`.
    Lost,
}

const NO_EVENT: u8 = 0;
const CHANGED: u8 = 1;
const LOST: u8 = 2;

/// A single-producer, single-consumer ring of samples. Positions only
/// grow, so the slot of a position is the position modulo the capacity,
/// and `floor` lets the producer drop everything queued without touching
/// the consumer's own index.
struct Ring {
    slots: Box<[AtomicU32]>,
    written: AtomicU64,
    read: AtomicU64,
    floor: AtomicU64,
}

impl Ring {
    fn new(capacity: usize) -> Ring {
        Ring {
            slots: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
            floor: AtomicU64::new(0),
        }
    }

    fn capacity(&self) -> u64 {
        self.slots.len() as u64
    }

    fn slot(&self, position: u64) -> &AtomicU32 {
        &self.slots[(position % self.capacity()) as usize]
    }

    fn queued(&self) -> u64 {
        let written = self.written.load(Ordering::Acquire);
        let start = self
            .read
            .load(Ordering::Acquire)
            .max(self.floor.load(Ordering::Acquire));
        written.saturating_sub(start)
    }

    /// Free room for the producer. Slots below the floor stay occupied
    /// until the consumer steps over them: it may be reading them now.
    fn free(&self) -> u64 {
        let written = self.written.load(Ordering::Relaxed);
        self.capacity() - (written - self.read.load(Ordering::Acquire))
    }

    fn push(&self, samples: &[f32]) -> usize {
        let room = self.free().min(samples.len() as u64);
        let written = self.written.load(Ordering::Relaxed);
        for (offset, sample) in samples[..room as usize].iter().enumerate() {
            self.slot(written + offset as u64)
                .store(sample.to_bits(), Ordering::Relaxed);
        }
        self.written.store(written + room, Ordering::Release);
        room as usize
    }

    fn pop(&self, out: &mut [f32], gain: f32) -> usize {
        let written = self.written.load(Ordering::Acquire);
        let start = self
            .read
            .load(Ordering::Relaxed)
            .max(self.floor.load(Ordering::Acquire));
        let taken = written.saturating_sub(start).min(out.len() as u64);
        for (offset, sample) in out[..taken as usize].iter_mut().enumerate() {
            *sample =
                f32::from_bits(self.slot(start + offset as u64).load(Ordering::Relaxed)) * gain;
        }
        self.read.store(start + taken, Ordering::Release);
        taken as usize
    }

    fn flush(&self) {
        self.floor
            .fetch_max(self.written.load(Ordering::Acquire), Ordering::AcqRel);
    }
}

/// The state both ends share: the ring, the clock and the controls.
/// A [`Device`] holds one and calls [`Stream::render`] from its own
/// thread; nothing on that road allocates, locks or waits.
pub struct Stream {
    format: AudioFormat,
    ring: Ring,
    played: AtomicU64,
    latency: AtomicU64,
    gain: AtomicU32,
    muted: AtomicBool,
    paused: AtomicBool,
    underruns: AtomicU64,
    event: AtomicU8,
}

impl Stream {
    /// A stream that queues up to `capacity` frames.
    #[must_use]
    pub fn new(format: AudioFormat, capacity: u64) -> Stream {
        let frames = capacity.max(1);
        Stream {
            format,
            ring: Ring::new((frames * u64::from(format.channels)) as usize),
            played: AtomicU64::new(0),
            latency: AtomicU64::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            event: AtomicU8::new(NO_EVENT),
        }
    }

    #[must_use]
    pub fn format(&self) -> AudioFormat {
        self.format
    }

    /// Fills one device buffer of interleaved samples in the stream's own
    /// format. A paused stream fills silence and takes nothing; a short
    /// queue plays what it has, then silence, and counts an underrun.
    pub fn render(&self, out: &mut [f32]) {
        let channels = usize::from(self.format.channels);
        let whole = out.len() - out.len() % channels;
        if self.paused.load(Ordering::Acquire) {
            out.fill(0.0);
            return;
        }
        let gain = if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(self.gain.load(Ordering::Relaxed))
        };
        let taken = self.ring.pop(&mut out[..whole], gain);
        out[taken..].fill(0.0);
        self.played
            .fetch_add((taken / channels) as u64, Ordering::AcqRel);
        if taken < whole && self.ring.written.load(Ordering::Acquire) > 0 {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The device's word on how many frames of this stream are still
    /// between [`Stream::render`] and the listener's ear.
    pub fn set_latency(&self, frames: u64) {
        self.latency.store(frames, Ordering::Release);
    }

    /// The device's word on what happened to it. A loss is kept over a
    /// change until the app takes it.
    pub fn notify(&self, event: DeviceEvent) {
        let code = match event {
            DeviceEvent::Changed => CHANGED,
            DeviceEvent::Lost => LOST,
        };
        self.event.fetch_max(code, Ordering::AcqRel);
    }
}

/// What a shell plugs into [`AudioOutput`]: something that, once open,
/// calls [`Stream::render`] on its own thread until it is dropped.
pub trait Device: Sized + Send + 'static {
    /// Starts pulling from `stream`.
    ///
    /// # Errors
    /// [`AudioError::Unavailable`] without an output device, or the
    /// platform's own refusal.
    fn open(stream: Arc<Stream>) -> Result<Self, AudioError>;

    /// The rate the system's output device runs at, when it says. Pushing
    /// at that rate spares the platform a resampler.
    fn preferred_rate() -> Option<u32>;
}

/// The shareable half of an output: its clock, its queue and its
/// controls, from any thread.
#[derive(Clone)]
pub struct AudioHandle {
    stream: Arc<Stream>,
}

impl AudioHandle {
    #[must_use]
    pub fn format(&self) -> AudioFormat {
        self.stream.format
    }

    /// Frames pushed and not yet handed to the device.
    #[must_use]
    pub fn queued_frames(&self) -> u64 {
        self.stream.ring.queued() / u64::from(self.stream.format.channels)
    }

    /// Frames a push can take now.
    #[must_use]
    pub fn free_frames(&self) -> u64 {
        self.stream.ring.free() / u64::from(self.stream.format.channels)
    }

    #[must_use]
    pub fn capacity_frames(&self) -> u64 {
        self.stream.ring.capacity() / u64::from(self.stream.format.channels)
    }

    /// How long a frame handed to the device takes to be heard.
    #[must_use]
    pub fn latency_frames(&self) -> u64 {
        self.stream.latency.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn latency(&self) -> Duration {
        self.stream.format.duration_of(self.latency_frames())
    }

    /// Frames the device took from the queue since the output opened. It
    /// only grows: a flush, a pause and an underrun add nothing.
    #[must_use]
    pub fn frames_played(&self) -> u64 {
        self.stream.played.load(Ordering::Acquire)
    }

    /// Frames the listener has heard: those played, less the latency.
    #[must_use]
    pub fn frames_heard(&self) -> u64 {
        self.frames_played().saturating_sub(self.latency_frames())
    }

    /// The clock to synchronise a picture to: how much of the pushed
    /// sound has reached the ear since the output opened.
    #[must_use]
    pub fn position(&self) -> Duration {
        self.stream.format.duration_of(self.frames_heard())
    }

    /// A gain from silent (0) to as recorded (1); out-of-range values are
    /// clamped and NaN is ignored.
    pub fn set_volume(&self, volume: f32) {
        if !volume.is_nan() {
            self.stream
                .gain
                .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.stream.gain.load(Ordering::Relaxed))
    }

    /// Silences the output and keeps the clock running; the volume stays.
    pub fn set_muted(&self, muted: bool) {
        self.stream.muted.store(muted, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.stream.muted.load(Ordering::Relaxed)
    }

    /// Stops taking from the queue: the device plays silence, the queue
    /// keeps what it has and the clock stands still.
    pub fn pause(&self) {
        self.stream.paused.store(true, Ordering::Release);
    }

    pub fn resume(&self) {
        self.stream.paused.store(false, Ordering::Release);
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.stream.paused.load(Ordering::Acquire)
    }

    /// Drops everything queued, for a seek. The clock keeps its count:
    /// the app remembers where the new samples start.
    pub fn flush(&self) {
        self.stream.ring.flush();
    }

    /// Device buffers that found the queue short after the first push.
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.stream.underruns.load(Ordering::Relaxed)
    }

    /// What happened to the device since the last call, if anything.
    #[must_use]
    pub fn take_device_event(&self) -> Option<DeviceEvent> {
        match self.stream.event.swap(NO_EVENT, Ordering::AcqRel) {
            CHANGED => Some(DeviceEvent::Changed),
            LOST => Some(DeviceEvent::Lost),
            _ => None,
        }
    }
}

/// An open output on device `D`: the producing end, plus everything
/// [`AudioHandle`] offers. Dropping it stops the device.
pub struct AudioOutput<D: Device> {
    handle: AudioHandle,
    device: D,
}

impl<D: Device> AudioOutput<D> {
    /// The queue an output gets from [`AudioOutput::open`]: a second.
    pub const DEFAULT_CAPACITY: Duration = Duration::from_secs(1);

    /// # Errors
    /// See [`Device::open`].
    pub fn open(format: AudioFormat) -> Result<Self, AudioError> {
        Self::with_capacity(format, format.frames_in(Self::DEFAULT_CAPACITY))
    }

    /// An output whose queue holds `frames` frames.
    ///
    /// # Errors
    /// See [`Device::open`].
    pub fn with_capacity(format: AudioFormat, frames: u64) -> Result<Self, AudioError> {
        let stream = Arc::new(Stream::new(format, frames));
        let device = D::open(Arc::clone(&stream))?;
        Ok(AudioOutput {
            handle: AudioHandle { stream },
            device,
        })
    }

    /// See [`Device::preferred_rate`].
    #[must_use]
    pub fn preferred_rate() -> Option<u32> {
        D::preferred_rate()
    }

    /// Queues interleaved samples and answers how many it took: whole
    /// frames only, as many as there is room for. The rest is the app's to
    /// push again once [`AudioHandle::free_frames`] grows.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let channels = usize::from(self.handle.stream.format.channels);
        let room = (self.handle.stream.ring.free() as usize).min(samples.len());
        self.handle
            .stream
            .ring
            .push(&samples[..room - room % channels])
    }

    /// A handle for the threads that read the clock or move the controls.
    #[must_use]
    pub fn handle(&self) -> AudioHandle {
        self.handle.clone()
    }

    #[must_use]
    pub fn device(&self) -> &D {
        &self.device
    }
}

impl<D: Device> std::ops::Deref for AudioOutput<D> {
    type Target = AudioHandle;

    fn deref(&self) -> &AudioHandle {
        &self.handle
    }
}

/// The device of a target with no sound: it plays nothing, and pulls from
/// the stream at its rate in real time, so the clock still advances.
pub struct Silent {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Silent {
    const BEAT: Duration = Duration::from_millis(10);
}

impl Device for Silent {
    fn open(stream: Arc<Stream>) -> Result<Silent, AudioError> {
        let running = Arc::new(AtomicBool::new(true));
        let alive = Arc::clone(&running);
        let thread = std::thread::Builder::new()
            .name("bunny-audio-silent".into())
            .spawn(move || {
                let format = stream.format();
                let channels = usize::from(format.channels());
                let mut buffer =
                    vec![0.0f32; format.frames_in(Silent::BEAT * 4) as usize * channels];
                let start = Instant::now();
                let mut pulled = 0u64;
                while alive.load(Ordering::Acquire) {
                    let due = format.frames_in(start.elapsed());
                    while pulled < due {
                        let frames = (due - pulled).min((buffer.len() / channels) as u64);
                        stream.render(&mut buffer[..frames as usize * channels]);
                        pulled += frames;
                    }
                    std::thread::park_timeout(Silent::BEAT);
                }
            })
            .map_err(|_| AudioError::Unavailable)?;
        Ok(Silent {
            running,
            thread: Some(thread),
        })
    }

    fn preferred_rate() -> Option<u32> {
        None
    }
}

impl Drop for Silent {
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

    fn stereo() -> AudioFormat {
        AudioFormat::new(48_000, 2).expect("format")
    }

    /// A device that renders only when the test says so.
    struct Manual {
        stream: Arc<Stream>,
    }

    impl Device for Manual {
        fn open(stream: Arc<Stream>) -> Result<Manual, AudioError> {
            Ok(Manual { stream })
        }

        fn preferred_rate() -> Option<u32> {
            Some(44_100)
        }
    }

    impl Manual {
        fn pull(&self, frames: usize) -> Vec<f32> {
            let mut out = vec![9.0; frames * usize::from(self.stream.format().channels())];
            self.stream.render(&mut out);
            out
        }
    }

    fn output(frames: u64) -> AudioOutput<Manual> {
        AudioOutput::with_capacity(stereo(), frames).expect("open")
    }

    #[test]
    fn a_format_keeps_to_the_rates_and_channels_a_device_takes() {
        assert_eq!(AudioFormat::new(7_999, 2), Err(AudioError::InvalidFormat));
        assert_eq!(AudioFormat::new(48_000, 0), Err(AudioError::InvalidFormat));
        assert_eq!(AudioFormat::new(48_000, 9), Err(AudioError::InvalidFormat));
        let format = AudioFormat::new(44_100, 1).expect("format");
        assert_eq!((format.rate(), format.channels()), (44_100, 1));
    }

    #[test]
    fn frames_and_durations_convert_both_ways() {
        let format = stereo();
        assert_eq!(format.duration_of(48_000), Duration::from_secs(1));
        assert_eq!(format.duration_of(24_000), Duration::from_millis(500));
        assert_eq!(format.frames_in(Duration::from_millis(10)), 480);
        assert_eq!(format.frames_in(format.duration_of(12_345)), 12_345);
    }

    #[test]
    fn a_push_takes_whole_frames_as_far_as_there_is_room() {
        let mut output = output(4);
        assert_eq!(output.capacity_frames(), 4);
        assert_eq!(
            output.push(&[0.1, 0.2, 0.3]),
            2,
            "half a frame stays with the app"
        );
        assert_eq!(output.push(&[0.5; 10]), 6, "the ring holds four frames");
        assert_eq!((output.queued_frames(), output.free_frames()), (4, 0));
        assert_eq!(output.push(&[0.5; 2]), 0);
    }

    #[test]
    fn the_device_plays_the_samples_in_order_and_silence_after_them() {
        let mut output = output(8);
        output.push(&[0.1, 0.2, 0.3, 0.4]);
        let heard = output.device().pull(3);
        assert_eq!(heard, vec![0.1, 0.2, 0.3, 0.4, 0.0, 0.0]);
        assert_eq!(output.frames_played(), 2, "silence is not played frames");
        assert_eq!(output.underruns(), 1);
    }

    #[test]
    fn the_ring_wraps_around_its_end() {
        let mut output = output(3);
        for round in 0..10 {
            let value = round as f32;
            assert_eq!(
                output.push(&[value, -value, value, -value]),
                4,
                "round {round}"
            );
            assert_eq!(output.device().pull(2), vec![value, -value, value, -value]);
        }
        assert_eq!(output.frames_played(), 20);
        assert_eq!(output.underruns(), 0);
    }

    #[test]
    fn the_volume_and_mute_shape_what_is_heard_and_not_the_clock() {
        let mut output = output(8);
        output.set_volume(0.5);
        output.push(&[1.0, -1.0]);
        assert_eq!(output.device().pull(1), vec![0.5, -0.5]);
        output.set_muted(true);
        output.push(&[1.0, -1.0]);
        assert_eq!(output.device().pull(1), vec![0.0, 0.0]);
        assert_eq!(output.frames_played(), 2, "a muted stream keeps time");
        assert!(output.is_muted());
        output.set_volume(4.0);
        assert_eq!(output.volume(), 1.0);
        output.set_volume(f32::NAN);
        assert_eq!(output.volume(), 1.0);
    }

    #[test]
    fn a_paused_stream_plays_silence_and_keeps_its_queue_and_its_clock() {
        let mut output = output(8);
        output.push(&[0.3, 0.3, 0.4, 0.4]);
        output.pause();
        assert_eq!(output.device().pull(2), vec![0.0; 4]);
        assert_eq!((output.frames_played(), output.queued_frames()), (0, 2));
        assert_eq!(output.underruns(), 0, "a pause is no underrun");
        output.resume();
        assert_eq!(output.device().pull(2), vec![0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn a_flush_drops_the_queue_and_what_follows_plays_next() {
        let mut output = output(4);
        output.push(&[0.1; 8]);
        output.flush();
        assert_eq!(output.queued_frames(), 0);
        assert_eq!(
            output.push(&[0.2; 2]),
            0,
            "the device has not stepped over the old samples yet"
        );
        assert_eq!(output.device().pull(1), vec![0.0, 0.0]);
        assert_eq!(output.push(&[0.7, 0.8]), 2);
        assert_eq!(output.device().pull(1), vec![0.7, 0.8]);
        assert_eq!(output.frames_played(), 1);
    }

    #[test]
    fn the_position_is_what_was_played_less_the_latency() {
        let mut output = output(48_000);
        output.push(&vec![0.0; 2 * 24_000]);
        output.device().pull(24_000);
        assert_eq!(output.position(), Duration::from_millis(500));
        output.device.stream.set_latency(4_800);
        assert_eq!(output.latency(), Duration::from_millis(100));
        assert_eq!(output.frames_heard(), 19_200);
        assert_eq!(output.position(), Duration::from_millis(400));
        output.device.stream.set_latency(48_000);
        assert_eq!(
            output.position(),
            Duration::ZERO,
            "nothing heard yet is zero, not negative"
        );
    }

    #[test]
    fn a_loss_outranks_a_change_until_it_is_taken() {
        let output = output(4);
        assert_eq!(output.take_device_event(), None);
        output.device().stream.notify(DeviceEvent::Changed);
        output.device().stream.notify(DeviceEvent::Lost);
        output.device().stream.notify(DeviceEvent::Changed);
        assert_eq!(output.take_device_event(), Some(DeviceEvent::Lost));
        assert_eq!(output.take_device_event(), None);
        output.device().stream.notify(DeviceEvent::Changed);
        assert_eq!(
            output.handle().take_device_event(),
            Some(DeviceEvent::Changed)
        );
    }

    #[test]
    fn the_producer_and_the_device_trade_across_threads() {
        let mut output = output(256);
        let stream = Arc::clone(&output.device().stream);
        let total = 48_000usize;
        let player = std::thread::spawn(move || {
            let mut heard = Vec::with_capacity(total * 2);
            let mut buffer = vec![0.0f32; 64];
            while heard.len() < total * 2 {
                let before = stream.played.load(Ordering::Acquire);
                stream.render(&mut buffer);
                let frames = (stream.played.load(Ordering::Acquire) - before) as usize;
                heard.extend_from_slice(&buffer[..frames * 2]);
            }
            heard
        });
        let samples: Vec<f32> = (0..total).flat_map(|i| [i as f32, -(i as f32)]).collect();
        let mut sent = 0;
        while sent < samples.len() {
            sent += output.push(&samples[sent..]);
            std::thread::yield_now();
        }
        assert_eq!(player.join().expect("player"), samples);
        assert_eq!(output.frames_played(), total as u64);
    }

    #[test]
    fn the_preferred_rate_is_the_devices() {
        assert_eq!(AudioOutput::<Manual>::preferred_rate(), Some(44_100));
        assert_eq!(AudioOutput::<Silent>::preferred_rate(), None);
    }

    #[test]
    fn the_silent_device_keeps_time_without_sound() {
        let mut output = AudioOutput::<Silent>::open(stereo()).expect("silent");
        output.push(&vec![0.5; 2 * 48_000]);
        let start = Instant::now();
        while output.frames_played() < 2_400 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let played = output.frames_played();
        assert!(
            played >= 2_400,
            "fifty milliseconds were played, in real time"
        );
        assert!(
            played <= output.format().frames_in(start.elapsed()) + 4_800,
            "never faster than the clock on the wall"
        );
        output.pause();
        std::thread::sleep(Duration::from_millis(30));
        let paused = output.frames_played();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            output.frames_played(),
            paused,
            "a paused clock stands still"
        );
    }
}
