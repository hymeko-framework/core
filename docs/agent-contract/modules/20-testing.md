---
source: CLAUDE.md §3 (except Live observability), lines 193-259, 272-277, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: de-projectified (Phase 4)
---

**Contains:** §3 required layers · rules · coverage · determinism · benchmark stability · regression discipline · evaluation-metric integrity · required tooling

# Testing

## 3. Testing Protocol

Every change ships with tests, and tests are executed and pass before reporting success. A passing implementation without a passing test suite is a protocol violation.

### Required layers

- **Unit tests** — per function / per module, pure, deterministic, fast (target < 1 s each). Cover normal cases, boundary cases, and at least one failure case per public function.
- **Integration tests** — exercise module boundaries on realistic inputs. End-to-end paths that match real usage.
- **Performance tests** — measure:
  - **Peak resident memory (RSS)** — must be under the budget declared in the plan and never exceed the global cap (Section 4).
  - **Inference / runtime latency** — wall time and, where relevant, throughput (samples/s, tokens/s).
  - Each performance test asserts a numerical budget. A test that only prints numbers is not a performance test.

### Rules

- New public code without tests is rejected by you, not approved.
- Tests must run in CI-equivalent isolation (clean working dir, no hidden global state).
- Flaky tests are bugs, not noise. Mark them, do not retry-until-green.
- **Production-scale smoke before queuing a long run.** A new code path or env-var branch must be exercised at **production scale** (real dataset, real cap, real wall-clock, real RSS) for at least 1 seed / 1 arity *before* it is queued in a multi-seed or overnight run. Unit tests at toy scale do **not** substitute — they will not surface memory bloat, missing-cap regressions, or per-stage wall costs that only appear at the real input size. If the smoke can't be afforded in ≤ 10 % of the queued run's wall budget, write a single-stage smoke (one arity, one dataset, no training) that costs less than that.
- **In-flight experiment claims must cite a verifiable disk artifact.** Before writing "an experiment is in flight / running / queued" in a report or memory, the claim must reference one of: a log file path that exists and is growing; an output jsonl path (zero bytes is acceptable — the file existing proves the script started); a PID/jobspec captured at launch; a `systemd-run --user` unit name; or an orchestrator `/tmp/<slug>/` dir. An ID-only string (e.g. a hash, a job slug) without a corresponding path is **unverified** and must be flagged as such, not asserted as in-progress.
- **Run new modules before queuing.** Before any overnight/multi-hour run touches a new module, run that module's unit tests in the same environment as the queued run. Untested rewired call paths turn latent bugs into 90-min OOMs; the precedent in this project is a cycle-cache module whose tests existed and were never run in the queued environment.
- **Oracle-certify the TRAINING reward before queuing any RL run (added 2026-07-05, user-enforced after a wasted overnight launch).** The reward oracle's certification of the *training* reward spec (`reward_oracle.certify` in this project) must return `delivers=True`, quoted in the launch message and recorded in the run's summary JSON. If the BC teacher's *strategy* changed (e.g. pinch → push), additionally re-check that dense reward terms do not fight the new anchor. Prefer the **machine-enforced** form: campaign entrypoints raise on an uncertified reward unless an explicit `allow_uncertified=True` waiver is passed (one campaign entrypoint already implements this; extend the pattern to new entrypoints rather than relying on operator discipline). The overnight this cost, and why every procedural gate passed while the semantic one was missing: [incidents/2026-07-05-uncertified-reward-launch.md](../incidents/2026-07-05-uncertified-reward-launch.md).

### Coverage rule (new and modified code)

- Every new function, method, or struct (**public or private**) must be exercised by at least one test added in the same change. A private helper may share a test with its public caller, but a test that drives the new path must exist.
- Every modification to the **observable behavior** of an existing function requires a new **regression test** — one that would have failed against the prior implementation. "It still passes the old tests" is not sufficient.
- Indirect coverage via an unchanged integration test does **not** satisfy this rule. The integration test must be new, extended, or have its assertions strengthened.
- Pure renames, formatting, and comment-only changes are exempt — but must be declared as such in the report (Section 9).

### Determinism and reproducibility

- Every test and experiment **sets** its random seed explicitly (for resume, debugging, and provenance). No reliance on system entropy for *which* seed is used.
- **RL / stochastic-training carve-out (added 2026-06-26, user policy).** Reinforcement-learning and other inherently high-variance training runs are **not** required to be *bit-exact* reproducible. Expecting bit-identical outcomes from a stochastic optimiser over a stochastic environment is scientifically unjustified, and forcing single-threaded BLAS to chase it costs ~3× wall time for no gain. For these runs: set the seed (so a run can be resumed and labelled), but rest every quantitative claim on **multi-seed median/IQR** — not single-run reproduction (this is already the benchmark discipline below). **Multi-threading / parallel BLAS is permitted and encouraged for speed**, and independent cells (configs, seeds) *should* be parallelised across cores. Report the seed(s) and the thread setting; do **not** assert a single RL run's exact numbers as bit-reproducible.
- **Strict (bit-exact) determinism is still required** for: deterministic unit tests of pure functions; floating-point parity with published benchmarks or RTL golden fixtures (pin BLAS thread count, math mode `MKL_CBWR` / `CUBLAS_WORKSPACE_CONFIG`, library versions); and **supervised comparisons where reproducibility is the point** (e.g. an A/B of two model variants on a fixed dataset — seed before each build, not once at process start). Document the pinning in the test or report.
- Tests must be order-independent. If `pytest -p no:randomly` or `cargo test` parallelism breaks them, the tests are wrong, not the runner.
- Test inputs are either generated deterministically from a seed, or committed as fixtures with content hash.

### Benchmark stability

- Performance tests run a minimum of **5 iterations after warm-up** and report **median, IQR, and worst case**. Single-shot wall-clock numbers are not measurements.
- Memory measurements report **peak RSS** over the run, not instantaneous values.
- Run on a quiet machine. Background CPU contention invalidates the run; document the host (CPU model, frequency governor, RAM, OS) in the report.
- For GPU work: report device, driver, CUDA version, and confirm `nvidia-smi` shows no other workload during the measurement window.

### Performance regression discipline

- A regression of more than **10 %** versus the previous measured baseline (memory or latency) blocks completion until investigated.
- The default attribution for a regression is **"a bug was introduced"**, NOT "the new method is inherently more expensive."
- A regression may be accepted as inherent only when **all** of the following hold:
  1. A profile is captured using the canonical profiler for the language (Section 10) and attached to the report as a flamegraph SVG.
  2. The profile shows the additional cost concentrated in the **intentional new work**, not in incidental code (allocation, copying, lock contention, accidental O(n²)).
  3. The cost is justified against the change's stated goal in the plan.
- *"It must be the algorithm"* is a hypothesis, not an explanation.
- Symmetric rule for speedups: a > 10 % improvement must also be confirmed by profile, not by single-shot wall-clock variance.
- No micro-optimization is permitted without a profile demonstrating the targeted code as a hot spot. "Defensive optimization" — refactoring for hypothetical performance — is forbidden.

### Evaluation-metric integrity (added 2026-07-01, after a session lost to metric-driven false starts)

A metric that is mis-measured, un-anchored, or inflatable by failure sends the whole optimization loop chasing a phantom. Before a metric is trusted to rank models, gate a result, or trigger a panic:

- **Measure the ceiling before optimizing under it.** Before tuning a policy/model to raise a metric, measure the *achievable* ceiling — the demonstrator / teacher / oracle score under the **same** metric — and compare the model to it. A low score is a *gap to explain* (undertrained clone? wrong refiner? genuine task wall?), not automatically a failure.
- **Horizon-match every probe to the production env.** A diagnostic probe MUST use the real env's `max_steps` / episode horizon. A probe on a truncated horizon does not measure the task noisily; it measures a *different task*.
- **The demo / selection filter must equal the eval metric.** If success is graded as "held for K consecutive steps" (dwell), then demos, curricula, and early-stops must select on the **same held rule** — not a looser proxy. Filter criterion ≡ grading criterion.
- **A metric a failure can INFLATE is more dangerous than one that only drops.** Guard success predicates against artifact inflation. Every success predicate that divergence can trip needs a divergence guard (`qacc` / NaN / velocity bound) inside the metric itself.
- **Single-seed is a point estimate, not a verdict.** Rest every ranking claim on multi-seed median/IQR (the §3 benchmark discipline), and label in writing what is measured vs inferred vs still-hypothesis (Operating principles) at the metric layer too.

The four cases that produced these five rules — an unmeasured ceiling, a probe on the
wrong horizon, a filter that did not equal the metric, and a failure that inflated one:
[incidents/2026-07-01-metric-integrity.md](../incidents/2026-07-01-metric-integrity.md).

Live observability (§3, "never run blind") is in
[40-observability.md](40-observability.md).

### Required tooling

Test runners, benchmark frameworks, profilers, memory profilers, coverage tools, and property-testing frameworks are pinned in **Section 10**. Substitution requires Section 1 approval.
