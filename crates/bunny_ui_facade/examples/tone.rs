//! A sine tone out of the speakers, and the clock that says how much of
//! it was heard.
//!
//! cargo run -p bunny-ui --example tone -- [seconds] [hertz] [volume]

use std::time::{Duration, Instant};

use bunny_ui::AudioOutput;
use bunny_ui::audio::AudioFormat;

fn main() {
    let mut args = std::env::args().skip(1).map(|arg| arg.parse::<f64>().ok());
    let seconds = args.next().flatten().unwrap_or(2.0);
    let hertz = args.next().flatten().unwrap_or(440.0);
    let volume = args.next().flatten().unwrap_or(0.2) as f32;

    let rate = AudioOutput::preferred_rate().unwrap_or(48_000);
    let format = AudioFormat::new(rate, 2).expect("format");
    let mut output = match AudioOutput::open(format) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("no sound: {error}");
            std::process::exit(1);
        }
    };
    output.set_volume(volume);
    println!("{rate} Hz, latency {:?}", output.latency());

    let total = format.frames_in(Duration::from_secs_f64(seconds));
    let step = std::f64::consts::TAU * hertz / f64::from(rate);
    let mut frame = 0u64;
    let mut pending = Vec::new();
    let start = Instant::now();
    while output.frames_played() < total {
        if pending.is_empty() && frame < total {
            let chunk = (total - frame).min(1_024);
            pending = (frame..frame + chunk)
                .flat_map(|n| {
                    let sample = (step * n as f64).sin() as f32;
                    [sample, sample]
                })
                .collect();
            frame += chunk;
        }
        let taken = output.push(&pending);
        pending.drain(..taken);
        if let Some(event) = output.take_device_event() {
            println!("device: {event:?}, latency now {:?}", output.latency());
        }
        std::thread::sleep(Duration::from_millis(5));
        if start.elapsed() > Duration::from_secs_f64(seconds + 5.0) {
            break;
        }
    }
    println!(
        "played {} frames in {:?}, heard {:?}, {} underruns",
        output.frames_played(),
        start.elapsed(),
        output.position(),
        output.underruns()
    );
}
