#!/bin/bash
# OIDC complete-chain drill (R2+): boot the real gateway with a file:// JWKS,
# then assert a JWKS-verified Bearer token authenticates while wrong-issuer /
# forged-signature tokens are rejected. Self-contained: embeds a long-lived
# HS256 fixture (secret "drill-secret-0123456789abcdef").
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${1:-target/debug/ontolith-server}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/jwks.json" <<'JSON'
{"keys": [{"kty": "oct", "kid": "drill-1", "alg": "HS256", "k": "ZHJpbGwtc2VjcmV0LTAxMjM0NTY3ODlhYmNkZWY"}]}
JSON

GOOD='eyJhbGciOiJIUzI1NiIsImtpZCI6ImRyaWxsLTEifQ.eyJpc3MiOiJodHRwczovL2lkcC5leGFtcGxlIiwiYXVkIjoib250b2xpdGgtc2VydmVyIiwidGVuYW50IjoiYWNtZSIsInN1YiI6InUtNDIiLCJleHAiOjIxMDE1NjI1Nzl9.w63HBhmD-NeX0I3gL0xRlbtlr28gB-DwwuSKmZjoBVM'
WRONG_ISS='eyJhbGciOiJIUzI1NiIsImtpZCI6ImRyaWxsLTEifQ.eyJpc3MiOiJodHRwczovL2V2aWwuZXhhbXBsZSIsImF1ZCI6Im9udG9saXRoLXNlcnZlciIsInRlbmFudCI6ImFjbWUiLCJzdWIiOiJ1LTQyIiwiZXhwIjoyMTAxNTYyNTkyfQ.wJrfCjXc3v8Oh2e_vQzx1tPYRPyBcGPQA9mA5o_Quys'

PORT=${ONTOLITH_DRILL_PORT:-18080}
GRPC_PORT=${ONTOLITH_DRILL_GRPC_PORT:-15051}

export ONTOLITH_BIND="127.0.0.1:$PORT"
export ONTOLITH_GRPC_BIND="127.0.0.1:$GRPC_PORT"
export ONTOLITH_AUTH_MODE=enforced
export ONTOLITH_OIDC_JWKS_URL="file://$WORK/jwks.json"
export ONTOLITH_OIDC_ISSUER=https://idp.example
export ONTOLITH_OIDC_AUDIENCE=ontolith-server
export ONTOLITH_OIDC_CACHE_TTL_SECS=60

"$BIN" > "$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; rm -rf "$WORK"' EXIT
sleep 2

code_good=$(curl -s -o "$WORK/good.json" -w "%{http_code}" \
  -H "Authorization: Bearer $GOOD" "http://127.0.0.1:$PORT/health")
code_bad_iss=$(curl -s -o "$WORK/bad-iss.json" -w "%{http_code}" \
  -H "Authorization: Bearer $WRONG_ISS" "http://127.0.0.1:$PORT/health")
code_bad_sig=$(curl -s -o "$WORK/bad-sig.json" -w "%{http_code}" \
  -H "Authorization: Bearer ${GOOD%?}A" "http://127.0.0.1:$PORT/health")

echo "== valid OIDC bearer: HTTP $code_good =="
cat "$WORK/good.json"; echo
echo "== tampered issuer:  HTTP $code_bad_iss =="
cat "$WORK/bad-iss.json"; echo
echo "== tampered signature: HTTP $code_bad_sig =="
cat "$WORK/bad-sig.json"; echo

[ "$code_good" = "200" ] || { echo "FAIL: expected 200 for valid token"; exit 1; }
grep -q '"oidc":"on"' "$WORK/good.json" || { echo "FAIL: health must report oidc:on"; exit 1; }
[ "$code_bad_iss" = "401" ] || { echo "FAIL: expected 401 for wrong issuer"; exit 1; }
[ "$code_bad_sig" = "401" ] || { echo "FAIL: expected 401 for forged signature"; exit 1; }

echo "=== OIDC DRILL PASS ==="
