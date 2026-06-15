use std::env;

use lxst::{AudioSource, CodecProfile, OpusFileSink, OpusFileSource};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!(
            "usage: cargo run -p lxst --example file_source -- input.opus output.opus [max_frames]"
        );
        return Ok(());
    };
    let output = args.next().unwrap_or_else(|| "copy.opus".to_string());
    let max_frames = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()?;

    let mut source = OpusFileSource::open(input, 40, false)?;
    let mut sink = OpusFileSink::create(&output, CodecProfile::OpusAudioMax)?;

    AudioSource::start(&mut source);
    let mut frames = 0usize;
    let mut samples = 0usize;
    while max_frames.is_none_or(|limit| frames < limit) {
        let Some(frame) = AudioSource::next_frame(&mut source)? else {
            break;
        };
        samples += frame.samples().len();
        sink.handle_frame(&frame)?;
        frames += 1;
    }
    AudioSource::stop(&mut source);
    sink.finalize()?;

    println!("wrote {frames} frames / {samples} samples to {output}");

    Ok(())
}
