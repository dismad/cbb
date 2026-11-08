#!/usr/bin/env python3
"""Graph cbb JSON results (pass2-warm preferred). Auto-detects services.
Generates both linear and log-scale versions of each plot.
"""

import json
import glob
import os
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

RESULTS_DIR = "results"
OUT_DIR = "graphs"
os.makedirs(OUT_DIR, exist_ok=True)

# Fixed distinct colors so the same service always looks the same
COLORS = {
    "lwd":     "#1f77b4",
    "zainod":  "#ff7f0e",
    "zingod":  "#2ca02c",
    "native":  "#d62728",
    "zebrad":  "#9467bd",
    "zinder":  "#8c564b",
    "lwd-rs":  "#e377c2",
    "ztreamer":"#7f7f7f",
}

def detect_services():
    found = set()
    for path in glob.glob(os.path.join(RESULTS_DIR, "*-pass2-warm.json")):
        found.add(os.path.basename(path).split("-")[0])
    return sorted(found) or ["lwd", "zainod"]

def load_warm(service):
    """Load pass2-warm JSON files for a service. Returns list of (concurrency, summary)."""
    pattern = os.path.join(RESULTS_DIR, f"{service}-*-pass2-warm.json")
    rows = []
    for path in sorted(glob.glob(pattern)):
        try:
            with open(path) as f:
                raw = f.read().strip()
            start = raw.find("[")
            if start == -1:
                print(f"skip (no JSON): {path}")
                continue
            data = json.loads(raw[start:])
            if not data:
                continue
            conc = int(os.path.basename(path).split("-")[1])
            rows.append((conc, data[0]))
        except (json.JSONDecodeError, ValueError, IndexError) as e:
            print(f"skip bad file {path}: {e}")
            continue
    return sorted(rows)

def series(rows, key, method=None):
    xs, ys = [], []
    for conc, s in rows:
        if method:
            m = next((x for x in s.get("methods", []) if x["method"] == method), None)
            if not m:
                continue
            val = m.get(key, 0)
        else:
            val = s.get(key, 0)
        xs.append(conc)
        ys.append(val)
    return xs, ys

def plot(services, title, ylabel, key, method=None, filename="plot.png", logy=False):
    plt.figure(figsize=(10, 5), constrained_layout=True)
    any_data = False
    for i, svc in enumerate(services):
        rows = load_warm(svc)
        if not rows:
            print(f"no data for {svc}")
            continue
        xs, ys = series(rows, key, method)
        if not xs:
            continue
        any_data = True
        color = COLORS.get(svc, f"C{i}")
        # For log scale, replace zeros with a tiny value so the point still appears
        if logy:
            ys = [max(v, 1e-4) if v == 0 else v for v in ys]
        plt.plot(xs, ys, marker="o", label=svc, color=color, linewidth=1.8, markersize=6)
    if not any_data:
        print(f"no data: {filename}")
        plt.close()
        return
    plt.xlabel("concurrency (wallets)")
    plt.ylabel(ylabel + (" (log)" if logy else ""))
    plt.title(title + (" — log scale" if logy else ""))
    if logy:
        plt.yscale("log")
    # Legend outside the plot, never overlapping data
    plt.legend(
        loc="upper left",
        bbox_to_anchor=(1.02, 1.0),
        fontsize=9,
        framealpha=0.95,
        borderaxespad=0.0,
    )
    plt.grid(True, alpha=0.3, which="both" if logy else "major")
    out = os.path.join(OUT_DIR, filename)
    plt.savefig(out, dpi=120, bbox_inches="tight")
    print(f"saved {out}")
    plt.close()

def main():
    services = detect_services()
    print(f"services: {services}")

    specs = [
        ("Error rate vs concurrency", "error rate", "error_rate", None, "error_rate"),
        ("p50 latency vs concurrency", "p50 (ms)", "p50", None, "p50"),
        ("p95 latency vs concurrency", "p95 (ms)", "p95", None, "p95"),
        ("p99 latency vs concurrency", "p99 (ms)", "p99", None, "p99"),
        ("GetBlockRange p95 vs concurrency", "p95 (ms)", "p95", "GetBlockRange", "blockrange_p95"),
        ("GetBlockRange errors vs concurrency", "errors", "errors", "GetBlockRange", "blockrange_errors"),
        ("Throughput vs concurrency", "requests/sec", "throughput_rps", None, "throughput"),
        ("Wall time vs concurrency", "seconds", "wall_time_secs", None, "wall_time"),
    ]

    for title, ylabel, key, method, base in specs:
        # Linear
        plot(services, title, ylabel, key, method, filename=f"{base}.png", logy=False)
        # Log (skip pure error-rate if you prefer, but generate both for consistency)
        plot(services, title, ylabel, key, method, filename=f"{base}_log.png", logy=True)

    print(f"\nGraphs written to {OUT_DIR}/ (linear + _log versions)")

if __name__ == "__main__":
    main()
