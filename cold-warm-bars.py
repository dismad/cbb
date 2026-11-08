#!/usr/bin/env python3
"""Grouped bar charts: cold vs warm at each concurrency.
Generates both linear and log-scale versions.
"""

import json
import glob
import os
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

RESULTS_DIR = "results"
OUT_DIR = "graphs"
os.makedirs(OUT_DIR, exist_ok=True)

# Fixed distinct colors
COLORS = {
    "lwd":      "#1f77b4",
    "zainod":   "#ff7f0e",
    "zingod":   "#2ca02c",
    "native":   "#d62728",
    "zebrad":   "#9467bd",
    "zinder":   "#8c564b",
    "lwd-rs":   "#e377c2",
    "ztreamer": "#7f7f7f",
}

def load_pass(service, label):
    pattern = os.path.join(RESULTS_DIR, f"{service}-*-{label}.json")
    out = {}
    for path in glob.glob(pattern):
        try:
            raw = open(path).read().strip()
            start = raw.find("[")
            if start == -1:
                continue
            data = json.loads(raw[start:])
            if not data:
                continue
            conc = int(os.path.basename(path).split("-")[1])
            out[conc] = data[0]
        except Exception:
            continue
    return out

def detect_services():
    found = set()
    for path in glob.glob(os.path.join(RESULTS_DIR, "*-pass*.json")):
        found.add(os.path.basename(path).split("-")[0])
    return sorted(found) or ["lwd", "zainod"]

def get_val(summary, key, method=None):
    if method:
        m = next((x for x in summary.get("methods", []) if x["method"] == method), None)
        return m.get(key, 0) if m else 0
    return summary.get(key, 0)

def plot_bars(services, title, ylabel, key, method=None, filename="bars.png", logy=False):
    all_conc = set()
    data = {}
    for svc in services:
        cold = load_pass(svc, "pass1")
        warm = load_pass(svc, "pass2-warm")
        data[svc] = (cold, warm)
        all_conc.update(cold.keys())
        all_conc.update(warm.keys())
    concs = sorted(all_conc)
    if not concs:
        print(f"no data: {filename}")
        return

    n_svc = len(services)
    group_width = 0.8
    bar_width = group_width / (n_svc * 2)
    x = np.arange(len(concs))

    # Wider figure so external legend has room
    plt.figure(figsize=(12, 5.5), constrained_layout=True)
    for i, svc in enumerate(services):
        cold, warm = data[svc]
        color = COLORS.get(svc, f"C{i}")
        cold_vals = [get_val(cold[c], key, method) if c in cold else 0 for c in concs]
        warm_vals = [get_val(warm[c], key, method) if c in warm else 0 for c in concs]
        # Tiny floor so log scale can still show zero bars
        if logy:
            cold_vals = [max(v, 1e-4) for v in cold_vals]
            warm_vals = [max(v, 1e-4) for v in warm_vals]
        offset = (i * 2 - n_svc + 0.5) * bar_width
        plt.bar(x + offset, cold_vals, bar_width, label=f"{svc} cold",
                color=color, alpha=0.45, hatch="//", edgecolor=color)
        plt.bar(x + offset + bar_width, warm_vals, bar_width, label=f"{svc} warm",
                color=color, alpha=0.9, edgecolor=color)

    plt.xlabel("concurrency (wallets)")
    plt.ylabel(ylabel + (" (log)" if logy else ""))
    plt.title(title + (" — log scale" if logy else ""))
    if logy:
        plt.yscale("log")
    plt.xticks(x, concs)
    # Legend outside the plot, never overlapping bars
    plt.legend(
        loc="upper left",
        bbox_to_anchor=(1.02, 1.0),
        fontsize=8,
        framealpha=0.95,
        borderaxespad=0.0,
        ncol=1,
    )
    plt.grid(True, axis="y", alpha=0.3, which="both" if logy else "major")
    out = os.path.join(OUT_DIR, filename)
    plt.savefig(out, dpi=130, bbox_inches="tight")
    print(f"saved {out}")
    plt.close()

def main():
    services = detect_services()
    print(f"services: {services}")

    specs = [
        ("Error rate  (hatched = cold, solid = warm)", "error rate", "error_rate", None, "bar_error_rate"),
        ("p95 latency  (hatched = cold, solid = warm)", "p95 (ms)", "p95", None, "bar_p95"),
        ("p99 latency  (hatched = cold, solid = warm)", "p99 (ms)", "p99", None, "bar_p99"),
        ("max latency  (hatched = cold, solid = warm)", "max (ms)", "max", None, "bar_max"),
        ("GetBlockRange p95  (hatched = cold, solid = warm)", "p95 (ms)", "p95", "GetBlockRange", "bar_blockrange_p95"),
        ("GetBlockRange errors  (hatched = cold, solid = warm)", "errors", "errors", "GetBlockRange", "bar_blockrange_errors"),
        ("GetTreeState p95  (hatched = cold, solid = warm)", "p95 (ms)", "p95", "GetTreeState", "bar_treestate_p95"),
        ("Throughput  (hatched = cold, solid = warm)", "requests/sec", "throughput_rps", None, "bar_throughput"),
        ("Wall time  (hatched = cold, solid = warm)", "seconds", "wall_time_secs", None, "bar_wall_time"),
    ]

    for title, ylabel, key, method, base in specs:
        plot_bars(services, title, ylabel, key, method, filename=f"{base}.png", logy=False)
        plot_bars(services, title, ylabel, key, method, filename=f"{base}_log.png", logy=True)

    print(f"\nBar graphs written to {OUT_DIR}/ (linear + _log versions)")

if __name__ == "__main__":
    main()
