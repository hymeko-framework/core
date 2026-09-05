"""Render the retained Rust baseline counts; no model/runtime coverage blending."""

import json
import re
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt


if __name__ == "__main__":
    evidence = Path(__file__).resolve().parent
    log = (evidence / "admission-baseline.log").read_text(encoding="utf-8-sig")
    counts = re.search(r"test result: FAILED\. (\d+) passed; (\d+) failed; (\d+) ignored", log)
    if counts is None:
        raise SystemExit("Missing expected baseline result; refusing to invent plot values")
    passed, failed, ignored = map(int, counts.groups())
    fig, ax = plt.subplots(figsize=(9.6, 4.8), layout="constrained")
    bars = ax.barh(["Passed", "Failed", "Ignored"], [passed, failed, ignored],
                   color=["#287d59", "#b84738", "#7a8288"])
    ax.bar_label(bars, padding=5)
    ax.set_xlim(0, max(passed, failed, ignored) + 1)
    ax.set_xticks(range(max(passed, failed, ignored) + 2))
    ax.set_xlabel("Public-API admission assertions (count)")
    ax.set_title("Core admission baseline on Amaterasu\nSDK c935909: known runtime failures")
    ax.spines[["top", "right"]].set_visible(False)
    fig.savefig(evidence / "admission-baseline.svg", metadata={"Date": None})
    (evidence / "admission-counts.json").write_text(
        json.dumps({"passed": passed, "failed": failed, "ignored": ignored,
                    "source": "admission-baseline.log", "matplotlib": matplotlib.__version__},
                   indent=2) + "\n", encoding="utf-8")
