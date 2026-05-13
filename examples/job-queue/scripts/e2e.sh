#!/usr/bin/env bash
# E2E test: drive the same workflow contract through two unrelated consumers
# (jobctl CLI + axum HTTP API) and assert each can observe the other's actions.
# See SPEC.md §Test plan §3.
set -euo pipefail

BASE=${BASE:-http://localhost:3030}
JOBCTL=${JOBCTL:-cargo run -q -p jobctl --}

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }
fail() { printf '\033[1;31m%s\033[0m\n' "$*" >&2; exit 1; }

# ── Path A: submit via CLI, query via HTTP API ─────────────────────────
say "A: submit via jobctl"
WID_A=$($JOBCTL submit --name "lint" --command "cargo clippy" | sed -n 's/.*workflow_id=\(.*\)/\1/p')
[ -n "$WID_A" ] || fail "A: jobctl did not return a workflow_id"
say "A: workflow_id=$WID_A"

say "A: status via HTTP API"
STAGE_A=$(curl -fsS "$BASE/jobs/$WID_A" | python3 -c 'import json,sys; print(json.load(sys.stdin)["stage"])')
[ -n "$STAGE_A" ] || fail "A: HTTP API did not see workflow"
say "A: HTTP API reports stage=$STAGE_A"

# ── Path B: submit via HTTP API, cancel via CLI ────────────────────────
say "B: submit via HTTP API"
RESP=$(curl -fsS -X POST "$BASE/jobs" -H content-type:application/json \
        -d '{"name":"build","command":"cargo build","timeout_seconds":60}')
WID_B=$(echo "$RESP" | python3 -c 'import json,sys; print(json.load(sys.stdin)["workflow_id"])')
say "B: workflow_id=$WID_B"

say "B: cancel via jobctl"
$JOBCTL cancel "$WID_B" --reason "e2e test"

say "B: poll for cancelled stage"
for _ in $(seq 1 20); do
    STAGE=$($JOBCTL status "$WID_B" 2>/dev/null | sed -n 's/.*stage=\([^ ]*\).*/\1/p' || true)
    [ "$STAGE" = "cancelled" ] && break
    sleep 1
done
[ "${STAGE:-}" = "cancelled" ] || fail "B: workflow did not reach cancelled (last stage=$STAGE)"
say "B: ✓ stage=cancelled via jobctl"

# ── Path C: HTTP API also sees the cancelled state ─────────────────────
STAGE_C=$(curl -fsS "$BASE/jobs/$WID_B" | python3 -c 'import json,sys; print(json.load(sys.stdin)["stage"])')
[ "$STAGE_C" = "cancelled" ] || fail "C: HTTP API saw stage=$STAGE_C, expected cancelled"
say "C: ✓ HTTP API also sees stage=cancelled"

# ── Path D: workflow A completes naturally ─────────────────────────────
say "D: wait for A (poll-based; macOS lacks coreutils timeout)"
DEADLINE=$((SECONDS + 25))
while [ $SECONDS -lt $DEADLINE ]; do
    STAGE_A=$($JOBCTL status "$WID_A" 2>/dev/null | sed -n 's/.*stage=\([^ ]*\).*/\1/p' || true)
    [ "$STAGE_A" = "done" ] && break
    sleep 1
done
[ "${STAGE_A:-}" = "done" ] || fail "D: A did not reach done (last stage=$STAGE_A)"
OUT=$($JOBCTL wait "$WID_A")
echo "$OUT" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["exit_code"]==0, d' >/dev/null
say "D: ✓ A completed with exit_code=0"

say "✓ all four paths passed"
