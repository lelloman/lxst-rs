#!/usr/bin/env python3
"""Generate dependency-free fixtures from the upstream Python LXST checkout."""

from __future__ import annotations

import ast
import importlib
import math
import subprocess
import sys
import types
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = Path(sys.argv[1]).expanduser() if len(sys.argv) > 1 else Path("~/lxst").expanduser()
OUT = ROOT / "lxst-core" / "tests" / "fixtures" / "upstream_core.rs"
OPUS_OUT = ROOT / "lxst" / "tests" / "fixtures" / "upstream_opus.rs"


def parse(relative: str) -> ast.Module:
    return ast.parse((UPSTREAM / relative).read_text(), filename=relative)


def class_node(module: ast.Module, name: str) -> ast.ClassDef:
    for node in module.body:
        if isinstance(node, ast.ClassDef) and node.name == name:
            return node
    raise KeyError(name)


def func_node(cls: ast.ClassDef, name: str) -> ast.FunctionDef:
    for node in cls.body:
        if isinstance(node, ast.FunctionDef) and node.name == name:
            return node
    raise KeyError(name)


def module_constants(module: ast.Module) -> dict[str, int]:
    values: dict[str, int] = {}
    for node in module.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            try:
                value = ast.literal_eval(node.value)
            except ValueError:
                continue
            if isinstance(value, int):
                values[node.targets[0].id] = value
    return values


def eval_literal(node: ast.AST, names: dict[str, object]) -> object:
    if isinstance(node, ast.Constant):
        return node.value
    if isinstance(node, ast.Name):
        return names[node.id]
    if isinstance(node, ast.List):
        return [eval_literal(item, names) for item in node.elts]
    if isinstance(node, ast.Tuple):
        return tuple(eval_literal(item, names) for item in node.elts)
    if isinstance(node, ast.Dict):
        return {eval_literal(key, names): eval_literal(value, names) for key, value in zip(node.keys, node.values)}
    raise ValueError(ast.dump(node))


def class_constants(cls: ast.ClassDef) -> dict[str, object]:
    values: dict[str, object] = {}
    for node in cls.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            try:
                values[node.targets[0].id] = eval_literal(node.value, values)
            except (KeyError, ValueError):
                pass
    return values


def attr_name(node: ast.AST) -> str:
    if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
        return node.attr
    raise TypeError(ast.dump(node))


def const_return_map(fn: ast.FunctionDef) -> dict[str, object]:
    out: dict[str, object] = {}
    nodes = list(fn.body)
    while nodes:
        node = nodes.pop(0)
        if isinstance(node, ast.If):
            test = node.test
            if isinstance(test, ast.Compare) and len(test.ops) == 1 and isinstance(test.ops[0], ast.Eq):
                key = attr_name(test.comparators[0])
                if node.body and isinstance(node.body[0], ast.Return):
                    out[key] = ast.literal_eval(node.body[0].value)
            nodes = list(node.orelse) + nodes
    return out


def available_profiles(fn: ast.FunctionDef) -> list[str]:
    for node in ast.walk(fn):
        if isinstance(node, ast.Return) and isinstance(node.value, ast.List):
            return [attr_name(item) for item in node.value.elts]
    raise ValueError("available_profiles return list not found")


def codec2_mode_headers(cls: ast.ClassDef, constants: dict[str, object]) -> list[tuple[int, int]]:
    for name in ("MODE_HEADERS",):
        value = constants[name]
        if isinstance(value, dict):
            return sorted((int(mode), int(header)) for mode, header in value.items())
    raise ValueError("MODE_HEADERS not found")


def msgpack_uint(value: int) -> bytes:
    if value <= 0x7F:
        return bytes([value])
    if value <= 0xFF:
        return bytes([0xCC, value])
    if value <= 0xFFFF:
        return bytes([0xCD]) + value.to_bytes(2, "big")
    return bytes([0xCE]) + value.to_bytes(4, "big")


def msgpack_int(value: int) -> bytes:
    if value >= 0:
        return msgpack_uint(value)
    if value >= -32:
        return bytes([(0x100 + value) & 0xFF])
    if value >= -128:
        return bytes([0xD0, value & 0xFF])
    raise ValueError("fixture negative integer out of range")


def msgpack_str(value: str) -> bytes:
    encoded = value.encode("utf-8")
    if len(encoded) < 32:
        return bytes([0xA0 | len(encoded)]) + encoded
    if len(encoded) <= 0xFF:
        return bytes([0xD9, len(encoded)]) + encoded
    raise ValueError("fixture string too large")


def msgpack_bin(value: bytes) -> bytes:
    if len(value) <= 0xFF:
        return bytes([0xC4, len(value)]) + value
    return bytes([0xC5]) + len(value).to_bytes(2, "big") + value


def msgpack_array(values: list[bytes]) -> bytes:
    if len(values) >= 16:
        raise ValueError("fixture array too large for fixarray encoder")
    return bytes([0x90 | len(values)]) + b"".join(values)


def msgpack_map(entries: list[tuple[int, bytes]]) -> bytes:
    if len(entries) >= 16:
        raise ValueError("fixture map too large for fixmap encoder")
    return bytes([0x80 | len(entries)]) + b"".join(msgpack_uint(key) + value for key, value in entries)


def rust_string(value: object) -> str:
    return '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'


def rust_named_value(name: str, value: int) -> str:
    return f"NamedValue {{ name: {rust_string(name)}, value: {value} }}"


def upstream_opus_encoder_class():
    """Load the upstream vendored pyogg encoder without importing all LXST."""
    lxst = types.ModuleType("LXST")
    lxst.__path__ = [str(UPSTREAM / "LXST")]
    sys.modules.setdefault("LXST", lxst)

    codecs = types.ModuleType("LXST.Codecs")
    codecs.__path__ = [str(UPSTREAM / "LXST" / "Codecs")]
    sys.modules.setdefault("LXST.Codecs", codecs)

    libs = types.ModuleType("LXST.Codecs.libs")
    libs.__path__ = [str(UPSTREAM / "LXST" / "Codecs" / "libs")]
    sys.modules.setdefault("LXST.Codecs.libs", libs)

    return importlib.import_module("LXST.Codecs.libs.pyogg").OpusEncoder


def pcm_sine_bytes(samplerate: int, channels: int, frame_duration_ms: int) -> bytes:
    frames = samplerate * frame_duration_ms // 1000
    out = bytearray()
    for index in range(frames):
        for channel in range(channels):
            frequency = 220.0 if channel == 0 else 330.0
            amplitude = 0.2 if channel == 0 else 0.15
            sample = int(amplitude * math.sin(2.0 * math.pi * frequency * index / samplerate) * 32767.0)
            out.extend(sample.to_bytes(2, "little", signed=True))
    return bytes(out)


def opus_payload_fixtures(
    opus_constants: dict[str, object],
    opus_channels: dict[str, object],
    opus_samplerates: dict[str, object],
    opus_applications: dict[str, object],
    opus_bitrates: dict[str, object],
) -> list[dict[str, object]]:
    encoder_class = upstream_opus_encoder_class()
    cases = []
    for name in (
        "PROFILE_VOICE_LOW",
        "PROFILE_VOICE_MEDIUM",
        "PROFILE_VOICE_MAX",
        "PROFILE_AUDIO_LOW",
        "PROFILE_AUDIO_HIGH",
    ):
        samplerate = int(opus_samplerates[name])
        channels = int(opus_channels[name])
        frame_duration_ms = 20
        max_bytes = math.ceil((int(opus_bitrates[name]) / 8.0) * (frame_duration_ms / 1000.0))

        encoder = encoder_class()
        encoder.set_application(str(opus_applications[name]))
        encoder.set_max_bytes_per_frame(max_bytes)
        encoder.set_sampling_frequency(samplerate)
        encoder.set_channels(channels)
        payload = encoder.encode(pcm_sine_bytes(samplerate, channels, frame_duration_ms)).tobytes()

        cases.append(
            {
                "name": name,
                "value": int(opus_constants[name]),
                "samplerate": samplerate,
                "channels": channels,
                "frame_duration_ms": frame_duration_ms,
                "frame_count": samplerate * frame_duration_ms // 1000,
                "max_bytes": max_bytes,
                "hex": payload.hex(),
            }
        )
    return cases


def main() -> None:
    network = parse("LXST/Network.py")
    codecs = parse("LXST/Codecs/__init__.py")
    raw = class_node(parse("LXST/Codecs/Raw.py"), "Raw")
    opus = class_node(parse("LXST/Codecs/Opus.py"), "Opus")
    codec2 = class_node(parse("LXST/Codecs/Codec2.py"), "Codec2")
    telephony = class_node(parse("LXST/Primitives/Telephony.py"), "Profiles")
    signalling = class_node(parse("LXST/Primitives/Telephony.py"), "Signalling")

    fields = module_constants(network)
    codec_headers = module_constants(codecs)
    raw_constants = class_constants(raw)
    opus_constants = class_constants(opus)
    codec2_constants = class_constants(codec2)
    profile_constants = class_constants(telephony)
    signal_constants = class_constants(signalling)

    profile_order = available_profiles(func_node(telephony, "available_profiles"))
    profile_names = const_return_map(func_node(telephony, "profile_name"))
    profile_abbreviations = const_return_map(func_node(telephony, "profile_abbrevation"))
    profile_frame_times = const_return_map(func_node(telephony, "get_frame_time"))
    opus_channels = const_return_map(func_node(opus, "profile_channels"))
    opus_samplerates = const_return_map(func_node(opus, "profile_samplerate"))
    opus_applications = const_return_map(func_node(opus, "profile_application"))
    opus_bitrates = const_return_map(func_node(opus, "profile_bitrate_ceiling"))

    source_commit = subprocess.check_output(
        ["git", "-C", str(UPSTREAM), "rev-parse", "HEAD"], text=True
    ).strip()
    opus_payloads = opus_payload_fixtures(
        opus_constants,
        opus_channels,
        opus_samplerates,
        opus_applications,
        opus_bitrates,
    )

    raw_headers = []
    for bitdepth_name, bitdepth in (
        ("BITDEPTH_16", raw_constants["BITDEPTH_16"]),
        ("BITDEPTH_32", raw_constants["BITDEPTH_32"]),
        ("BITDEPTH_64", raw_constants["BITDEPTH_64"]),
        ("BITDEPTH_128", raw_constants["BITDEPTH_128"]),
    ):
        for channels in (1, 2, 32):
            raw_headers.append((bitdepth_name, int(bitdepth), channels, (int(bitdepth) << 6) | (channels - 1)))

    packets = [
        (
            "single_opus_frame",
            msgpack_map([(fields["FIELD_FRAMES"], msgpack_bin(bytes([codec_headers["OPUS"], 0xAA, 0xBB])))]),
        ),
        (
            "scalar_ringing_signal",
            msgpack_map([(fields["FIELD_SIGNALLING"], msgpack_uint(signal_constants["STATUS_RINGING"]))]),
        ),
        (
            "calling_with_preferred_medium_quality",
            msgpack_map(
                [
                    (
                        fields["FIELD_SIGNALLING"],
                        msgpack_array(
                            [
                                msgpack_uint(signal_constants["STATUS_CALLING"]),
                                msgpack_uint(signal_constants["PREFERRED_PROFILE"] + profile_constants["QUALITY_MEDIUM"]),
                            ]
                        ),
                    )
                ]
            ),
        ),
        (
            "mixed_raw_codec2_frames",
            msgpack_map(
                [
                    (
                        fields["FIELD_FRAMES"],
                        msgpack_array(
                            [
                                msgpack_bin(bytes([codec_headers["RAW"], 0x10])),
                                msgpack_bin(bytes([codec_headers["CODEC2"], 0x20])),
                            ]
                        ),
                    )
                ]
            ),
        ),
        (
            "empty_packet",
            msgpack_map([]),
        ),
        (
            "status_sequence_and_all_codecs",
            msgpack_map(
                [
                    (
                        fields["FIELD_SIGNALLING"],
                        msgpack_array(
                            [
                                msgpack_uint(signal_constants["STATUS_BUSY"]),
                                msgpack_uint(signal_constants["STATUS_REJECTED"]),
                                msgpack_uint(signal_constants["STATUS_AVAILABLE"]),
                                msgpack_uint(signal_constants["STATUS_ESTABLISHED"]),
                            ]
                        ),
                    ),
                    (
                        fields["FIELD_FRAMES"],
                        msgpack_array(
                            [
                                msgpack_bin(bytes([codec_headers["RAW"], 0x01, 0x02])),
                                msgpack_bin(bytes([codec_headers["OPUS"], 0x03])),
                                msgpack_bin(bytes([codec_headers["CODEC2"], 0x04, 0x05])),
                                msgpack_bin(bytes([codec_headers["NULL"], 0x06])),
                            ]
                        ),
                    ),
                ]
            ),
        ),
        (
            "unknown_field_is_ignored",
            msgpack_map(
                [
                    (99, msgpack_str("ignored")),
                    (fields["FIELD_SIGNALLING"], msgpack_uint(signal_constants["STATUS_AVAILABLE"])),
                ]
            ),
        ),
    ]

    malformed_packets = [
        ("root_array", msgpack_array([])),
        ("root_uint", msgpack_uint(1)),
        ("trailing_byte_after_empty_map", msgpack_map([]) + b"\x00"),
        (
            "negative_signal",
            msgpack_map([(fields["FIELD_SIGNALLING"], msgpack_int(-1))]),
        ),
        (
            "string_signal",
            msgpack_map([(fields["FIELD_SIGNALLING"], msgpack_str("ringing"))]),
        ),
        (
            "frame_not_binary",
            msgpack_map([(fields["FIELD_FRAMES"], msgpack_uint(1))]),
        ),
        (
            "empty_frame_binary",
            msgpack_map([(fields["FIELD_FRAMES"], msgpack_bin(b""))]),
        ),
        (
            "unknown_codec_header",
            msgpack_map([(fields["FIELD_FRAMES"], msgpack_bin(b"\x7f"))]),
        ),
        (
            "frame_array_contains_non_binary",
            msgpack_map(
                [
                    (
                        fields["FIELD_FRAMES"],
                        msgpack_array(
                            [
                                msgpack_bin(bytes([codec_headers["RAW"], 0x10])),
                                msgpack_uint(1),
                            ]
                        ),
                    )
                ]
            ),
        ),
    ]

    lines = [
        "// @generated by scripts/generate-upstream-fixtures.py; do not edit by hand.",
        "UpstreamCoreFixture {",
        f"    source_commit: {rust_string(source_commit)},",
        "    fields: FieldFixture {",
        f"        signalling: {fields['FIELD_SIGNALLING']},",
        f"        frames: {fields['FIELD_FRAMES']},",
        "    },",
        "    codec_headers: CodecHeaderFixture {",
        f"        raw: {codec_headers['RAW']},",
        f"        opus: {codec_headers['OPUS']},",
        f"        codec2: {codec_headers['CODEC2']},",
        f"        null: {codec_headers['NULL']},",
        "    },",
        "    signals: &[",
    ]
    for name in sorted(k for k, v in signal_constants.items() if isinstance(v, int)):
        lines.append(f"        {rust_named_value(name, signal_constants[name])},")
    lines += [
        "    ],",
        "    profiles: &[",
    ]
    for index, name in enumerate(profile_order):
        value = int(profile_constants[name])
        next_name = profile_order[(index + 1) % len(profile_order)]
        lines.append(
            "        ProfileFixture { "
            f"name: {rust_string(name)}, value: {value}, index: {index}, "
            f"display_name: {rust_string(profile_names[name])}, "
            f"abbreviation: {rust_string(profile_abbreviations[name])}, "
            f"frame_time_ms: {profile_frame_times[name]}, "
            f"next_value: {profile_constants[next_name]} }},"
        )
    lines += [
        "    ],",
        "    opus_profiles: &[",
    ]
    for name in sorted(k for k in opus_constants if k.startswith("PROFILE_")):
        value = int(opus_constants[name])
        lines.append(
            "        OpusProfileFixture { "
            f"name: {rust_string(name)}, value: {value}, "
            f"channels: {opus_channels[name]}, samplerate: {opus_samplerates[name]}, "
            f"application: {rust_string(opus_applications[name])}, "
            f"bitrate_ceiling: {opus_bitrates[name]} }},"
        )
    lines += [
        "    ],",
        "    codec2_modes: &[",
    ]
    for mode, header in codec2_mode_headers(codec2, codec2_constants):
        lines.append(f"        Codec2ModeFixture {{ mode: {mode}, header: {header} }},")
    lines += [
        "    ],",
        "    raw_frame_headers: &[",
    ]
    for bitdepth_name, bitdepth, channels, header in raw_headers:
        lines.append(
            "        RawFrameHeaderFixture { "
            f"bitdepth_name: {rust_string(bitdepth_name)}, bitdepth_value: {bitdepth}, "
            f"channels: {channels}, header: {header} }},"
        )
    lines += [
        "    ],",
        "    packet_cases: &[",
    ]
    for name, payload in packets:
        lines.append(f"        PacketFixture {{ name: {rust_string(name)}, hex: {rust_string(payload.hex())} }},")
    lines += [
        "    ],",
        "    malformed_packet_cases: &[",
    ]
    for name, payload in malformed_packets:
        lines.append(f"        PacketFixture {{ name: {rust_string(name)}, hex: {rust_string(payload.hex())} }},")
    lines += [
        "    ],",
        "}",
        "",
    ]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines))

    opus_lines = [
        "// @generated by scripts/generate-upstream-fixtures.py; do not edit by hand.",
        "UpstreamOpusFixture {",
        f"    source_commit: {rust_string(source_commit)},",
        "    cases: &[",
    ]
    for case in opus_payloads:
        opus_lines.append(
            "        OpusEncodedFixture { "
            f"profile_name: {rust_string(case['name'])}, "
            f"profile_value: {case['value']}, "
            f"samplerate: {case['samplerate']}, "
            f"channels: {case['channels']}, "
            f"frame_duration_ms: {case['frame_duration_ms']}, "
            f"frame_count: {case['frame_count']}, "
            f"max_bytes: {case['max_bytes']}, "
            f"hex: {rust_string(case['hex'])} }},"
        )
    opus_lines += [
        "    ],",
        "}",
        "",
    ]
    OPUS_OUT.parent.mkdir(parents=True, exist_ok=True)
    OPUS_OUT.write_text("\n".join(opus_lines))


if __name__ == "__main__":
    main()
