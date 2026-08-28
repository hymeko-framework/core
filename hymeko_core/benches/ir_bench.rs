// APPROVED-CORE-EDIT: unified-ir-phase0-audit-only (2026-08-28).
//
// Measurement only. This benchmark adds no production code and changes no
// behaviour; it exists so the Unified IR Phase 0 migration plan rests on
// numbers rather than on estimates.

//! What the canonical IR costs today.
//!
//! Three questions the migration plan needs answered, and which nothing in the
//! repository measured before:
//!
//! 1. **What does building an `Ir` cost?** If `Ir` becomes the canonical mutable
//!    model, full reconstruction is the fallback whenever an incremental path is
//!    unavailable, so its cost is the ceiling on "just rebuild it".
//! 2. **What does canonical hashing cost?** Commit identity for an IR-stream
//!    transaction is a hash of the state or of the delta. If hashing the whole
//!    state is cheap, versioning is easy; if it is not, the design must commit
//!    to incremental identity.
//! 3. **What do the duplicated mappings cost in memory?** `Ir` stores
//!    `decl_to_node`, `decl_to_edge`, `decl_to_arc`, `decl_hash` and `arc_hash`
//!    as dense parallel arrays beside the records that already carry the
//!    reverse edge. That redundancy is what a mutable IR would have to keep
//!    consistent, so its size is part of the decision.
//!
//! Deliberately **not** measured: parsing, lowering from AST, emitting. Those
//! live in sibling repositories that this phase may not modify, and mixing them
//! in would attribute their cost to the IR.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId, SymId};
use hymeko::common::pathkey::PathKey;
use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, Ir, NodeRec, RefAtomR, SignedRefR,
};
use hymeko::resolution::interner::Interner;
use hymeko::resolution::resolve::Index;

/// Append one declaration, keeping every parallel array in lockstep.
///
/// That this is the caller's duty, in five places, is itself an audit finding:
/// nothing in the type system couples `decl_nodes` to `decl_hash`,
/// `decl_to_node`, `decl_to_edge` and `decl_to_arc`, so a mutable IR would have
/// five independent chances to desynchronise on every insertion.
fn push_decl(ir: &mut Ir, root: DeclId, kind: DeclKind, name: usize) -> DeclId {
    let id = DeclId::new(ir.decl_nodes.len());
    ir.decl_nodes.push(DeclNode {
        kind,
        name: SymId::new(name),
        parent: root,
        first_child: DeclId::NONE,
        last_child: DeclId::NONE,
        next_sibling: DeclId::NONE,
        anno: AnnoR::default(),
    });
    ir.decl_hash.push(None);
    ir.decl_to_node.push(None);
    ir.decl_to_edge.push(None);
    ir.decl_to_arc.push(None);

    // Maintain the intrusive sibling list.
    let prev = ir.decl_nodes[root.0].last_child;
    if prev.is_none() {
        ir.decl_nodes[root.0].first_child = id;
    } else {
        ir.decl_nodes[prev.0].next_sibling = id;
    }
    ir.decl_nodes[root.0].last_child = id;
    id
}

/// Build a synthetic but structurally realistic IR.
///
/// `nodes` node declarations, `edges` hyperedge declarations, each edge holding
/// `arity` arcs' worth of signed references. The declaration hierarchy is
/// populated (a single root with every declaration as a child) so the
/// intrusive-list fields are not left in an unrepresentative empty state.
fn build_ir(nodes: usize, edges: usize, arity: usize) -> Ir {
    let mut ir = Ir::default();

    // Root declaration, so parent/first_child/next_sibling are exercised.
    ir.decl_nodes.push(DeclNode {
        kind: DeclKind::Node,
        name: SymId::new(0),
        parent: DeclId::NONE,
        first_child: DeclId::NONE,
        last_child: DeclId::NONE,
        next_sibling: DeclId::NONE,
        anno: AnnoR::default(),
    });
    ir.decl_hash.push(None);
    ir.decl_to_node.push(None);
    ir.decl_to_edge.push(None);
    ir.decl_to_arc.push(None);
    let root = DeclId::new(0);

    let node_decls: Vec<DeclId> = (0..nodes)
        .map(|i| push_decl(&mut ir, root, DeclKind::Node, i + 1))
        .collect();
    for &d in &node_decls {
        let nid = NodeId::new(ir.nodes.len());
        ir.nodes.push(NodeRec::new(d, Vec::new()));
        ir.decl_to_node[d.0] = Some(nid);
    }

    for e in 0..edges {
        let ed = push_decl(&mut ir, root, DeclKind::Edge, nodes + e + 1);
        let eid = EdgeId::new(ir.edges.len());
        ir.edges.push(EdgeRec::new(ed, Vec::new()));
        ir.decl_to_edge[ed.0] = Some(eid);

        let ad = push_decl(&mut ir, root, DeclKind::HyperArc, nodes + edges + e + 1);
        let aid = HyperArcId::new(ir.arcs.len());
        let refs: Vec<SignedRefR> = (0..arity)
            .map(|k| {
                let target = node_decls[(e * arity + k) % node_decls.len().max(1)];
                let atom = RefAtomR {
                    target,
                    anno: AnnoR::default(),
                    weights: None,
                };
                match k % 3 {
                    0 => SignedRefR::Plus(atom),
                    1 => SignedRefR::Minus(atom),
                    _ => SignedRefR::Neutral(atom),
                }
            })
            .collect();
        ir.arcs.push(ArcRec {
            anno: AnnoR::default(),
            in_edge: ed,
            refs,
        });
        ir.arc_hash.push(None);
        ir.decl_to_arc[ad.0] = Some(aid);
        ir.edges[eid.0].arcs.push(aid);
    }

    ir
}

/// Incidences the IR describes: the quantity every downstream view is sized by.
fn incidences(ir: &Ir) -> usize {
    ir.arcs.iter().map(|a| a.refs.len()).sum()
}

fn bench_ir_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_construction");
    for &(nodes, edges, arity) in &[
        (1_000usize, 500usize, 4usize),
        (10_000, 5_000, 4),
        (100_000, 50_000, 4),
        (10_000, 5_000, 32),
    ] {
        let probe = build_ir(nodes, edges, arity);
        let inc = incidences(&probe);
        let label = format!("n{nodes}_e{edges}_a{arity}");
        eprintln!(
            "[ir_construction {label}] decls={} nodes={} edges={} arcs={} incidences={inc}",
            probe.decl_nodes.len(),
            probe.nodes.len(),
            probe.edges.len(),
            probe.arcs.len()
        );
        drop(probe);

        group.throughput(Throughput::Elements(inc as u64));
        if nodes >= 100_000 {
            group
                .sample_size(10)
                .measurement_time(Duration::from_secs(8));
        } else {
            group
                .sample_size(30)
                .measurement_time(Duration::from_secs(5));
        }
        group.bench_function(BenchmarkId::from_parameter(&label), |b| {
            b.iter(|| {
                black_box(build_ir(
                    black_box(nodes),
                    black_box(edges),
                    black_box(arity),
                ))
            });
        });
    }
    group.finish();
}

/// Serialising the whole IR: the crude form of "commit identity over state".
///
/// Measured to establish whether hashing the entire IR per transaction is
/// affordable, or whether commit identity must be incremental. Uses the
/// `Serialize` impls already present, so it measures the existing
/// representation and not a hypothetical one.
fn bench_ir_state_identity(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_state_identity");
    for &(nodes, edges, arity) in &[(1_000usize, 500usize, 4usize), (10_000, 5_000, 4)] {
        let ir = build_ir(nodes, edges, arity);
        let inc = incidences(&ir);
        let json = serde_json::to_vec(&ir).map_or(0, |v| v.len());
        eprintln!("[ir_state_identity n{nodes}] incidences={inc} serialized_json_bytes={json}");

        group.throughput(Throughput::Elements(inc as u64));
        group
            .sample_size(30)
            .measurement_time(Duration::from_secs(5));
        group.bench_function(BenchmarkId::from_parameter(format!("n{nodes}")), |b| {
            b.iter(|| {
                // Hash the declaration and arc tables the way a state digest
                // would have to: every record, in order.
                let mut h = blake3::Hasher::new();
                for d in &black_box(&ir).decl_nodes {
                    h.update(&[d.kind as u8]);
                    h.update(&(d.name.raw() as u64).to_le_bytes());
                    h.update(&(d.parent.raw() as u64).to_le_bytes());
                }
                for a in &black_box(&ir).arcs {
                    h.update(&(a.in_edge.raw() as u64).to_le_bytes());
                    for r in &a.refs {
                        h.update(&[r.sign() as u8]);
                        h.update(&(r.target().raw() as u64).to_le_bytes());
                    }
                }
                black_box(*h.finalize().as_bytes())
            });
        });
    }
    group.finish();
}

/// The memory the duplicated mappings occupy.
///
/// Reported through `eprintln!` rather than as a timing: it is a size, and a
/// timing benchmark that printed a size would be misleading. Criterion needs at
/// least one measured routine per group, so the trivial one below exists only to
/// keep the group well-formed.
fn bench_ir_redundancy_footprint(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_redundancy");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(2));

    {
        let (nodes, edges, arity) = (10_000usize, 5_000usize, 4usize);
        let ir = build_ir(nodes, edges, arity);
        let d = ir.decl_nodes.len();
        let a = ir.arcs.len();
        let opt_id = core::mem::size_of::<Option<NodeId>>();
        let opt_hash = core::mem::size_of::<Option<hymeko::ir::hash::HashId>>();
        let derived = d * opt_id * 3 + d * opt_hash + a * opt_hash;
        let records = d * core::mem::size_of::<DeclNode>()
            + ir.nodes.len() * core::mem::size_of::<NodeRec>()
            + ir.edges.len() * core::mem::size_of::<EdgeRec>()
            + a * core::mem::size_of::<ArcRec>();
        eprintln!(
            "[ir_redundancy] decls={d} arcs={a}  \
             derived_maps={derived} B  record_tables={records} B  \
             derived/records={:.1}%  size_of(Id)={} size_of(Option<NodeId>)={opt_id}",
            derived as f64 / records as f64 * 100.0,
            core::mem::size_of::<NodeId>(),
        );

        group.bench_function("decl_to_node_lookup", |b| {
            b.iter(|| {
                let mut hits = 0usize;
                for i in 0..black_box(&ir).decl_to_node.len() {
                    if ir.decl_to_node[i].is_some() {
                        hits += 1;
                    }
                }
                black_box(hits)
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_ir_construction,
    bench_ir_state_identity,
    bench_ir_redundancy_footprint,
    bench_ir_lookup,
    bench_ir_clone,
    bench_ir_canonical_hash,
    bench_ir_serde,
    bench_m2a_decl_lookup,
    bench_m2a_record_lookup,
    bench_m2a_map_lookup,
    bench_m2a_iteration,
    bench_m2a_hierarchy
);
criterion_main!(benches);

// ---------------------------------------------------------------------------
// M0.5 Part F additions.
//
// APPROVED-CORE-EDIT: unified-ir-m0-5-tests-and-benchmarks (2026-08-28).
//
// Scope is deliberately narrow: only paths nothing else in the workspace
// measures. Already covered elsewhere, and therefore NOT duplicated here
// (CLAUDE.md 6.1):
//
//   HyperGraphView::from_ir  -> hymeko_hnn/benches/hnn_bench.rs groups
//                               "view_construction_from_ir" and
//                               "view_from_ir_weight_path"
//   clique/star expansion    -> hnn_bench "representation_expansion",
//                               "representation_clique_growth"
//   CSR SpMV / SpMM          -> hymeko_core/benches/csr_bench.rs
//
// Not measurable from this crate, reported as MISSING rather than substituted:
// the editor->compiler bridge, the compiler->editor bridge, and the round trip.
// They live in transformation/hymeko_emitter, which depends on hymeko_core;
// benchmarking them from here would invert the dependency.
// ---------------------------------------------------------------------------

/// Build an IR together with the `Index` and `Interner` the canonical hash
/// needs. Paths are two segments deep, matching the real-corpus median of 3
/// measured in M0.5 Part E rather than a synthetic extreme.
fn build_indexed(nodes: usize, edges: usize, arity: usize) -> (Ir, Index, Interner) {
    let mut it = Interner::new();
    let mut idx = Index::new();
    let mut ir = Ir::default();
    let root_sym = it.intern("m");

    fn push(ir: &mut Ir, idx: &mut Index, root: SymId, leaf: SymId, kind: DeclKind) -> DeclId {
        let id = DeclId::new(ir.decl_nodes.len());
        ir.decl_nodes.push(DeclNode {
            kind,
            name: leaf,
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        ir.decl_hash.push(None);
        ir.decl_to_node.push(None);
        ir.decl_to_edge.push(None);
        ir.decl_to_arc.push(None);
        idx.by_path.insert(PathKey(vec![root, leaf]), id);
        id
    }

    let node_decls: Vec<DeclId> = (0..nodes)
        .map(|i| {
            let s = it.intern(&format!("n{i}"));
            let d = push(&mut ir, &mut idx, root_sym, s, DeclKind::Node);
            let nid = NodeId::new(ir.nodes.len());
            ir.nodes.push(NodeRec::new(d, Vec::new()));
            ir.decl_to_node[d.0] = Some(nid);
            d
        })
        .collect();

    for e in 0..edges {
        let s = it.intern(&format!("e{e}"));
        let ed = push(&mut ir, &mut idx, root_sym, s, DeclKind::Edge);
        let eid = EdgeId::new(ir.edges.len());
        ir.edges.push(EdgeRec::new(ed, Vec::new()));
        ir.decl_to_edge[ed.0] = Some(eid);

        let aid = HyperArcId::new(ir.arcs.len());
        let refs: Vec<SignedRefR> = (0..arity)
            .map(|k| {
                let target = node_decls[(e * arity + k) % node_decls.len().max(1)];
                let atom = RefAtomR {
                    target,
                    anno: AnnoR::default(),
                    weights: None,
                };
                match k % 3 {
                    0 => SignedRefR::Plus(atom),
                    1 => SignedRefR::Minus(atom),
                    _ => SignedRefR::Neutral(atom),
                }
            })
            .collect();
        ir.arcs.push(ArcRec {
            anno: AnnoR::default(),
            in_edge: ed,
            refs,
        });
        ir.arc_hash.push(None);
        ir.edges[eid.0].arcs.push(aid);
    }

    (ir, idx, it)
}

/// Lookup: the axis M2d could regress when the stored `decl_to_*` maps become a
/// derived index. Measures the current dense path so a later change has a
/// baseline to be compared against.
fn bench_ir_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_lookup");
    for &(nodes, edges) in &[(1_000usize, 500usize), (10_000, 5_000)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        let ids: Vec<DeclId> = (0..ir.decl_nodes.len()).map(DeclId::new).collect();
        group.throughput(Throughput::Elements(ids.len() as u64));
        group
            .sample_size(30)
            .measurement_time(Duration::from_secs(5));

        group.bench_function(BenchmarkId::new("decl_node_dense", nodes), |b| {
            b.iter(|| {
                let mut acc = 0usize;
                for &d in black_box(&ids) {
                    acc += ir.decl_nodes[d.0].name.raw();
                }
                black_box(acc)
            });
        });
        group.bench_function(BenchmarkId::new("decl_to_node_dense", nodes), |b| {
            b.iter(|| {
                let mut hits = 0usize;
                for &d in black_box(&ids) {
                    if ir.decl_to_node[d.0].is_some() {
                        hits += 1;
                    }
                }
                black_box(hits)
            });
        });
    }
    group.finish();
}

/// Clone: bounds any transactional overlay that copies before mutating.
fn bench_ir_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_clone");
    for &(nodes, edges) in &[(1_000usize, 500usize), (10_000, 5_000)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        group.throughput(Throughput::Elements(ir.decl_nodes.len() as u64));
        group
            .sample_size(30)
            .measurement_time(Duration::from_secs(5));
        group.bench_function(BenchmarkId::from_parameter(nodes), |b| {
            b.iter(|| black_box(black_box(&ir).clone()));
        });
    }
    group.finish();
}

/// The real canonical hash, not a flat BLAKE3 sweep.
///
/// The Phase 0 appendix measured a flat sweep and said so explicitly: it was a
/// lower bound. This measures `canonical_program_hash` itself - path map,
/// skeleton forest, subtree fold, Prufer encoding and incidence bytes - which
/// is the figure the 10 ms commit budget must actually be compared against.
fn bench_ir_canonical_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_canonical_hash");
    for &(nodes, edges, arity) in &[
        (1_000usize, 500usize, 4usize),
        (10_000, 5_000, 4),
        (10_000, 5_000, 32),
    ] {
        let (ir, idx, it) = build_indexed(nodes, edges, arity);
        let incidences: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();
        eprintln!(
            "[ir_canonical_hash n{nodes}_a{arity}] decls={} incidences={incidences}",
            ir.decl_nodes.len()
        );
        group.throughput(Throughput::Elements(incidences as u64));
        group
            .sample_size(20)
            .measurement_time(Duration::from_secs(6));
        group.bench_function(
            BenchmarkId::from_parameter(format!("n{nodes}_a{arity}")),
            |b| {
                b.iter(|| {
                    black_box(canonical_program_hash(
                        CanonHashCfg::default(),
                        black_box(&idx),
                        black_box(&ir),
                        black_box(&it),
                    ))
                });
            },
        );
    }
    group.finish();
}

/// Serialization, separated from construction and from hashing.
fn bench_ir_serde(c: &mut Criterion) {
    let mut group = c.benchmark_group("ir_serde");
    for &(nodes, edges) in &[(1_000usize, 500usize), (10_000, 5_000)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        let json = serde_json::to_vec(&ir).expect("serialize");
        eprintln!("[ir_serde n{nodes}] json_bytes={}", json.len());
        group.throughput(Throughput::Bytes(json.len() as u64));
        group
            .sample_size(20)
            .measurement_time(Duration::from_secs(5));

        group.bench_function(BenchmarkId::new("serialize", nodes), |b| {
            b.iter(|| black_box(serde_json::to_vec(black_box(&ir)).expect("serialize")));
        });
        group.bench_function(BenchmarkId::new("deserialize", nodes), |b| {
            b.iter(|| {
                let v: Ir = serde_json::from_slice(black_box(&json)).expect("deserialize");
                black_box(v)
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// M2a Part E - accessor versus direct access.
//
// APPROVED-CORE-EDIT: unified-ir-m2a-accessor-api (2026-08-28).
//
// Variants are registered in ALTERNATING rounds (direct, accessor, direct,
// accessor, ...) rather than as two blocks. Criterion runs registrations in
// order, so blocked ordering would alias thermal drift and ambient load onto
// the comparison - the same discipline the M0.5 protocol requires.
// ---------------------------------------------------------------------------

const M2A_ROUNDS: usize = 3;

/// Declaration lookup: raw index versus the checked accessor.
fn bench_m2a_decl_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("m2a_decl_lookup");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(3));

    for &(nodes, edges) in &[(1_000usize, 500usize), (10_000, 5_000)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        let ids: Vec<DeclId> = (0..ir.decl_nodes.len()).map(DeclId::new).collect();
        group.throughput(Throughput::Elements(ids.len() as u64));

        for round in 0..M2A_ROUNDS {
            group.bench_function(BenchmarkId::new(format!("direct_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for &d in black_box(&ids) {
                        acc += ir.decl_nodes[d.0].name.raw();
                    }
                    black_box(acc)
                });
            });
            // Fair control: direct access doing the SAME work the accessor does
            // - reject NONE, bounds-check, branch on absence. Without this the
            // comparison charges the accessor for safety the direct path never
            // provided, and reports a "regression" that is really a difference
            // in contract.
            group.bench_function(
                BenchmarkId::new(format!("direct_guarded_r{round}"), nodes),
                |b| {
                    b.iter(|| {
                        let mut acc = 0usize;
                        let len = ir.decl_nodes.len();
                        for &d in black_box(&ids) {
                            if d.is_some() && d.0 < len {
                                acc += ir.decl_nodes[d.0].name.raw();
                            }
                        }
                        black_box(acc)
                    });
                },
            );
            group.bench_function(BenchmarkId::new(format!("accessor_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for &d in black_box(&ids) {
                        if let Some(n) = ir.decl_node(d) {
                            acc += n.name.raw();
                        }
                    }
                    black_box(acc)
                });
            });
        }
    }
    group.finish();
}

/// Typed record lookup: raw index versus `Ir::node`.
fn bench_m2a_record_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("m2a_record_lookup");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(3));

    for &(nodes, edges) in &[(10_000usize, 5_000usize)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        let ids: Vec<NodeId> = (0..ir.nodes.len()).map(NodeId::new).collect();
        group.throughput(Throughput::Elements(ids.len() as u64));

        for round in 0..M2A_ROUNDS {
            group.bench_function(BenchmarkId::new(format!("direct_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for &n in black_box(&ids) {
                        acc += ir.nodes[n.0].decl.0;
                    }
                    black_box(acc)
                });
            });
            group.bench_function(BenchmarkId::new(format!("accessor_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for &n in black_box(&ids) {
                        if let Some(r) = ir.node(n) {
                            acc += r.decl.0;
                        }
                    }
                    black_box(acc)
                });
            });
        }
    }
    group.finish();
}

/// Declaration to record id: raw map index versus the existing `as_node`, and
/// versus the new `node_for_decl` which additionally verifies the back-link.
fn bench_m2a_map_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("m2a_map_lookup");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(3));

    for &(nodes, edges) in &[(10_000usize, 5_000usize)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        let ids: Vec<DeclId> = (0..ir.decl_nodes.len()).map(DeclId::new).collect();
        group.throughput(Throughput::Elements(ids.len() as u64));

        for round in 0..M2A_ROUNDS {
            group.bench_function(BenchmarkId::new(format!("direct_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut hits = 0usize;
                    for &d in black_box(&ids) {
                        if ir.decl_to_node[d.0].is_some() {
                            hits += 1;
                        }
                    }
                    black_box(hits)
                });
            });
            group.bench_function(BenchmarkId::new(format!("as_node_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut hits = 0usize;
                    for &d in black_box(&ids) {
                        if ir.as_node(d).is_some() {
                            hits += 1;
                        }
                    }
                    black_box(hits)
                });
            });
            group.bench_function(
                BenchmarkId::new(format!("for_decl_checked_r{round}"), nodes),
                |b| {
                    b.iter(|| {
                        let mut hits = 0usize;
                        for &d in black_box(&ids) {
                            if ir.node_for_decl(d).is_some() {
                                hits += 1;
                            }
                        }
                        black_box(hits)
                    });
                },
            );
        }
    }
    group.finish();
}

/// Iteration: raw `Vec` iteration versus the typed-id iterator.
fn bench_m2a_iteration(c: &mut Criterion) {
    let mut group = c.benchmark_group("m2a_iteration");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(3));

    for &(nodes, edges) in &[(1_000usize, 500usize), (10_000, 5_000)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        group.throughput(Throughput::Elements(ir.decl_nodes.len() as u64));

        for round in 0..M2A_ROUNDS {
            group.bench_function(BenchmarkId::new(format!("direct_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for d in black_box(&ir).decl_nodes.iter() {
                        acc += d.name.raw();
                    }
                    black_box(acc)
                });
            });
            group.bench_function(BenchmarkId::new(format!("accessor_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for (_, d) in black_box(&ir).decls() {
                        acc += d.name.raw();
                    }
                    black_box(acc)
                });
            });
        }
    }
    group.finish();
}

/// Hierarchy: raw field read versus the total accessor.
fn bench_m2a_hierarchy(c: &mut Criterion) {
    let mut group = c.benchmark_group("m2a_hierarchy");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(3));

    for &(nodes, edges) in &[(10_000usize, 5_000usize)] {
        let (ir, _, _) = build_indexed(nodes, edges, 4);
        let ids: Vec<DeclId> = (0..ir.decl_nodes.len()).map(DeclId::new).collect();
        group.throughput(Throughput::Elements(ids.len() as u64));

        for round in 0..M2A_ROUNDS {
            group.bench_function(BenchmarkId::new(format!("direct_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for &d in black_box(&ids) {
                        acc += ir.decl_nodes[d.0].parent.0 & 1;
                    }
                    black_box(acc)
                });
            });
            group.bench_function(BenchmarkId::new(format!("accessor_r{round}"), nodes), |b| {
                b.iter(|| {
                    let mut acc = 0usize;
                    for &d in black_box(&ids) {
                        if let Some(p) = ir.parent_of(d) {
                            acc += p.0 & 1;
                        }
                    }
                    black_box(acc)
                });
            });
        }
    }
    group.finish();
}
