# cbb — Compact Block Bench

Realistic light-client load testing for Zcash CompactTxStreamer endpoints.

Compares **lightwalletd**, **zainod**, **zingod**, **zebrad native**, and other CompactTxStreamer servers under wallet-style traffic (concurrent `GetBlockRange` streams + occasional `GetTreeState`).

## Build

```bash
cd cbb
cargo build --release
```

## Usage

```bash
# Compare four endpoints
./target/release/cbb \
  --endpoint lwd=http://127.0.0.1:9067 \
  --endpoint zainod=http://127.0.0.1:8137 \
  --endpoint zingod=http://127.0.0.1:9068 \
  --endpoint native=http://127.0.0.1:8237 \
  --concurrency 50 \
  --chunk 100 \
  --steps 20 \
  --start-height 2000000

# JSON output for graphing
./target/release/cbb --json --endpoint lwd=http://127.0.0.1:9067 \
  --concurrency 100 --json-out results/lwd-100-pass2-warm.json
```

### Flags

| Flag                  | Default | Description                                            |
| --------------------- | ------- | ------------------------------------------------------ |
| `--endpoint name=url` | —       | Repeatable. At least one required.                     |
| `--concurrency`       | 50      | Number of virtual wallets                              |
| `--chunk`             | 100     | Blocks per `GetBlockRange`                             |
| `--steps`             | 20      | Sequential range requests per wallet                   |
| `--start-height`      | 2000000 | Starting height (wallets are offset)                   |
| `--tree-every`        | 5       | Call `GetTreeState` every N range requests (0 = never) |
| `--timeout`           | 30      | Request timeout in seconds                             |
| `--warmup`            | 2       | Warm-up requests per wallet (not counted)              |
| `--json`              | false   | Emit JSON summary to stdout                            |
| `--json-out PATH`     | —       | Also write the JSON summary to a file                  |
| `--tls-ca PATH`       | —       | CA cert PEM to trust (e.g. self-signed zingod)         |

## What it measures

- `GetBlockRange` latency (full stream drain)
- Occasional `GetTreeState`
- Error rate and timeouts
- p50 / p95 / p99 / max
- Per-method breakdown
- Throughput (requests/sec) and wall time

JSON summaries are suitable for the graphing scripts below.

## Graphing

Two Python scripts produce comparison plots from the JSON results:

```bash
# Line plots (pass2-warm preferred)
python3 graph.py

# Grouped cold vs warm bar charts
python3 cold-warm-bars.py
```

Both write to `graphs/` and generate linear + log-scale versions (`*_log.png`) with fixed distinct colors per service.

Expected result filename pattern: `{service}-{concurrency}-pass2-warm.json` (and optionally `pass1` for cold).

## Notes

- All endpoints should be pointed at the **same** underlying zebrad for a fair comparison.
- First run after indexer start is cold; re-run for warm-cache numbers (`pass2-warm`).
- Increase `--concurrency` and `--steps` to stress resource limits (gRPC stream limits, JSON-RPC connection caps, etc.).
- `zingod` often needs raised gRPC limits (`max_streams_per_connection`, `max_range_reads`) to stay error-free at high concurrency.
- `lightwalletd` benefits from `--grpc-max-concurrent-streams 0` and `--max-concurrent-blockrange 0` when the backend can handle the load.
- zebrad’s native CompactTxStreamer endpoint (`[rpc] lightwalletd_listen_addr`) works but is typically much slower than dedicated indexers under concurrent load.
