#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/validation/results/packet-media-roundtrip"
UPSTREAM="${UPSTREAM_LXST:-/home/lelloman/lxst}"
mkdir -p "$(dirname "$OUT")"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
COMMANDS=(
  "python3 scripts/generate-upstream-fixtures.py $UPSTREAM"
  "cargo test -p lxst-core --test interop"
  "cargo test -p lxst --test audio_codec_tests opus_decodes_generated_upstream_encoded_fixtures"
  "cargo test -p lxst --test media_tests ogg_opus_file_sink_and_source_round_trip_audio"
)
if [[ ! -d "$UPSTREAM" ]]; then
  STATUS="skipped"
  NOTE="Upstream LXST checkout not found at $UPSTREAM."
  printf '{"name":"packet-media-roundtrip","status":"%s","started_at":"%s","upstream":"%s","note":"%s"}\n' "$STATUS" "$STARTED_AT" "$UPSTREAM" "$NOTE" > "$OUT.json"
  printf '# Packet/media round trip\n\n- Status: %s\n- Upstream: `%s`\n- Note: %s\n' "$STATUS" "$UPSTREAM" "$NOTE" > "$OUT.md"
  exit 0
fi
CODE=0
for command in "${COMMANDS[@]}"; do
  (cd "$ROOT" && eval "$command") || { CODE=$?; break; }
done
if [[ $CODE -eq 0 ]]; then STATUS="passed"; else STATUS="failed"; fi
printf '{"name":"packet-media-roundtrip","status":"%s","started_at":"%s","upstream":"%s","exit_code":%d}\n' "$STATUS" "$STARTED_AT" "$UPSTREAM" "$CODE" > "$OUT.json"
printf '# Packet/media round trip\n\n- Status: %s\n- Upstream: `%s`\n- Exit code: %d\n\nCommands:\n' "$STATUS" "$UPSTREAM" "$CODE" > "$OUT.md"
for command in "${COMMANDS[@]}"; do printf -- '- `%s`\n' "$command" >> "$OUT.md"; done
exit "$CODE"
