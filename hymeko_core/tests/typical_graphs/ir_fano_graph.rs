#![cfg(test)]
mod ir_fano_graph {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use crate::typical_graphs::fano::constants::*;
    use hymeko::common::ids::{DeclId, SymId};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::common::ref_target;
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass::Interned;
    use hymeko::resolution::interner::Interner;
    use hymeko::resolution::{intern_pass, resolve};
    use log::{Level, debug, info, log_enabled};
    use std::time::Instant;

    fn invert_index(
        idx: &resolve::Index,
        it: &Interner,
    ) -> std::collections::HashMap<DeclId, String> {
        let mut inv = std::collections::HashMap::new();

        for (k, did) in idx.by_path.iter() {
            // k: &PathKey
            let s =
                k.0.iter()
                    .map(|&sid| it.resolve(sid).to_string())
                    .collect::<Vec<_>>()
                    .join(".");
            inv.insert(*did, s);
        }

        inv
    }

    // --- segéd: edge DeclId lekérése path alapján ---
    fn did_of_path(idx: &resolve::Index, path: &[SymId]) -> DeclId {
        *idx.by_path
            .get(&PathKey(path.to_vec()))
            .unwrap_or_else(|| panic!("Missing DeclId for path: {:?}", path))
    }

    #[test]
    fn fano_graph_lowers_to_ir_with_correct_arc_targets() -> Result<(), Box<dyn std::error::Error>>
    {
        log_test_header(
            "fano_graph_lowers_to_ir_with_correct_arc_targets",
            "Lowers the Fano graph to IR and confirms each arc hits the expected nodes.",
        );
        let start = Instant::now();
        // 1) parse -> AST<String>
        let source_code =
            parser::read_source_file(FANO_GRAPH_PATH).expect("failed to read source file");

        // 2. Parse it, tying the AST lifetimes to the String
        let d_str = parser::parse_description(&source_code).unwrap();

        // 2) intern -> AST<SymId> + interner
        let Interned { ast, mut interner } = intern_pass::intern_ast(&d_str);

        // 3) resolve index (PathKey -> DeclId)
        let idx = resolve::build_index_sym(&ast, &interner).unwrap();

        // 4) lower -> IR
        let ir = lower_to_ir(&ast, &idx, &mut interner).unwrap();

        // invert index: DeclId -> "fano.n0"
        let inv = invert_index(&idx, &interner);

        // várt incidenciák
        let expected = FANO_EXPECTED_EDGE_TARGETS;

        let fano_sid = interner
            .get_id(FANO_BLOCK_NAME)
            .expect("missing SymId for 'fano'");

        for (ename, nodes) in expected {
            let e_sid = interner
                .get_id(ename)
                .unwrap_or_else(|| panic!("missing SymId for '{ename}'"));

            // edge DeclId = path [fano, eX]
            let edge_did = did_of_path(&idx, &[fano_sid, e_sid]);

            // IR: DeclId -> EdgeId -> EdgeRec
            let edge_id = ir.decl_to_edge[edge_did.0]
                .unwrap_or_else(|| panic!("Edge DeclId {:?} not mapped to EdgeId in IR", edge_did));

            let edge = &ir.edges[edge_id.0];

            if log_enabled!(Level::Debug) {
                debug!(
                    "Checking edge '{ename}' (DeclId: {:?}, EdgeId: {:?})",
                    edge_did, edge_id
                );
                for (i, arc_ref) in edge.arcs.iter().enumerate() {
                    let arc_did = ir
                        .decl_to_arc
                        .iter()
                        .position(|&aid| aid == Some(*arc_ref))
                        .map(|idx| DeclId::new(idx))
                        .unwrap_or_else(|| {
                            panic!("ArcId {:?} not mapped to DeclId in IR", arc_ref)
                        });
                    debug!(
                        "  Arc {i}: DeclId: {:?}, Parent Edge DeclId: {:?}",
                        arc_did, edge_did
                    );
                    let arc = &ir.arcs[arc_ref.0];
                    for (j, r) in arc.refs.iter().enumerate() {
                        let target_did = ref_target(r);
                        let target_name = inv
                            .get(&target_did)
                            .cloned()
                            .unwrap_or_else(|| format!("<unknown {target_did:?}>"));
                        debug!(
                            "    Ref {j}: target DeclId: {:?}, name: {}",
                            target_did, target_name
                        );
                    }
                }
            }

            assert_eq!(edge.arcs.len(), 1, "{ename}: expected exactly 1 arc");

            let arc = &ir.arcs[edge.arcs[0].0];

            // targetok (DeclId) -> "fano.nK"
            let mut got = arc
                .refs
                .iter()
                .map(|r| {
                    let did = ref_target(r);
                    inv.get(&did)
                        .cloned()
                        .unwrap_or_else(|| format!("<unknown {did:?}>"))
                })
                .collect::<Vec<_>>();
            got.sort();

            let mut exp = nodes
                .iter()
                .map(|n| format!("fano.{n}"))
                .collect::<Vec<_>>();
            exp.sort();

            assert_eq!(got, exp, "{ename}: IR arc targets mismatch");
        }

        info!(
            "Verified {} IR edges retained the expected arc targets",
            FANO_EDGE_COUNT
        );

        log_test_footer(
            "fano_graph_lowers_to_ir_with_correct_arc_targets",
            Some(start.elapsed()),
            "IR arcs matched the expected node triplets.",
        );
        Ok(())
    }
}
