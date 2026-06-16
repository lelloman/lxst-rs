#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/validation/results/python-rust-signalling-smoke"
UPSTREAM="${UPSTREAM_LXST:-/home/lelloman/lxst}"
mkdir -p "$(dirname "$OUT")"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CMD="cargo test -p lxst-core --test interop"
if [[ ! -d "$UPSTREAM" ]]; then
  STATUS="skipped"
  NOTE="Upstream LXST checkout not found at $UPSTREAM."
  printf '{"name":"python-rust-signalling-smoke","status":"%s","started_at":"%s","upstream":"%s","note":"%s"}\n' "$STATUS" "$STARTED_AT" "$UPSTREAM" "$NOTE" > "$OUT.json"
  printf '# Python LXST <-> Rust signalling smoke\n\n- Status: %s\n- Upstream: `%s`\n- Note: %s\n' "$STATUS" "$UPSTREAM" "$NOTE" > "$OUT.md"
  exit 0
fi
(cd "$ROOT" && $CMD)
CODE=$?
if [[ $CODE -eq 0 ]]; then STATUS="passed"; else STATUS="failed"; fi
printf '{"name":"python-rust-signalling-smoke","status":"%s","started_at":"%s","upstream":"%s","command":"%s","exit_code":%d}\n' "$STATUS" "$STARTED_AT" "$UPSTREAM" "$CMD" "$CODE" > "$OUT.json"
printf '# Python LXST <-> Rust signalling smoke\n\n- Status: %s\n- Upstream: `%s`\n- Command: `%s`\n- Exit code: %d\n' "$STATUS" "$UPSTREAM" "$CMD" "$CODE" > "$OUT.md"
exit "$CODE"
