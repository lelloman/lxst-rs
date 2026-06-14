#!/usr/bin/env python3
"""Generate deterministic DSP fixtures from upstream LXST formulas."""

from __future__ import annotations

import math
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = Path(sys.argv[1]).expanduser() if len(sys.argv) > 1 else Path("~/lxst").expanduser()
OUT = ROOT / "lxst" / "tests" / "fixtures" / "upstream_dsp.rs"


def rust_float(value: float) -> str:
    if math.isnan(value):
        return "f32::NAN"
    if math.isinf(value):
        return "f32::INFINITY" if value > 0 else "f32::NEG_INFINITY"
    rendered = f"{value:.9g}"
    if "e" not in rendered and "." not in rendered:
        rendered += ".0"
    return rendered


def rust_samples(values: list[float]) -> str:
    return "&[" + ", ".join(rust_float(value) for value in values) + "]"


def tone_samples(frequency: float, samplerate: int, channels: int, gain: float, frames: int, ease: bool) -> list[float]:
    theta = 0.0
    ease_gain = 0.0 if ease else 1.0
    ease_step = 1.0 / (samplerate * (20.0 / 1000.0))
    step = (frequency * 2.0 * math.pi) / samplerate
    out: list[float] = []
    for _ in range(frames):
        theta += step
        amplitude = math.sin(theta) * gain * ease_gain
        out.extend([amplitude] * channels)
        if ease and ease_gain < 1.0:
            ease_gain = min(1.0, ease_gain + ease_step)
    return out


def highpass(samples: list[float], samplerate: int, channels: int, cut: float) -> list[float]:
    dt = 1.0 / samplerate
    rc = 1.0 / (2.0 * math.pi * cut)
    alpha = rc / (rc + dt)
    states = [0.0] * channels
    last_inputs = [0.0] * channels
    out = [0.0] * len(samples)
    frames = len(samples) // channels
    for ch in range(channels):
        input_diff = samples[ch] - last_inputs[ch]
        out[ch] = alpha * (states[ch] + input_diff)
    for frame in range(1, frames):
        for ch in range(channels):
            idx = frame * channels + ch
            input_diff = samples[idx] - samples[idx - channels]
            out[idx] = alpha * (out[idx - channels] + input_diff)
    return out


def lowpass(samples: list[float], samplerate: int, channels: int, cut: float) -> list[float]:
    dt = 1.0 / samplerate
    rc = 1.0 / (2.0 * math.pi * cut)
    alpha = dt / (rc + dt)
    states = [0.0] * channels
    out = [0.0] * len(samples)
    frames = len(samples) // channels
    one_minus_alpha = 1.0 - alpha
    for ch in range(channels):
        out[ch] = alpha * samples[ch] + one_minus_alpha * states[ch]
    for frame in range(1, frames):
        for ch in range(channels):
            idx = frame * channels + ch
            out[idx] = alpha * samples[idx] + one_minus_alpha * out[idx - channels]
    return out


def agc(samples: list[float], samplerate: int, channels: int, target_level: float, max_gain: float) -> list[float]:
    trigger_level = 0.003
    target_linear = 10.0 ** (target_level / 10.0)
    max_gain_linear = 10.0 ** (max_gain / 10.0)
    attack_time = 0.0001
    release_time = 0.002
    hold_time = 0.001
    attack_coeff = 1.0 - math.exp(-1.0 / (attack_time * samplerate))
    release_coeff = 1.0 - math.exp(-1.0 / (release_time * samplerate))
    hold_samples = int(hold_time * samplerate)
    hold_counter = 0
    current_gain = [1.0] * channels
    out = list(samples)
    frames = len(samples) // channels
    block_target = int((frames / samplerate) / 0.01)
    block_size = max(1, frames // max(1, block_target))
    for block_start in range(0, frames, block_size):
        block_end = min(block_start + block_size, frames)
        block_samples = block_end - block_start
        for ch in range(channels):
            sum_squares = 0.0
            for frame in range(block_start, block_end):
                sample = out[frame * channels + ch]
                sum_squares += sample * sample
            rms = math.sqrt(sum_squares / block_samples)
            if rms > 1e-9 and rms > trigger_level:
                target_gain = min(target_linear / rms, max_gain_linear)
            else:
                target_gain = current_gain[ch]
            if target_gain < current_gain[ch]:
                current_gain[ch] = attack_coeff * target_gain + (1.0 - attack_coeff) * current_gain[ch]
                hold_counter = hold_samples
            elif hold_counter > 0:
                hold_counter = max(0, hold_counter - block_samples)
            else:
                current_gain[ch] = release_coeff * target_gain + (1.0 - release_coeff) * current_gain[ch]
            for frame in range(block_start, block_end):
                out[frame * channels + ch] *= current_gain[ch]
    peak_limit = 0.75
    frames = len(out) // channels
    for ch in range(channels):
        peak = max(abs(out[frame * channels + ch]) for frame in range(frames))
        if peak > peak_limit:
            scale = peak_limit / peak
            for frame in range(frames):
                out[frame * channels + ch] *= scale
    return out


def main() -> None:
    source_commit = subprocess.check_output(["git", "-C", str(UPSTREAM), "rev-parse", "HEAD"], text=True).strip()
    input_samples = [0.0, 0.25, -0.5, 0.75, 0.5, -0.25, 0.125, -0.125]
    samplerate = 8000
    channels = 1
    high = highpass(input_samples, samplerate, channels, 300.0)
    low = lowpass(input_samples, samplerate, channels, 1200.0)
    band = lowpass(highpass(input_samples, samplerate, channels, 300.0), samplerate, channels, 1200.0)
    agc_input = [0.05] * 80 + [0.4] * 80
    agc_out = agc(agc_input, samplerate, channels, -12.0, 12.0)

    lines = [
        "// @generated by scripts/generate-dsp-fixtures.py; do not edit by hand.",
        "#[allow(clippy::excessive_precision)]",
        "UpstreamDspFixture {",
        f'    source_commit: "{source_commit}",',
        "    tone_plain: DspCase { samplerate: 8000, channels: 2, samples: " + rust_samples(tone_samples(1000.0, 8000, 2, 0.5, 8, False)) + " },",
        "    tone_eased: DspCase { samplerate: 8000, channels: 1, samples: " + rust_samples(tone_samples(1000.0, 8000, 1, 0.5, 8, True)) + " },",
        "    filter_input: DspCase { samplerate: 8000, channels: 1, samples: " + rust_samples(input_samples) + " },",
        "    highpass_300hz: DspCase { samplerate: 8000, channels: 1, samples: " + rust_samples(high) + " },",
        "    lowpass_1200hz: DspCase { samplerate: 8000, channels: 1, samples: " + rust_samples(low) + " },",
        "    bandpass_300_1200hz: DspCase { samplerate: 8000, channels: 1, samples: " + rust_samples(band) + " },",
        "    agc_input: DspCase { samplerate: 8000, channels: 1, samples: " + rust_samples(agc_input) + " },",
        "    agc_output: DspCase { samplerate: 8000, channels: 1, samples: " + rust_samples(agc_out) + " },",
        "}",
        "",
    ]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines))


if __name__ == "__main__":
    main()
