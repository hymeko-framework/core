# Workload matrix (§2)

Each interpretable operation runs at small / medium / large. Topologies (never averaged into one
headline number — each reported per-topology):

Steiner · sunflower · Mycielski · random-uniform · random degree-skew · random hub · grid/lattice ·
VLAN/failure-domain · service-dependency · humanoid · hexapod · chicken/biomedical · many-small-hyperedge ·
few-high-arity · disconnected-components · deep-and-wide declaration hierarchy.

Size points cover: 0/1/2 · 63/64/65 (word-boundary) · ~1e3 · 1e4 · 1e5 · 1e6 incidences.

Small/medium run parser-first from `.hymeko` source; large from a deterministic generator using the
**same canonical Ir contract** (no second model). One shared workload definition is used where repo
boundaries allow; generators are NOT copied across repos.

Every workload reports: declarations · nodes · edges · arcs · incidences · arity distribution ·
max degree · component count · deterministic fingerprint.

## Execution status this session (honest)
- **Executed** (single-host, unified method, representative cells): the operations for which a
  reusable benchmark already exists — see the benchmark inventory. Verified headline claims + A/A null.
- **Cited** from prior trustworthy reports (CLAUDE.md — not re-measured from scratch): the published
  baseline cells (CPU SpMV/SpMM crossovers, incidence throughput, memory).
- **Planned/PROVISIONAL**: the full 16-topology × 6-size × two-host grid is defined here and executed
  only where an existing harness + single quiet-ish host allow; the remainder is a defined plan (the
  second host lacks a toolchain; ambient CPU is borderline). No topology is averaged into a headline.
