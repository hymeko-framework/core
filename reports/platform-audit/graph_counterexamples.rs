//! Audit-only counterexamples and bounded diagnostics using Graph's existing fixture helpers.
//! Expected values are hand-derived below. Runbook records exact rustc --extern artifacts.
use hymeko::common::ids::{DeclId, NodeId};
use hymeko::ir::incidence::build_incidence_index;
use hymeko::ir::ir::{AnnoR, ArcRec, Ir, RefAtomR, SignedRefR, ValueR};
use hymeko_graph::direct_ir::{
    BergeHeuristic, DirectedProducerConsumer, HyperTraversal, IncidenceWeightCost,
    InformedSearchWorkspace, Undirected, UnitCost, astar_berge_path, dijkstra_berge_path,
};
#[path = "../../../graph/hymeko_graph/tests/direct_ir_common/mod.rs"]
mod dc;
#[path = "../../../graph/hymeko_graph/tests/informed_common/mod.rs"]
mod ic;

#[test]
fn directed_arc_remains_available_after_rejected_entry() {
    // Producer s -> consumer p through entry, producer p -> consumer goal through shared.
    assert_directed_cost("vv {}\ng { s {} p {} goal {} @shared { (+ s, - p, + goal); } @entry { (- s, + p); } }\n", 2.0);
}

#[test]
fn directed_duplicate_entry_slots_preserve_signs() {
    // The second occurrence of s is a producer, so the one-hop path is legal.
    assert_directed_cost("vv {}\ng { s {} goal {} @shared { (+ s, - s, + goal); } }\n", 1.0);
}

fn assert_directed_cost(source: &str, expected: f64) {
    let w = dc::compile(source);
    let idx = build_incidence_index(w.ir()).unwrap();
    let trav = HyperTraversal::new(&idx);
    let mut ws = InformedSearchWorkspace::new();
    let path = dijkstra_berge_path(&trav, w.node("s"), w.node("goal"), &UnitCost, &DirectedProducerConsumer, &mut ws).unwrap();
    assert_eq!(path.as_ref().map(|p| p.total_cost), Some(expected));
}

struct ConsistentBound;
impl BergeHeuristic for ConsistentBound {
    const ADMISSIBLE: bool = true;
    const CONSISTENT: bool = true;
    fn h(&self, _ir: &Ir, node: DeclId) -> f64 {
        if node.raw() == 2 { 1.0 } else { 0.0 }
    }
}

#[test]
fn consistent_astar_preserves_cheapest_entry_to_shared_arc() {
    let mut ir = ic::window_ir(4, 2, false, ic::SignMode::AllPlus);
    ir.arcs = [
        vec![(0, 1.0), (1, 2.0)],
        vec![(0, 1.0), (2, 1.0)],
        vec![(1, 1.0), (2, 1.0), (3, 10.0)],
    ].into_iter().map(|members| ArcRec {
        anno: AnnoR::default(),
        in_edge: DeclId::NONE,
        refs: members.into_iter().map(|(node, weight)| SignedRefR::Plus(RefAtomR {
            target: DeclId::new(node), anno: AnnoR::default(),
            weights: Some(vec![ValueR::Num(weight)]),
        })).collect(),
    }).collect();
    let idx = build_incidence_index(&ir).unwrap();
    let trav = HyperTraversal::new(&idx);
    let cost = IncidenceWeightCost { base: 0.0 };
    let mut ws = InformedSearchWorkspace::new();
    // h = [0,0,1,0]; only nonzero source bound is 1, and every outgoing step costs >= 1.
    // Exact goal distances are [11,10,10,0], proving admissibility independently.
    for (node, distance) in [11.0, 10.0, 10.0, 0.0].into_iter().enumerate() {
        assert!(ConsistentBound.h(&ir, DeclId::new(node)) <= distance);
    }
    for arc in &ir.arcs {
        for from in &arc.refs {
            for to in &arc.refs {
                let target = to.target();
                let h_from = ConsistentBound.h(&ir, from.target());
                let h_to = ConsistentBound.h(&ir, target);
                let step = if target.raw() == 3 { 10.0 } else if target.raw() == 1 && arc.refs.len() == 2 { 2.0 } else { 1.0 };
                assert!(h_from <= step + h_to);
            }
        }
    }
    let oracle = ic::oracle_dijkstra_cost(&ir, &idx, NodeId::new(0), NodeId::new(3), ic::OracleCost::Weight { base: 0.0 });
    assert_eq!(oracle, Some(11.0));
    let dijkstra = dijkstra_berge_path(&trav, NodeId::new(0), NodeId::new(3), &cost, &Undirected, &mut ws).unwrap().unwrap();
    assert_eq!(dijkstra.total_cost, 11.0);
    let astar = astar_berge_path(&trav, NodeId::new(0), NodeId::new(3), &cost, &ConsistentBound, &Undirected, &mut ws).unwrap().unwrap();
    assert_eq!(astar.total_cost, dijkstra.total_cost);
}

#[test]
#[ignore = "bounded diagnostic: three warmups and twenty raw samples per size"]
fn bounded_diamond_enumeration_probe() {
    use hymeko_graph::direct_ir::{BudgetKind, EnumBudget, enumerate_berge_paths};
    use std::fmt::Write;
    use std::time::Instant;

    println!("# sample,diamonds,rep,results,output_steps,bound,elapsed_us");
    for diamonds in [2_usize, 4, 8, 12, 16] {
        let mut source = String::from("vv {}\ng {\n");
        for i in 0..=diamonds {
            writeln!(source, "v{i} {{}}").unwrap();
        }
        for i in 0..diamonds {
            let j = i + 1;
            writeln!(source, "a{i} {{}} b{i} {{}}\n@p{i} {{ (+ v{i}, + a{i}); }}\n@q{i} {{ (+ a{i}, + v{j}); }}\n@r{i} {{ (+ v{i}, + b{i}); }}\n@s{i} {{ (+ b{i}, + v{j}); }}").unwrap();
        }
        source.push_str("}\n");
        let world = dc::compile(&source);
        let index = build_incidence_index(world.ir()).unwrap();
        let start = world.node("v0");
        let goal = world.node(&format!("v{diamonds}"));
        let expected = (1_usize << diamonds).min(4096);
        for rep in 0..23 {
            let began = Instant::now();
            let result = enumerate_berge_paths(
                &index, start, Some(goal), EnumBudget::new(3 * diamonds + 2, 4096, 1_000_000),
            ).unwrap();
            let elapsed = began.elapsed().as_secs_f64() * 1e6;
            assert_eq!(result.items.len(), expected);
            assert_eq!(result.bound, if expected == 4096 { Some(BudgetKind::Results) } else { None });
            assert!(result.items.iter().all(|path| path.hyperedge_len() == 2 * diamonds && path.nodes().last() == Some(goal)));
            let steps: usize = result.items.iter().map(|path| path.hyperedge_len()).sum();
            if rep >= 3 {
                println!("sample,{diamonds},{},{},{steps},{:?},{elapsed:.6}", rep - 3, result.items.len(), result.bound);
            }
        }
    }
}
