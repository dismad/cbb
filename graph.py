#!/usr/bin/env python3
"""Graph cbb JSON results (pass2-warm preferred). Auto-detects services.
Generates both linear and log-scale versions of each plot.
Handles both "svc-50-pass2-warm.json" and "svc-blockrange-50-pass2-warm.json".
"""

import json
import glob
import os
import re
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
    """Return (service, concurrency) from either naming style.
    Examples:
      lwd-50-pass2-warm.json              -> ("lwd", 50)
      lwd-blockrange-50-pass2-warm.json   -> ("lwd", 50)
    """
    stem = os.path.splitext(basename)[0]
    # Try the blockrange form first
    m = re.match(r'^([a-zA-Z0-9]+)-blockrange-(\d+)-', stem)
    if m:
        return m.group(1), int(m.group(2))
    # Fall back to original form
    m = re.match(r'^([a-zA-Z0-9]+)-(\d+)-', stem)
    if m:
        return m.group(1), int(m.group(2))
    return None, None

def detect_services():
    found = set()
    for path in glob.glob(os.path.join(RESULTS_DIR, "*-pass2-warm.json")):
        svc, _ = parse_filename(os.path.basename(path))
        if svc:
            found.add(svc)
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
            svc, conc = parse_filename(os.path.basename(path))
            if svc != service or conc is None:
                continue
            rows.append((conc, data[0]))
        except (json.JSONDecodeError, ValueError, IndexError) as e:
            print(f"skip bad file {path}: {e}")
            continue
    return sorted(rows, key=lambda x: x[0])

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

def plot(services, colors, title, ylabel, key, method=None, filename="plot.png", logy=False):
    plt.figure(figsize=(10, 5), constrained_layout=True)
    any_data = False
    for svc in services:
        rows = load_warm(svc)
        if not rows:
            print(f"no data for {svc}")
            continue
        xs, ys = series(rows, key, method)
        if not xs:
            continue
        any_data = True
        color = colors.get(svc, "#333333")
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
    colors = assign_colors(services)
    print(f"services: {services}")
    print(f"colors: {colors}")

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
        plot(services, colors, title, ylabel, key, method, filename=f"{base}.png", logy=False)
        plot(services, colors, title, ylabel, key, method, filename=f"{base}_log.png", logy=True)

    print(f"\nGraphs written to {OUT_DIR}/ (linear + _log versions)")

if __name__ == "__main__":
    main()
