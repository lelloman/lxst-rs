use lxst::{AudioFrame, Mixer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut mixer = Mixer::default();
    mixer.set_gain_db(-3.0);

    mixer.push(100, AudioFrame::new(8_000, 1, vec![0.25; 80])?);
    mixer.push(200, AudioFrame::new(8_000, 1, vec![0.50; 80])?);

    let Some(mixed) = mixer.mix_next()? else {
        println!("no frames were ready to mix");
        return Ok(());
    };

    let peak = mixed
        .samples()
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0, f32::max);

    println!(
        "mixed {} samples from two sources, peak {:.3}",
        mixed.samples().len(),
        peak
    );

    Ok(())
}
