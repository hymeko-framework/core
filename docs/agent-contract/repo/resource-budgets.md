---
source: CLAUDE.md §4 Resource Budgets, lines 278-290, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: verbatim extraction — not yet de-projectified
---

**Contains:** §4 the 16 GB RSS cap, its enforcement, long-running-script obligations

## 4. Resource Budgets

- **Hard memory cap: 16 GB peak RSS.** This applies to every process spawned by a task. The cap is on **resident set size (RSS)**, not virtual address space (VAS) — modern PyTorch+CUDA stacks reserve ~30 GB of sparse VAS at process start even when RSS stays under 2 GB, so a VAS-based cap would kill normal workloads at `.to('cuda')`.
  - Enforce with `systemd-run --user -p MemoryMax=16G` (cgroups v2, RSS gate) — the canonical option. For Python-only entry points, `resource.setrlimit(resource.RLIMIT_DATA, …)` is also acceptable.
  - **`ulimit -v` is forbidden.** It is a VAS gate, not an RSS gate, and triggers `RuntimeError: CUDA driver error: out of memory` at the first `torch.Tensor.to('cuda')` call. Measured 2026-05-11 on HymeYOLO `train_circles_ricci`: RSS = 1.77 GB, VSZ = 29.45 GB — a healthy run that `ulimit -v 16777216` killed three times in a row before the cause was diagnosed.
  - If a run exceeds the RSS cap, **abort and redesign**. Do not raise the cap. Do not add swap. Reduce batch size, stream the data, or refactor the algorithm.
- Every long-running script must:
  - Report **peak RSS** and **wall time** on exit.
  - Support checkpointing. Never rely on a single uninterrupted multi-hour run.
- GPU memory budgets, if any, are declared in the plan per task.
