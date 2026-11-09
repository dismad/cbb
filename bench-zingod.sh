#!/bin/bash
# Usage: ./bench-zingod.sh [--blockrange]
#   --blockrange   only measure GetBlockRange (skip GetTreeState)

ENDPOINT="zingod=https://127.0.0.1:8137"
START=2000000
STEPS=50
CHUNK=100
OUTDIR="results"
CA_FILE="zingod-ca.pem"
mkdir -p "$OUTDIR"

# one-time: save zingod's self-signed cert if missing
if [ ! -f "$CA_FILE" ]; then
  echo "Extracting zingod TLS cert to $CA_FILE..."
  echo | openssl s_client -connect 127.0.0.1:8137 -servername localhost 2>/dev/null \
    | openssl x509 -outform PEM > "$CA_FILE"
  if [ ! -s "$CA_FILE" ]; then
    echo "error: failed to extract cert. Is zingod running on 8137?"
    exit 1
  fi
  echo "Saved $CA_FILE"
fi

BLOCKRANGE=0
TAG=""
EXTRA_ARGS=()
if [ "${1:-}" = "--blockrange" ]; then
  BLOCKRANGE=1
  TAG="-blockrange"
  EXTRA_ARGS=(--tree-every 0)
  echo "Mode: GetBlockRange only"
fi

for CONC in 50 100 150 200; do
  for PASS in 1 2; do
    LABEL="pass${PASS}"
    [ "$PASS" = "2" ] && LABEL="pass2-warm"
    echo "======== concurrency=$CONC ($LABEL) ========"

    # human-readable
    ./target/release/cbb \
      --endpoint "$ENDPOINT" \
      --tls-ca "$CA_FILE" \
      --concurrency $CONC \
      --steps $STEPS \
      --chunk $CHUNK \
      --start-height $START \
      "${EXTRA_ARGS[@]}"

    # JSON for graphing
    ./target/release/cbb \
      --endpoint "$ENDPOINT" \
      --tls-ca "$CA_FILE" \
      --concurrency $CONC \
      --steps $STEPS \
      --chunk $CHUNK \
      --start-height $START \
      "${EXTRA_ARGS[@]}" \
      --json > "$OUTDIR/zingod${TAG}-${CONC}-${LABEL}.json"

    echo
  done
done

echo "JSON results saved in $OUTDIR/"
ls -la "$OUTDIR"/zingod${TAG}-*.json
