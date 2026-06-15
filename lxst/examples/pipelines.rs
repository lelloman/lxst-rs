use lxst::{AudioFrame, AudioSource, BufferedSource, Loopback, Pipeline, RawBitDepth, RawCodec};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = BufferedSource::new(8_000, 1)?;
    source.push_frame(AudioFrame::new(8_000, 1, vec![0.125; 160])?)?;

    let loopback = Loopback::new(Box::new(RawCodec::new(RawBitDepth::Float32)), 8_000, 1, 4)?;
    let mut pipeline = Pipeline::new(
        Box::new(source),
        Box::new(RawCodec::new(RawBitDepth::Float32)),
        Box::new(loopback.clone()),
    );

    pipeline.start();
    let processed = pipeline.process_next()?;
    pipeline.stop();

    let mut decoded_source = loopback.clone();
    AudioSource::start(&mut decoded_source);
    let decoded = AudioSource::next_frame(&mut decoded_source)?;
    AudioSource::stop(&mut decoded_source);

    let decoded_samples = decoded
        .as_ref()
        .map(|frame| frame.samples().len())
        .unwrap_or_default();

    println!(
        "processed {processed}, queued {}, decoded samples {}",
        loopback.queued_frames(),
        decoded_samples
    );

    Ok(())
}
