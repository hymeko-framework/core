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
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, Ir, NodeRec, RefAtomR, SignedRefR,
};

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
    bench_ir_redundancy_footprint
);
criterion_main!(benches);
