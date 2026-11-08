#!/usr/bin/env python3
"""Pretty-print all cbb JSON results as a table + deltas (colorized)."""

import json, sys, glob, re
from pathlib import Path
from collections import defaultdict

RESET = "\033[0m"
GREEN = "\033[32m"
RED = "\033[31m"
YELLOW = "\033[33m"
BOLD = "\033[1m"

def colorize(text, color):
    if not sys.stdout.isatty():
        return text
    return f"{color}{text}{RESET}"

def load(path):
    raw = Path(path).read_text()
    start = raw.find('[')
    if start == -1:
        start = raw.find('{')
    if start == -1:
        raise ValueError(f"No JSON found in {path}")
    return json.loads(raw[start:])

def parse_name(stem: str):
    m = re.match(r'^([a-zA-Z0-9]+)-(\d+)(?:-(.+))?$', stem)
    if not m:
        return (stem, 0, "")
    indexer, conc, variant = m.group(1), int(m.group(2)), m.group(3) or ""
    return (indexer, conc, variant)

def main():
    if len(sys.argv) > 1:
        files = sys.argv[1:]
    else:
        files = sorted(glob.glob("results/*.json"))

    if not files:
        print("No result files found in results/")
        return

    rows = []
    for f in files:
        try:
            data = load(f)
            summary = data[0] if isinstance(data, list) and data else data
        except Exception as e:
            print(f"skip {f}: {e}", file=sys.stderr)
            continue

        stem = Path(f).stem
        indexer, conc, variant = parse_name(stem)
        err_rate = summary.get("error_rate", 0) * 100  # fraction → percent
        rows.append({
            "indexer": indexer,
            "conc": conc,
            "variant": variant,
            "stem": stem,
            "error_rate": err_rate,
            "errors": summary.get("errors", 0),
            "p50": summary.get("p50", 0),
            "p95": summary.get("p95", 0),
            "p99": summary.get("p99", 0),
            "rps": summary.get("throughput_rps", 0),
            "wall": summary.get("wall_time_secs", 0),
        })

    if not rows:
        print("No valid results parsed")
        return

    rows.sort(key=lambda r: (r["indexer"], r["conc"], r["variant"]))

    print(colorize(f"{'run':<28} {'conc':>5} {'err%':>6} {'errs':>6} {'p50':>8} {'p95':>8} {'p99':>8} {'rps':>8} {'wall':>7}", BOLD))
    print("-" * 98)
    for r in rows:
        print(f"{r['stem']:<28} {r['conc']:>5} {r['error_rate']:>6.1f} {r['errors']:>6} "
              f"{r['p50']:>8.1f} {r['p95']:>8.1f} {r['p99']:>8.1f} "
              f"{r['rps']:>8.1f} {r['wall']:>7.1f}")

    groups = defaultdict(list)
    for r in rows:
        groups[(r["conc"], r["variant"])].append(r)

    print("\n" + "=" * 98)
    print(colorize("Deltas vs best (lower latency / higher rps is better)", BOLD))
    print("=" * 98)
    print(colorize(f"{'run':<28} {'conc':>5} {'p50 Δ%':>9} {'p95 Δ%':>9} {'p99 Δ%':>9} {'rps Δ%':>9}", BOLD))
    print("-" * 98)

    for key in sorted(groups.keys(), key=lambda k: (k[0], k[1])):
        group = groups[key]
        if len(group) < 2:
            continue
        best_p50 = min((r["p50"] for r in group if r["p50"] > 0), default=0)
        best_p95 = min((r["p95"] for r in group if r["p95"] > 0), default=0)
        best_p99 = min((r["p99"] for r in group if r["p99"] > 0), default=0)
        best_rps = max((r["rps"] for r in group if r["rps"] > 0), default=0)

        for r in sorted(group, key=lambda x: x["indexer"]):
            p50_d = ((r["p50"] - best_p50) / best_p50 * 100) if best_p50 else 0
            p95_d = ((r["p95"] - best_p95) / best_p95 * 100) if best_p95 else 0
            p99_d = ((r["p99"] - best_p99) / best_p99 * 100) if best_p99 else 0
            rps_d = ((r["rps"] - best_rps) / best_rps * 100) if best_rps else 0

            def style(val, lower_better=True):
                if abs(val) < 0.05:
                    return GREEN
                if lower_better:
                    if val > 100:
                        return RED
                    if val > 20:
                        return YELLOW
                else:
                    if val < -50:
                        return RED
                    if val < -10:
                        return YELLOW
                return ""

            p50_s = colorize(f"{p50_d:>+8.1f}%", style(p50_d, True))
            p95_s = colorize(f"{p95_d:>+8.1f}%", style(p95_d, True))
            p99_s = colorize(f"{p99_d:>+8.1f}%", style(p99_d, True))
            rps_s = colorize(f"{rps_d:>+8.1f}%", style(rps_d, False))

            print(f"{r['stem']:<28} {r['conc']:>5} {p50_s} {p95_s} {p99_s} {rps_s}")
        print()

if __name__ == "__main__":
    main()