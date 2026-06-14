use lxst::audio::AudioFilter;
use lxst::{Agc, AudioFrame, AudioSource, BandPass, HighPass, LowPass, ToneSource};

#[derive(Debug)]
struct UpstreamDspFixture {
    source_commit: &'static str,
    tone_plain: DspCase,
    tone_eased: DspCase,
    filter_input: DspCase,
    highpass_300hz: DspCase,
    lowpass_1200hz: DspCase,
    bandpass_300_1200hz: DspCase,
    agc_input: DspCase,
    agc_output: DspCase,
}

#[derive(Debug, Clone, Copy)]
struct DspCase {
    samplerate: u32,
    channels: u8,
    samples: &'static [f32],
}

fn fixture() -> UpstreamDspFixture {
    include!("fixtures/upstream_dsp.rs")
}

fn frame(case: DspCase) -> AudioFrame {
    AudioFrame::new(case.samplerate, case.channels, case.samples.to_vec()).unwrap()
}

fn assert_samples_close(actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let diff = (actual - expected).abs();
        assert!(
            diff <= tolerance,
            "sample {index}: actual {actual:.9} expected {expected:.9} diff {diff:.9} tolerance {tolerance:.9}"
        );
    }
}

#[test]
fn generated_dsp_fixture_is_pinned_to_upstream_checkout() {
    let fixture = fixture();
    assert_eq!(fixture.source_commit.len(), 40);
    assert_eq!(fixture.tone_plain.samplerate, 8_000);
    assert_eq!(fixture.tone_plain.channels, 2);
}

#[test]
fn tone_source_matches_upstream_phase_order_without_ease() {
    let fixture = fixture();
    let mut tone = ToneSource::new(1_000.0, 8_000, 2, 0.5);
    tone.set_ease(false);

    let actual = tone.next_frame(8).unwrap();
    assert_samples_close(actual.samples(), fixture.tone_plain.samples, 1e-6);
}

#[test]
fn tone_source_matches_upstream_ease_in_shape() {
    let fixture = fixture();
    let mut tone = ToneSource::with_frame_ms(1_000.0, 8_000, 1, 0.5, 1);
    tone.start();

    let actual = AudioSource::next_frame(&mut tone).unwrap().unwrap();
    assert_samples_close(&actual.samples()[..8], fixture.tone_eased.samples, 1e-6);
}

#[test]
fn highpass_matches_upstream_native_filter_vector() {
    let fixture = fixture();
    let mut filter = HighPass::new(300.0);
    let actual = filter.process(frame(fixture.filter_input));
    assert_samples_close(actual.samples(), fixture.highpass_300hz.samples, 1e-6);
}

#[test]
fn lowpass_matches_upstream_native_filter_vector() {
    let fixture = fixture();
    let mut filter = LowPass::new(1_200.0);
    let actual = filter.process(frame(fixture.filter_input));
    assert_samples_close(actual.samples(), fixture.lowpass_1200hz.samples, 1e-6);
}

#[test]
fn bandpass_matches_upstream_native_filter_vector() {
    let fixture = fixture();
    let mut filter = BandPass::new(300.0, 1_200.0).unwrap();
    let actual = filter.process(frame(fixture.filter_input));
    assert_samples_close(actual.samples(), fixture.bandpass_300_1200hz.samples, 1e-6);
}

#[test]
fn agc_matches_upstream_native_filter_vector() {
    let fixture = fixture();
    let mut filter = Agc::new(-12.0, 12.0);
    let actual = filter.process(frame(fixture.agc_input));
    assert_samples_close(actual.samples(), fixture.agc_output.samples, 1e-5);
}
