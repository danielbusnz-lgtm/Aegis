#!/usr/bin/env bash
# Behavioral test for the two usage tiers, run against a local wrangler dev.
#
# Usage:
#   ./scripts/test-tiers.sh
#
# It starts `wrangler dev --local`, then drives the metered endpoint with fresh
# device ids to prove each tier's lifetime call cap is enforced exactly:
#   - trial (no invite code): cap = TRIAL_TURNS_CAP from wrangler.toml
#   - recruiter (minted code): cap = uses * 3
#
# Why this works without provider secrets: every metered endpoint charges a
# turn BEFORE it calls the upstream provider. With no secret the upstream call
# 401s, but the turn is already spent, so the only thing that matters here is
# whether the request got past the gate (any non-429) or was capped (429).

set -uo pipefail
cd "$(dirname "$0")/.."

PORT=8799
URL="http://localhost:${PORT}"
# Single metered endpoint is enough; it consumes one turn per call.
ENDPOINT="${URL}/v1/cartesia/token"
PASS=0
FAIL=0

uuid() { cat /proc/sys/kernel/random/uuid; }

# POST one metered call, echo the HTTP status. $1 = device id, $2 = invite code
# (optional).
hit() {
    local device="$1" code="${2:-}"
    if [[ -n "$code" ]]; then
        curl -s -o /dev/null -w "%{http_code}" -X POST "$ENDPOINT" \
            -H "x-aegis-device-id: ${device}" -H "x-aegis-invite-code: ${code}"
    else
        curl -s -o /dev/null -w "%{http_code}" -X POST "$ENDPOINT" \
            -H "x-aegis-device-id: ${device}"
    fi
}

check() {
    local label="$1" got="$2" want="$3"
    if [[ "$got" == "$want" ]]; then
        echo "  PASS  ${label} (${got})"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  ${label}: got ${got}, want ${want}"
        FAIL=$((FAIL + 1))
    fi
}

# Drive `cap` calls (expect all admitted) then one more (expect 429), and
# confirm the 429 body carries the expected error code. $1 label, $2 cap,
# $3 expected error, $4 device, $5 invite (optional).
assert_cap() {
    local label="$1" cap="$2" want_err="$3" device="$4" code="${5:-}"

    local i status admitted=1
    for ((i = 1; i <= cap; i++)); do
        status="$(hit "$device" "$code")"
        [[ "$status" != "429" ]] || { admitted=0; break; }
    done
    check "${label}: ${cap} calls admitted" "$admitted" "1"

    status="$(hit "$device" "$code")"
    check "${label}: call $((cap + 1)) capped" "$status" "429"

    local body
    if [[ -n "$code" ]]; then
        body="$(curl -s -X POST "$ENDPOINT" -H "x-aegis-device-id: ${device}" -H "x-aegis-invite-code: ${code}")"
    else
        body="$(curl -s -X POST "$ENDPOINT" -H "x-aegis-device-id: ${device}")"
    fi
    if grep -q "\"${want_err}\"" <<<"$body"; then
        echo "  PASS  ${label}: error is ${want_err}"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  ${label}: expected error ${want_err}, got ${body}"
        FAIL=$((FAIL + 1))
    fi
}

# ── boot a local worker, tear it down on exit ──────────────────────────────
LOG="$(mktemp)"
setsid npx wrangler dev --port "$PORT" --local >"$LOG" 2>&1 &
WD_PGID=$!
# Kill the whole process group (wrangler spawns a detached workerd child that a
# plain kill on the parent leaves behind). TERM, then KILL anything left.
cleanup() {
    kill -TERM -"$WD_PGID" 2>/dev/null
    sleep 0.5
    kill -KILL -"$WD_PGID" 2>/dev/null
}
trap cleanup EXIT

echo "starting wrangler dev on :${PORT} ..."
for _ in $(seq 1 60); do
    grep -q "Ready on" "$LOG" && break
    sleep 1
done
grep -q "Ready on" "$LOG" || { echo "wrangler dev never came up:"; tail -20 "$LOG"; exit 1; }

TRIAL_CAP="$(grep -E '^TRIAL_TURNS_CAP' wrangler.toml | grep -oE '[0-9]+')"

echo
echo "── trial tier (no code, cap=${TRIAL_CAP}) ──"
assert_cap "trial" "$TRIAL_CAP" "trial_exhausted" "$(uuid)"

echo
echo "── recruiter tier (10-use code, cap=30) ──"
MINT_OUT="$(bash scripts/mint-code.sh TESTTIER 10 5 --local 2>&1)"
CODE="$(grep -E '^Code:' <<<"$MINT_OUT" | awk '{print $2}')"
[[ -n "$CODE" ]] || { echo "mint failed:"; echo "$MINT_OUT"; exit 1; }
echo "  minted ${CODE}"
DEV_A="$(uuid)"
assert_cap "recruiter" 30 "code_exhausted" "$DEV_A" "$CODE"

echo
echo "── per-device counter (same code, new device) ──"
# A second device under the same code must start fresh, proving the counter is
# keyed per (code, device), not per code. Any non-429 means it got past the gate.
DEV_B_STATUS="$(hit "$(uuid)" "$CODE")"
if [[ "$DEV_B_STATUS" != "429" ]]; then
    echo "  PASS  recruiter: device B admitted (${DEV_B_STATUS})"
    PASS=$((PASS + 1))
else
    echo "  FAIL  recruiter: device B capped (429), counter not per-device"
    FAIL=$((FAIL + 1))
fi

echo
echo "── bad device id rejected ──"
check "missing device id 401" \
    "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$ENDPOINT")" "401"

echo
echo "${PASS} passed, ${FAIL} failed"
[[ "$FAIL" -eq 0 ]]
