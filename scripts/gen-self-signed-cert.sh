#!/usr/bin/env bash
# Generate a self-signed certificate/key pair for ontolith-management-server
# in-process TLS termination (ONTOLITH_TLS_CERT / ONTOLITH_TLS_KEY).
#
# Usage:
#   scripts/gen-self-signed-cert.sh [--days 365] [--cn localhost] OUT_DIR
#   # then configure deployments/ontolith-management.user.env with the two paths.
set -euo pipefail

DAYS=365
CN="localhost"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --days) DAYS="$2"; shift 2 ;;
    --cn) CN="$2"; shift 2 ;;
    *) OUT_DIR="$1"; shift ;;
  esac
done

if [[ -z "${OUT_DIR:-}" ]]; then
  echo "usage: $0 [--days N] [--cn NAME] OUT_DIR" >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"
CERT="${OUT_DIR}/ontolith.crt.pem"
KEY="${OUT_DIR}/ontolith.key.pem"

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${KEY}" -out "${CERT}" \
  -days "${DAYS}" -subj "/CN=${CN}" \
  -addext "subjectAltName=DNS:${CN},IP:127.0.0.1,IP:::1"

chmod 600 "${KEY}"
echo "==> wrote ${CERT}"
echo "==> wrote ${KEY} (mode 600)"
echo "==> configure: ONTOLITH_TLS_CERT=${CERT} ONTOLITH_TLS_KEY=${KEY}"
