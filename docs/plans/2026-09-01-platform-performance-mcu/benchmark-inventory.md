# Benchmark inventory (§0) — consolidated from the two read-only audits

## Core (hymeko_core / hymeko_wgpu)
Criterion (registered `[[bench]]`, autobenches=false):
- `hymeko_core/benches/ir_bench.rs` — Ir build/clone/serde(JSON)/canonical_program_hash/lookup/hierarchy.
- `hymeko_core/benches/ir_stream_bench.rs` — validate_payload/sections/reject; assembler staging/replay.
- `hymeko_core/benches/csr_bench.rs` — CPU SpMV/SpMM serial vs parallel crossover (`sparse_fixture`).
- `hymeko_core/benches/interning_bench.rs` — SIMD lexer + parse->intern frontend.
- `hymeko_wgpu/benches/{spmm_bench,chain_bench,arena_bench}.rs` — GPU SpMM cells A-F, activation+chain, arena.
Std::time #[ignore] (reusable against production types):
- `tests/unified_ir/verification/incidence.rs:302` bench_incidence_operator_vs_clique — build/Bx/BTy/BTBx/clique. RUN this session.
- `tests/unified_ir/validation/corpus.rs:385` bench_lowering_corpus — parse/resolve/lower over the corpus.
WGPU example probes (Instant/TSV): phase7_probe (kernel timestamps), phase8_readback_bench, crossover_sweep, sustained_chain.
WebXR: `tests/webxr/mod.rs` bench_compile_vs_validate (#[ignore]). RUN this session.

## Graph (hymeko_graph)
Criterion (`benchmarks/`, own workspace): graph_bench (DFS/BFS/cycle/A*), spine_bench (FIR, std::time main),
incidence_bench (build_edge_incidence), pgraph_bench (MSG/SSG/ABB).
Std::time #[ignore]: `tests/direct_ir_bench.rs` bench_direct_vs_star_vs_clique (reachable A/B/C/D/F) +
bench_direct_ir_ops (ADDED this session: berge_shortest_path/connected_components/dual_reachability/
s_connected_components/batch serial+Rayon — previously unbenched). RUN this session.

## Nagare (D:\hakiko_ai_ws\03_implementation\nagare — foreign, read-only)
`benches/parity_gate.rs`: clifford_fir_forward (FIR), scatter_mean (gather/scatter/message-pass),
runtime predict/step. No CSR SpMV/SpMM, no activation, no HGNN-named, no GPU.
NOT RUN: its `hymeko_graph` path points at the RETIRED `hymeko_framework_rust` (nagare/Cargo.toml:20), and
criterion 0.5 vs 0.8.2 skew. Running it unmodified measures the retired copy, not the current direct-Ir
platform -> excluded per §7 "if available WITHOUT modification" (it is not, without a dependency repoint).

## Duplications flagged (consolidation targets — NOT implemented, measurement phase)
- `SparseSpec`/`DegreeShape`/`Lcg` duplicated byte-identically: `hymeko_core/benches/sparse_fixture.rs` and
  `hymeko_wgpu/src/testing.rs` (both re-implement the retired hymeko_hnn::workloads). Consolidate to one.
- CPU SpMM measured twice: `csr_bench` and `spmm_bench` cells A/B (same functions) -- acknowledged in-code.
- Three independent generators of the same edge->arc->signed-ref topology: ir_bench build_ir, incidence.rs
  build_synth_ir, proto_incidence_bench gen. Prefer `hymeko_core::generators` (library API, currently used by
  no bench).
- Cross-repo: Steiner + Sunflower generators exist in BOTH graph (`hymeko_pgraph/tests/workload/families.rs`)
  and core (`hymeko_core/src/generators.rs`). graph depends on core by git -> reuse the core library, do not copy.
- decl->record map lookup measured twice (Criterion m2a_map_lookup + docs/plans prototypes).

## ir_stream wire format (the MCU C wire-view foundation)
`src/ir_stream/`: 144-byte Envelope (MAGIC 0x484D4B53 "HYMKS", schema_version u16=1, byte-wise LE, msg_type,
stream/seq/txn u64, base/target SnapshotVersion=4xu64, payload_len u64, payload_hash [u8;32] BLAKE3). Section
table `[u32 count][u32 reserved][SectionEntry(32B)xN][bodies]`; SectionEntry{kind u16, element_kind u16,
reserved u32, element_count u64, byte_offset u64, byte_len u64}; SectionKind {EdgeOffsets/EdgeNodes/
IncidenceWeights/NodeFeatures/ModelParams/SignPattern + delta patch kinds + Unknown(u16) skippable}.
MessageType {Hello/SnapshotBegin/SnapshotChunk/DeltaBegin/DeltaChunk/Commit/Abort/...}. **Bans usize on the
wire (mod.rs:40).** Offset+length, digest, deterministic order, skippable unknowns -- already satisfies §10.

## CBOR (dormant; §12 input)
Only `writers/cbor_writer.rs::CborPayload` -- a serde struct (Ir+Index+all interned strings+hash+imports),
BY VALUE (full clone). NOT wired in (no constructor/serializer call), no CBOR crate dependency (ciborium is a
transitive dev-dep of criterion only). Not canonical, not incremental. -> a real CBOR path does not exist.

## §11 portability hazard (serde path, not the ir_stream path)
`Ir` manual serde emits `Id<T>` as bare `usize` (common/ids_impl.rs) and DeclKind/SignedRefR as serde-tagged
enums -- usize width + NONE=usize::MAX + tagged-enum are NOT 32-bit-MCU/C portable. The MCU path must use the
ir_stream fixed-width section encoding, never the serde/JSON path.
