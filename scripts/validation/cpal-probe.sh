#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/validation/results/cpal-probe"
mkdir -p "$(dirname "$OUT")"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CMD="cargo run -p rnphone -- --list-devices"
if [[ "${RUN_CPAL_PROBE:-0}" != "1" ]]; then
  STATUS="skipped"
  NOTE="Set RUN_CPAL_PROBE=1 on a host with intended audio devices to enumerate CPAL devices."
  printf '{"name":"cpal-probe","status":"%s","started_at":"%s","command":"%s","note":"%s"}\n' "$STATUS" "$STARTED_AT" "$CMD" "$NOTE" > "$OUT.json"
  printf '# CPAL probe\n\n- Status: %s\n- Command: `%s`\n- Note: %s\n' "$STATUS" "$CMD" "$NOTE" > "$OUT.md"
  exit 0
fi
(cd "$ROOT" && eval "$CMD")
CODE=$?
if [[ $CODE -eq 0 ]]; then STATUS="passed"; else STATUS="failed"; fi
printf '{"name":"cpal-probe","status":"%s","started_at":"%s","command":"%s","exit_code":%d}\n' "$STATUS" "$STARTED_AT" "$CMD" "$CODE" > "$OUT.json"
printf '# CPAL probe\n\n- Status: %s\n- Command: `%s`\n- Exit code: %d\n' "$STATUS" "$CMD" "$CODE" > "$OUT.md"
exit "$CODE"
