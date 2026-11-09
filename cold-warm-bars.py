#!/usr/bin/env python3
"""Grouped bar charts: cold vs warm at each concurrency.
Generates both linear and log-scale versions.
Handles both "svc-50-pass1.json" and "svc-blockrange-50-pass1.json".
"""

import json
import glob
import os
import re
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.colors import to_hex

RESULTS_DIR = "results"
OUT_DIR = "graphs"
os.makedirs(OUT_DIR, exist_ok=True)

PREFERRED = {
    "lwd":      "#1f77b4",
    "zainod":   "#ff7f0e",
    "zingod":   "#2ca02c",
    "native":   "#d62728",
    "zebrad":   "#9467bd",
    "zinder":   "#8c564b",
    "lwd-rs":   "#e377c2",
    "ztreamerd":"#7f7f7f",
    "ztreamer": "#7f7f7f",
}

def distinct_colors(n):
    if n <= 10:
        cmap = plt.cm.tab10
    elif n <= 20:
        cmap = plt.cm.tab20
    else:
        cmap = plt.cm.gist_ncar
    return [to_hex(cmap(i / max(n - 1, 1))) for i in range(n)]

def assign_colors(services):
    colors = {}
    used = set()
    for svc in services:
        if svc in PREFERRED and PREFERRED[svc] not in used:
            colors[svc] = PREFERRED[svc]
            used.add(PREFERRED[svc])
    remaining = [s for s in services if s not in colors]
    if remaining:
        pool = distinct_colors(len(services) + len(remaining))
        idx = 0
        for svc in remaining:
            while idx < len(pool) and pool[idx] in used:
                idx += 1
            colors[svc] = pool[idx] if idx < len(pool) else f"C{len(colors)}"
            used.add(colors[svc])
            idx += 1
    return colors

def parse_filename(basename):
    """Return (service, concurrency) from either naming style."""
    stem = os.path.splitext(basename)[0]
    m = re.match(r'^([a-zA-Z0-9]+)-blockrange-(\d+)-', stem)
    if m:
        return m.group(1), int(m.group(2))
    m = re.match(r'^([a-zA-Z0-9]+)-(\d+)-', stem)
    if m:
        return m.group(1), int(m.group(2))
    return None, None

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
            svc, conc = parse_filename(os.path.basename(path))
            if svc != service or conc is None:
                continue
            out[conc] = data[0]
        except Exception:
            continue
    return out

def detect_services():
    found = set()
    for path in glob.glob(os.path.join(RESULTS_DIR, "*-pass*.json")):
        svc, _ = parse_filename(os.path.basename(path))
        if svc:
            found.add(svc)
    return sorted(found) or ["lwd", "zainod"]

def get_val(summary, key, method=None):
    if method:
        m = next((x for x in summary.get("methods", []) if x["method"] == method), None)
        return m.get(key, 0) if m else 0
    return summary.get(key, 0)

def plot_bars(services, colors, title, ylabel, key, method=None, filename="bars.png", logy=False):
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

    plt.figure(figsize=(12, 5.5), constrained_layout=True)
    for i, svc in enumerate(services):
        cold, warm = data[svc]
        color = colors.get(svc, "#333333")
        cold_vals = [get_val(cold[c], key, method) if c in cold else 0 for c in concs]
        warm_vals = [get_val(warm[c], key, method) if c in warm else 0 for c in concs]
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
    colors = assign_colors(services)
    print(f"services: {services}")
    print(f"colors: {colors}")

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
        plot_bars(services, colors, title, ylabel, key, method, filename=f"{base}.png", logy=False)
        plot_bars(services, colors, title, ylabel, key, method, filename=f"{base}_log.png", logy=True)

    print(f"\nBar graphs written to {OUT_DIR}/ (linear + _log versions)")

if __name__ == "__main__":
    main()
