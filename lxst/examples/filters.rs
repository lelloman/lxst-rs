use std::f32::consts::TAU;

use lxst::audio::AudioFilter;
use lxst::{Agc, AudioFrame, BandPass};

fn peak(frame: &AudioFrame) -> f32 {
    frame
        .samples()
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0, f32::max)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let samplerate = 48_000;
    let channels = 1;
    let frame_count = 960;
    let samples = (0..frame_count)
        .map(|index| {
            let t = index as f32 / samplerate as f32;
            let low = 0.20 * (TAU * 120.0 * t).sin();
            let voice_band = 0.08 * (TAU * 1_200.0 * t).sin();
            let high = 0.05 * (TAU * 12_000.0 * t).sin();
            low + voice_band + high
        })
        .collect();

    let input = AudioFrame::new(samplerate, channels, samples)?;
    let mut bandpass = BandPass::new(200.0, 8_500.0)?;
    let mut agc = Agc::new(-12.0, 12.0);

    let filtered = bandpass.process(input.clone());
    let output = agc.process(filtered);

    println!(
        "input peak {:.3}, filtered+agc peak {:.3}, duration {:.1} ms",
        peak(&input),
        peak(&output),
        output.duration_ms()
    );

    Ok(())
}
