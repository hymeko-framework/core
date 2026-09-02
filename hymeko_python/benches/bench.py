"""Python-side benchmark (mission §10).

Axes: `import hymeko`, parse+lower+coherence+hash (the fused build), canonical hash
(O(1) precomputed), scalar id lookup (single + 1e3 loop), batch incidence export.
Reports peak process RSS (native) and the Python heap separately. Compare the batch
number against the Rust-only SoA build printed by
`cargo test -p hymeko_python --test perf_timing --release -- --nocapture`.

Run: `.venv/Scripts/python benches/bench.py`
"""

from __future__ import annotations

import ctypes
import ctypes.wintypes as wt
import pathlib
import statistics
import subprocess
import sys
import time
import tracemalloc

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "tests" / "python"))
import _support as sup  # noqa: E402

import hymeko  # noqa: E402


def median_ms(iters, fn, warmup=2):
    for _ in range(warmup):
        fn()
    xs = []
    for _ in range(iters):
        t = time.perf_counter()
        fn()
        xs.append((time.perf_counter() - t) * 1e3)
    return statistics.median(xs)


def peak_rss_mb():
    """Windows peak working set (native peak RSS) in MiB."""
    class PMC(ctypes.Structure):
        _fields_ = [
            ("cb", wt.DWORD), ("PageFaultCount", wt.DWORD),
            ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t), ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t),
        ]
    kernel32 = ctypes.windll.kernel32
    kernel32.GetCurrentProcess.restype = wt.HANDLE
    psapi = ctypes.windll.psapi
    psapi.GetProcessMemoryInfo.argtypes = [wt.HANDLE, ctypes.POINTER(PMC), wt.DWORD]
    psapi.GetProcessMemoryInfo.restype = wt.BOOL
    c = PMC()
    c.cb = ctypes.sizeof(PMC)
    if not psapi.GetProcessMemoryInfo(kernel32.GetCurrentProcess(), ctypes.byref(c), c.cb):
        return float("nan")
    return c.PeakWorkingSetSize / (1024 * 1024)


def bench_import():
    # cold import time in a fresh interpreter
    code = "import time,importlib; t=time.perf_counter(); importlib.import_module('hymeko'); print((time.perf_counter()-t)*1e3)"
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    return float(out.stdout.strip())


def main():
    print(f"python {sys.version.split()[0]}  hymeko {hymeko.__version__}")
    print(f"import hymeko (cold)      : {bench_import():8.2f} ms")

    sizes = [("small", 204, 5), ("medium", 2004, 5), ("1e5", 100004, 5)]
    print(f"\n{'size':7} {'nnz':>8} {'parse_ms':>10} {'hash_ms':>9} {'lookup_ns':>10} {'loop1e3_ms':>11} {'batch_ms':>9}")
    tracemalloc.start()
    for label, n, k in sizes:
        src = sup.windowed_source("Scale", n, k)
        nnz = sup.windowed_incidence_count(n, k)
        parse_ms = median_ms(3 if n < 50000 else 2, lambda s=src: hymeko.parse(s))
        m = hymeko.parse(src)
        hash_ms = median_ms(20, m.canonical_hash)
        nodes = m.nodes()
        nid = nodes[0].id
        lookup_ms = median_ms(50, lambda: m.node(nid))
        ids = [nd.id for nd in nodes[: min(1000, len(nodes))]]
        loop_ms = median_ms(20, lambda: [m.node(i) for i in ids])
        batch_ms = median_ms(10, m.incidence_arrays)
        print(f"{label:7} {nnz:>8} {parse_ms:>10.3f} {hash_ms:>9.4f} {lookup_ms*1e6:>10.0f} {loop_ms:>11.3f} {batch_ms:>9.3f}")

    cur, peak_heap = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    print(f"\nPython heap (tracemalloc peak): {peak_heap/1024/1024:7.2f} MiB")
    print(f"process peak RSS (native+py)  : {peak_rss_mb():7.2f} MiB  (cap 16 GiB)")


if __name__ == "__main__":
    main()
