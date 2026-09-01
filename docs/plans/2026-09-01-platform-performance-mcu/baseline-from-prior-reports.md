# Baseline table from prior reports (citable; do not re-measure from scratch)

| operation | size | metric | value | source report |
|---|---|---|---|---|
| IncidenceIndex build | nnz 1e6, arity 5 | cold ms | 139 ms | 2026-09-01-incidence-navigation-corrective |
| IncidenceIndex build | nnz 1e6, arity 20 | cold ms | 70 ms | " |
| bytes/incidence (index) | nnz 1e6 | retained | 12.4–14.8 B/inc | " |
| Bx (node_to_arc) | nnz 1e6 | resident ms | 15–25 ms | " |
| Bᵀy (arc_to_node) | nnz 1e6 | resident ms | 168–243 ms (~10× Bx) | " |
| BᵀBx (star_operator) | nnz 1e6 | resident ms | 207–314 ms | " |
| clique/2-section build (control) | nnz 1e6 | cold ms | 193–480 ms (1.4–6.8×) | " |
| resident incidence throughput | up to 1e6 | ns/incidence | 202–606 ns/inc (topology-dep; VLAN worst) | 2026-08-30-large-hypergraph-performance |
| canonical memory | 20k incidences | bytes/decl | ~146 B/decl canonical + 89 B/decl derived | " |
| serialized JSON (diagnostic) | 20k incidences | bytes | ~5.86 MB | " |
| CPU CSR SpMV serial | 1k → 4.2M nnz | µs/ms | 0.53 µs → 5.23 ms | 2026-08-25-published-baseline |
| CPU CSR SpMV Rayon | top end | µs | 44 → 537 µs (up to 9.7×) | " |
| CSR SpMV serial→Rayon crossover | — | nnz | PAR_SPMV_NNZ_THRESHOLD = 81 920 | " |
| direct-Ir reachable (A) | nnz 1e6, arity 5 | resident ms | 14.1 ms | 2026-09-01-graph-direct-ir-algorithms |
| flat sparse-star (C, control) | nnz 1e6, arity 5 | resident ms | 1.0 ms (A ≈ 13× C) | " |
| clique (D, control) | nnz 1e6, arity 5 | resident ms | 0.66 ms; build 60.7 ms (4.3×) | " |
| WebXR validate | 1e5 entities | resident ms | 32 ms (0.32 µs/entity) after linearization | 2026-09-01-webxr-semantic-contract-completion |

Notes: prior hosts were also "not exclusively quiet" (large-hypergraph report), so these too are
best read as scaling signals + crossovers, not absolute regression gates. This survey verifies the
headline claims (incidence Bᵀy≈10×Bx; direct≈13×flat-star; webxr 522→32ms) with the unified method + an
A/A null, and cites the rest.
