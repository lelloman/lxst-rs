#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/validation/results/rnphone-two-node-smoke"
mkdir -p "$(dirname "$OUT")"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CMD="cargo test -p lxst --test network_tests reticulum_loopback_completes_telephony_call_flow"
if [[ "${RUN_LIVE_RNS:-0}" != "1" ]]; then
  STATUS="skipped"
  NOTE="Set RUN_LIVE_RNS=1 after configuring two local Reticulum/rnphone nodes for a live smoke. Ran no live network action."
  printf '{"name":"rnphone-two-node-smoke","status":"%s","started_at":"%s","command":"%s","note":"%s"}\n' "$STATUS" "$STARTED_AT" "$CMD" "$NOTE" > "$OUT.json"
  printf '# rnphone two-node smoke\n\n- Status: %s\n- Command: `%s`\n- Note: %s\n' "$STATUS" "$CMD" "$NOTE" > "$OUT.md"
  exit 0
fi
(cd "$ROOT" && eval "$CMD")
CODE=$?
if [[ $CODE -eq 0 ]]; then STATUS="passed"; else STATUS="failed"; fi
printf '{"name":"rnphone-two-node-smoke","status":"%s","started_at":"%s","command":"%s","exit_code":%d}\n' "$STATUS" "$STARTED_AT" "$CMD" "$CODE" > "$OUT.json"
printf '# rnphone two-node smoke\n\n- Status: %s\n- Command: `%s`\n- Exit code: %d\n' "$STATUS" "$CMD" "$CODE" > "$OUT.md"
exit "$CODE"
