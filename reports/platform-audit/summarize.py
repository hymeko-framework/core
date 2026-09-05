"""Rebuild audit tables from retained raw samples; Python standard library only.

Criterion quantiles describe batch averages, not individual request tails.
Diagnostic logs that contain only medians remain raw evidence, not invented samples.
"""
from __future__ import annotations

import csv
import hashlib
import json
import math
from pathlib import Path
import re
import statistics
import sys
import zipfile
from collections import defaultdict


class AuditSummary:
    def __init__(self, root: Path):
        self.root = root
        self.rows: list[dict] = []
        self.cold: list[dict] = []
        self.throughput: list[dict] = []

    @staticmethod
    def read(path: Path) -> str:
        raw = path.read_bytes()
        return raw.decode("utf-16" if raw.startswith(b"\xff\xfe") else "utf-8-sig")

    @staticmethod
    def quantile(values: list[float], q: float) -> float:
        p = (len(values) - 1) * q
        lo, hi = math.floor(p), math.ceil(p)
        return values[lo] + (values[hi] - values[lo]) * (p - lo)

    def record(self, campaign: str, run: int, benchmark: str,
               values: list[float], kind: str, ci: tuple = ()) -> None:
        ordered = sorted(values)
        assert ordered and all(math.isfinite(v) and v >= 0 for v in ordered)
        mean = statistics.mean(ordered)
        variance = statistics.variance(ordered) if len(ordered) > 1 else 0.0
        self.rows.append(dict(
            campaign=campaign, run=run, benchmark=benchmark, sample_kind=kind,
            samples=len(ordered), mean_us=mean, median_us=statistics.median(ordered),
            iqr_us=self.quantile(ordered, .75) - self.quantile(ordered, .25),
            p95_us=self.quantile(ordered, .95), p99_us=self.quantile(ordered, .99),
            worst_us=ordered[-1], variance_us2=variance,
            cv=math.sqrt(variance) / mean if mean else 0,
            median_ci_lo_us=ci[0] if ci else "", median_ci_hi_us=ci[1] if ci else "",
        ))

    def criterion(self) -> None:
        for directory in sorted(self.root.iterdir()):
            match = re.fullmatch(r"(core-run|wgpu-run|core-extended-run|wgpu-final-run)([123])", directory.name)
            if not match:
                continue
            campaign, run = match[1], int(match[2])
            for sample in sorted(directory.rglob("new/sample.json")):
                name = json.loads(self.read(sample.with_name("benchmark.json")))["full_id"]
                valid = {
                    "core-run": name.startswith(("canonical_cbor_", "ir_canonical_hash/")),
                    "wgpu-run": name.startswith("spmm_"),
                    "core-extended-run": name.startswith("ir_construction/") or (
                        name.startswith("csr_spmm_paths/") and name.endswith("/fd32")),
                    "wgpu-final-run": bool(re.fullmatch(
                        r"spmm_gpu_resident/E_(resident_readback|resident_reused_readback|reused_single_wait)/medium/fd32", name)) or (
                        name.startswith("spmm_prepare/") and name != "spmm_prepare/prepare_warm_cache"),
                }[campaign]
                if not valid:
                    continue
                raw = json.loads(self.read(sample))
                values = [t / n / 1000 for t, n in zip(raw["times"], raw["iters"], strict=True)]
                interval = json.loads(self.read(sample.with_name("estimates.json")))["median"]["confidence_interval"]
                self.record(campaign, run, name, values, "criterion_batch_average",
                            (interval["lower_bound"] / 1000, interval["upper_bound"] / 1000))

    def diagnostics(self) -> None:
        for run in range(1, 4):
            for campaign in ("cuda-regular", "cuda-irregular", "transport", "wgpu-kernel", "graph-berge", "wgpu-readback"):
                path = self.root / f"{campaign}-run{run}.stdout.log"
                groups = defaultdict(list)
                for line in self.read(path).splitlines():
                    fields = line.split(",")
                    if campaign.startswith("cuda") and fields[0] == "cold":
                        self.cold.append(dict(campaign=campaign, run=run, span=fields[1], us=float(fields[2])))
                    elif campaign.startswith("cuda") and fields[0] == "sample":
                        if fields[1] == "kernel":
                            groups["kernel_event"].append(float(fields[3]))
                            groups["kernel_host_instrumented"].append(float(fields[4]))
                        else:
                            groups[fields[1]].append(float(fields[3]))
                    elif campaign == "transport" and fields[0] == "sample":
                        for stage, value in zip(("encode", "publish", "receive", "decode", "projection", "update", "e2e"), fields[5:], strict=True):
                            groups[f"{fields[1]}/{fields[2]}/{stage}"].append(float(value))
                    elif campaign == "transport" and fields[0] == "throughput":
                        self.throughput.append(dict(run=run, scale=fields[1], transport=fields[2],
                            requests=int(fields[3]), seconds=float(fields[4]), requests_per_second=float(fields[5])))
                    elif campaign == "graph-berge" and fields[0] == "sample":
                        groups[f"diamonds{fields[1]}/results{fields[3]}/steps{fields[4]}/{fields[5]}"].append(float(fields[6]))
                    elif campaign == "wgpu-kernel" and line.startswith("A\t"):
                        fields = line.split("\t")
                        assert fields[9] == "0", "discarded GPU timestamp sample must be classified"
                        key = f"rows{fields[1]}/deg{fields[2]}/fd{fields[3]}/nnz{fields[4]}"
                        groups[key + "/host"].append(float(fields[7]))
                        groups[key + "/gpu_timestamp"].append(float(fields[8]))
                    elif campaign == "wgpu-readback" and line.startswith("B\t"):
                        fields = line.split("\t")
                        assert fields[14] == "true", "readback payload correctness failed"
                        if fields[6] != "steady":
                            continue
                        key = f"mib{fields[1]}/{fields[3]}/chunk{fields[4]}"
                        for stage, value in zip(("allocation", "gpu_copy", "submit_wait", "map", "memcpy", "total"), fields[7:13], strict=True):
                            if value != "NA":
                                groups[key + "/" + stage].append(float(value))
                assert groups, path
                for key, values in groups.items():
                    self.record(campaign, run, key, values, "individual_diagnostic_sample")

    @staticmethod
    def write_csv(path: Path, rows: list[dict]) -> None:
        assert rows
        with path.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)

    def write(self) -> None:
        archive = self.root / "criterion-raw.zip"
        if archive.exists():
            with zipfile.ZipFile(archive) as bundle:
                for member in bundle.infolist():
                    destination = (self.root / member.filename).resolve()
                    if not destination.is_relative_to(self.root.resolve()):
                        raise ValueError(f"Unsafe archive member: {member.filename}")
                    if not destination.exists():
                        bundle.extract(member, self.root)
        self.criterion()
        self.diagnostics()
        self.write_csv(self.root / "criterion-summary.csv", [r for r in self.rows if r["sample_kind"] == "criterion_batch_average"])
        self.write_csv(self.root / "diagnostic-summary.csv", [r for r in self.rows if r["sample_kind"] != "criterion_batch_average"])
        self.write_csv(self.root / "cuda-cold-summary.csv", self.cold)
        if self.throughput:
            self.write_csv(self.root / "transport-throughput-summary.csv", self.throughput)
        groups = defaultdict(list)
        for row in self.rows:
            groups[(row["campaign"], row["benchmark"])].append(row)
        combined = []
        for (campaign, benchmark), rows in sorted(groups.items()):
            medians = [r["median_us"] for r in rows]
            center = statistics.median(medians)
            relative_range = (max(medians) - min(medians)) / center if center else 0
            max_cv = max(r["cv"] for r in rows)
            combined.append(dict(campaign=campaign, benchmark=benchmark, runs=len(rows),
                medians_us=medians, median_of_medians_us=center, min_median_us=min(medians),
                max_median_us=max(medians), median_relative_range=relative_range,
                max_within_run_cv=max_cv, unstable_heuristic=relative_range > .1 or max_cv > .1,
                p95_us_by_run=[r["p95_us"] for r in rows], p99_us_by_run=[r["p99_us"] for r in rows],
                samples_per_run=[r["samples"] for r in rows]))
        (self.root / "campaign-summary.json").write_text(json.dumps(combined, indent=2) + "\n", encoding="utf-8")
        memory = []
        for path in sorted(self.root.glob("*.memory.json")):
            record = json.loads(self.read(path))
            memory.append(dict(log_prefix=path.stem.removesuffix(".memory"), **record))
        (self.root / "memory-summary.json").write_text(json.dumps(memory, indent=2) + "\n", encoding="utf-8")
        fixtures = []
        for path in [*sorted(self.root.glob("transport-*.cbor")), self.root.parent.parent / "target/canonical-cbor-memory.cbor"]:
            raw = path.read_bytes()
            fixtures.append(dict(path=str(path.relative_to(self.root.parent.parent)), bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest()))
        (self.root / "fixture-summary.json").write_text(json.dumps(fixtures, indent=2) + "\n", encoding="utf-8")
        print(f"Summarized {len(self.rows)} sample groups, {len(memory)} memory records, {len(fixtures)} fixtures")

    def plot(self) -> None:
        """Render existing measurements, with no simulation or interpolated model motion."""
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
        from matplotlib.animation import FuncAnimation, PillowWriter

        rows = json.loads(self.read(self.root / "campaign-summary.json"))
        lookup = {(r["campaign"], r["benchmark"]): r for r in rows}
        fig, axes = plt.subplots(2, 2, figsize=(13, 9), constrained_layout=True)
        resident = [r for r in rows if r["campaign"] == "wgpu-final-run" and r["benchmark"].startswith("spmm_gpu_resident/")]
        for x, row in enumerate(resident):
            axes[0, 0].scatter([x] * 3, [v / 1000 for v in row["medians_us"]], color="#225e91")
        axes[0, 0].set(xticks=range(3), xticklabels=["Fresh readback", "Reused / two waits", "Reused / one wait"], ylabel="Run median (ms)", title="WGPU: same resident input, separate processes")
        scales = ["small", "medium", "large"]
        for mode, marker in [("loopback", "o"), ("tcp", "s")]:
            values = [lookup[("transport", f"{scale}/{mode}/e2e")]["median_of_medians_us"] for scale in scales]
            axes[0, 1].plot(scales, values, marker=marker, label=mode)
        axes[0, 1].set(yscale="log", ylabel="Headless encode-to-model median (us)", title="Transport: 2,268 / 57,214 / 916,106 bytes")
        axes[0, 1].legend()
        berge = sorted([r for r in rows if r["campaign"] == "graph-berge"], key=lambda r: int(re.search(r"diamonds(\d+)", r["benchmark"])[1]))
        axes[1, 0].plot([2, 4, 8, 12, 16], [r["median_of_medians_us"] for r in berge], "o-", color="#225e91")
        axes[1, 0].set(yscale="log", xlabel="Chained diamonds", ylabel="Enumeration median (us)", title="Berge: output-sensitive; cap = 4,096 paths", xticks=[2, 4, 8, 12, 16])
        for x, row in zip([2, 4, 8, 12, 16], berge):
            count = re.search(r"results(\d+)", row["benchmark"])[1]
            axes[1, 0].annotate(count + " paths", (x, row["median_of_medians_us"]), xytext=(0, 8), textcoords="offset points", ha="center", fontsize=8)
        axes[1, 0].margins(y=.15)
        labels = ["small", "medium", "large"]
        for mode in ("encode", "decode"):
            cells = sorted([r for r in rows if r["campaign"] == "core-run" and r["benchmark"].startswith(f"canonical_cbor_{mode}/")], key=lambda r: next(i for i, label in enumerate(labels) if label in r["benchmark"]))
            axes[1, 1].plot(labels, [r["median_of_medians_us"] for r in cells], "o-", label=mode)
        axes[1, 1].set(yscale="log", ylabel="Criterion batch-average median (us)", title="Core CBOR: current campaign")
        axes[1, 1].legend()
        for axis in axes.flat:
            axis.grid(alpha=.2)
        fig.suptitle("HyMeKo audit / Windows RTX 5080 Laptop / three runs / 2026-09-05\nRaw samples and instability flags accompany this figure", fontsize=12)
        for suffix in ("png", "svg"):
            fig.savefig(self.root / f"campaign.{suffix}", dpi=160)
        plt.close(fig)

        # Reveal actual successive request samples; this is a transport trace,
        # not a browser recording, simulated movement, or extra benchmark run.
        series = defaultdict(list)
        for line in self.read(self.root / "transport-run1.stdout.log").splitlines():
            fields = line.split(",")
            if line.startswith("sample,small,"):
                series[fields[2]].append(float(fields[-1]))
        fig, axis = plt.subplots(figsize=(12, 8), dpi=100)
        axis.set(xlim=(0, 99), ylim=(0, max(max(v) for v in series.values()) * 1.1), xlabel="Measured request index (5 warmups excluded)", ylabel="Headless encode-to-model latency (us)", title="Measured transport trace / small payload / run 1\nProgressive display of retained samples; not simulator motion")
        artists = {mode: axis.plot([], [], label=mode)[0] for mode in series}
        axis.legend()
        axis.grid(alpha=.2)

        def frame(end):
            for mode, artist in artists.items():
                artist.set_data(range(end), series[mode][:end])
            return tuple(artists.values())

        animation = FuncAnimation(fig, frame, frames=range(5, 101, 5), interval=200, blit=True)
        animation.save(self.root / "transport-trace.gif", writer=PillowWriter(fps=5))
        plt.close(fig)
        print("Rendered campaign.png, campaign.svg and measured transport-trace.gif")


if __name__ == "__main__":
    summary = AuditSummary(Path(__file__).resolve().parent)
    if "--plot-only" in sys.argv:
        summary.plot()
    else:
        summary.write()
