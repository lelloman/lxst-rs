use lxst::{AudioSource, ToneSource};

fn peak(samples: &[f32]) -> f32 {
    samples.iter().copied().map(f32::abs).fold(0.0, f32::max)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut tone = ToneSource::with_frame_ms(388.0, 8_000, 1, 0.4, 20);

    AudioSource::start(&mut tone);
    for index in 0..5 {
        let Some(frame) = AudioSource::next_frame(&mut tone)? else {
            break;
        };
        println!(
            "frame {index}: {} samples, {:.1} ms, peak {:.3}",
            frame.frame_count(),
            frame.duration_ms(),
            peak(frame.samples())
        );
    }

    AudioSource::stop(&mut tone);
    while AudioSource::is_running(&tone) {
        if AudioSource::next_frame(&mut tone)?.is_none() {
            break;
        }
    }

    Ok(())
}
