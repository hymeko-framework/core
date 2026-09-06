"""Plot this verification run's resource records, not consumer performance.

The earlier platform-audit summarizer owns Criterion sample distributions;
this figure consumes the distinct container-stage schema in summary.json.
Run: python3 reports/core-amaterasu/plot_resources.py
"""
import json
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt


if __name__ == "__main__":
    root = Path(__file__).resolve().parent
    summary = json.loads((root / "summary.json").read_text(encoding="utf-8"))
    stages = summary["cpu"]["stages"] + summary["gpu"]["stages"]
    labels = [stage["stage"] for stage in stages]
    colors = ["#2563eb"] * len(summary["cpu"]["stages"]) + ["#13795b"] * len(summary["gpu"]["stages"])
    fig, axes = plt.subplots(1, 2, figsize=(12, 5.8), sharey=True)
    axes[0].barh(labels, [stage["elapsed_s"] for stage in stages], color=colors)
    axes[0].set_xlabel("Elapsed seconds, including compilation where needed")
    axes[1].barh(labels, [stage["max_rss_kib"] / 1024 for stage in stages], color=colors)
    axes[1].set_xlabel("GNU time maximum process RSS (MiB)")
    axes[0].invert_yaxis()
    for axis in axes:
        axis.grid(axis="x", alpha=0.2)
        axis.set_axisbelow(True)
    fig.suptitle("Core container verification on Amaterasu: one run per stage")
    fig.text(0.5, 0.015, "Blue: CPU checks. Green: serial GPU checks. Different workloads; no speedup or latency claim.", ha="center")
    fig.tight_layout(rect=(0, 0.035, 1, 0.95))
    for suffix in ("png", "svg"):
        fig.savefig(root / f"verification-resources.{suffix}", dpi=160, metadata={"Date": None} if suffix == "svg" else None)
    plt.close(fig)
