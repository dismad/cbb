#!/bin/bash
# Usage: [NAME=<name>] ./bench-zainod.sh [--blockrange | --tx]
#   (no flag)      runs GetBlockRange and GetTreeState
#   --blockrange   runs GetBlockRange only; skips GetTreeState
#   --tx           runs GetTransaction only; skips GetBlockRange and GetTreeState
#   NAME=<name>    names the results; letters, digits and _ only (default: zainod)

NAME="${NAME:-zainod}"
if [[ ! "$NAME" =~ ^[a-zA-Z0-9_]+$ ]]; then
  echo "error: NAME must be letters, digits and _ only (got '$NAME')" >&2
  exit 1
fi

ENDPOINT="$NAME=http://127.0.0.1:8137"
START=2000000
STEPS=50
CHUNK=100
OUTDIR="results"
mkdir -p "$OUTDIR"

BLOCKRANGE=0
TAG=""
EXTRA_ARGS=()
if [ "${1:-}" = "--blockrange" ]; then
  BLOCKRANGE=1
  TAG="-blockrange"
  EXTRA_ARGS=(--tree-every 0)
  echo "Mode: GetBlockRange only"
elif [ "${1:-}" = "--tx" ]; then
  TAG="-tx"
  EXTRA_ARGS=(--tx)
  echo "Mode: GetTransaction only"
fi

for CONC in 50 100 150 200; do
  for PASS in 1 2; do
    LABEL="pass${PASS}"
    [ "$PASS" = "2" ] && LABEL="pass2-warm"
    echo "======== concurrency=$CONC ($LABEL) ========"

    # Print and save the same run
    ./target/release/cbb \
      --endpoint "$ENDPOINT" \
      --concurrency $CONC \
      --steps $STEPS \
      --chunk $CHUNK \
      --start-height $START \
      "${EXTRA_ARGS[@]}" \
      --json-out "$OUTDIR/${NAME}${TAG}-${CONC}-${LABEL}.json"

    echo
  done
done

echo "JSON results saved in $OUTDIR/"
ls -la "$OUTDIR"/${NAME}${TAG}-*.json
