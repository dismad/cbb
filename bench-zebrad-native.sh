#!/bin/bash
ENDPOINT="zebrad=http://127.0.0.1:9069"
START=2000000
STEPS=50
CHUNK=100
OUTDIR="results"
mkdir -p "$OUTDIR"
for CONC in 50 100 150 200; do
  for PASS in 1 2; do
    LABEL="pass${PASS}"
    [ "$PASS" = "2" ] && LABEL="pass2-warm"
    echo "======== concurrency=$CONC ($LABEL) ========"
    # human-readable
    ./target/release/cbb \
      --endpoint "$ENDPOINT" \
      --concurrency $CONC \
      --steps $STEPS \
      --chunk $CHUNK \
      --start-height $START
    # JSON for graphing
    ./target/release/cbb \
      --endpoint "$ENDPOINT" \
      --concurrency $CONC \
      --steps $STEPS \
      --chunk $CHUNK \
      --start-height $START \
      --json > "$OUTDIR/zebrad-${CONC}-${LABEL}.json"
    echo
  done
done
echo "JSON results saved in $OUTDIR/"
ls -la "$OUTDIR"/zebrad-*.json