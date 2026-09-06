//! End-to-end validation of the public canonical-CBOR facade.

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId, SymId};
use hymeko::common::pathkey::PathKey;
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, Ir, NodeRec, RefAtomR, SignedRefR,
};
use hymeko::ir_stream::DecodeLimits;
use hymeko::module_store::module_store::ModuleKey;
use hymeko::resolution::interner::Interner;
use hymeko::resolution::resolve::Index;
use hymeko::writers::cbor_writer::{CborError, CborPayload, decode, encode_compiled};
use hymeko::writers::ir_writer::IrWriter;
use std::hint::black_box;
use std::mem::size_of;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

fn portable(path: &Path) -> PathBuf {
    PathBuf::from(
        path.components()
            .map(|component| match component {
                Component::Normal(part) => part.to_str().expect("corpus paths are UTF-8"),
                _ => panic!("expected a relative normal path, got {}", path.display()),
            })
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn interned_strings(interner: &Interner) -> Vec<String> {
    interner.iter().map(|(_, value)| value.to_owned()).collect()
}

fn rendered(ir: &hymeko::ir::ir::Ir, interner: &Interner) -> Vec<u8> {
    let mut output = Vec::new();
    IrWriter::new(ir, interner)
        .write_all(&mut output)
        .expect("coherent IR must render");
    output
}

fn expected_imports(
    root: &Path,
    imports: &[(hymeko::common::ids::SymId, ModuleKey)],
) -> Vec<(hymeko::common::ids::SymId, ModuleKey)> {
    let canonical_root = root.canonicalize().expect("corpus root canonicalizes");
    imports
        .iter()
        .map(|(namespace, module)| {
            let relative = module
                .0
                .strip_prefix(&canonical_root)
                .expect("corpus import must stay under the corpus root");
            (*namespace, ModuleKey(portable(relative)))
        })
        .collect()
}

fn assert_round_trip(path: &Path) -> usize {
    let root = crate::unified_ir::support::corpus::corpus_root();
    let (store, compiled) = crate::test_helpers::load_and_lower(path)
        .unwrap_or_else(|error| panic!("{} did not lower: {error:?}", path.display()));
    let bytes = encode_compiled(&root, &compiled, &store.it)
        .unwrap_or_else(|error| panic!("{} did not encode: {error}", path.display()));
    let decoded = decode(&bytes, &DecodeLimits::DEFAULT)
        .unwrap_or_else(|error| panic!("{} did not decode: {error}", path.display()));

    assert_eq!(
        decoded.payload().root_path,
        portable(
            path.strip_prefix(&root)
                .expect("fixture is under corpus root")
        ),
        "root path on {}",
        path.display()
    );
    assert_eq!(
        decoded.canonical_hash(),
        compiled.canon_hash,
        "canonical hash on {}",
        path.display()
    );
    assert_eq!(
        decoded.index().by_path,
        compiled.idx.by_path,
        "path index on {}",
        path.display()
    );
    assert_eq!(
        decoded.index().aliases,
        compiled.idx.aliases,
        "aliases on {}",
        path.display()
    );
    assert_eq!(
        decoded.payload().interned_strings,
        interned_strings(&store.it),
        "interner on {}",
        path.display()
    );
    assert_eq!(
        decoded.payload().imports,
        expected_imports(&root, &compiled.imports),
        "imports on {}",
        path.display()
    );
    assert_eq!(
        rendered(decoded.ir(), decoded.interner()),
        rendered(&compiled.ir, &store.it),
        "rendered semantics on {}",
        path.display()
    );
    compiled.ir.decl_nodes.len()
}

#[test]
fn every_lowerable_corpus_program_round_trips_through_the_public_facade() {
    let files = crate::unified_ir::support::corpus::require_fixtures();
    let mut lowered = 0usize;
    for path in files {
        if crate::test_helpers::load_and_lower(&path).is_ok() {
            assert_round_trip(&path);
            lowered += 1;
        }
    }
    assert_eq!(lowered, 102, "the codec gate must cover the frozen corpus");
}

#[test]
fn deterministic_small_medium_large_corpus_workloads_round_trip() {
    let mut lowerable = crate::unified_ir::support::corpus::require_fixtures()
        .into_iter()
        .filter_map(|path| {
            crate::test_helpers::load_and_lower(&path)
                .ok()
                .map(|(_, compiled)| (compiled.ir.decl_nodes.len(), path))
        })
        .collect::<Vec<_>>();
    lowerable.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    assert!(
        lowerable.len() >= 3,
        "workload selection needs three fixtures"
    );

    let selected = [0, lowerable.len() / 2, lowerable.len() - 1];
    let observed = selected.map(|index| {
        let declarations = assert_round_trip(&lowerable[index].1);
        eprintln!(
            "[cbor workload] declarations={declarations} path={}",
            lowerable[index].1.display()
        );
        declarations
    });
    assert!(
        observed.windows(2).all(|pair| pair[0] <= pair[1]),
        "selected workload scales must be monotonic"
    );
}

#[test]
fn corpus_wire_faults_fail_closed() {
    let root = crate::unified_ir::support::corpus::corpus_root();
    let (path, store, compiled) = crate::unified_ir::support::corpus::require_fixtures()
        .into_iter()
        .find_map(|path| {
            crate::test_helpers::load_and_lower(&path)
                .ok()
                .map(|(store, compiled)| (path, store, compiled))
        })
        .expect("the frozen corpus has a lowerable fixture");
    let bytes = encode_compiled(&root, &compiled, &store.it).unwrap();

    for end in 0..bytes.len() {
        assert!(
            decode(&bytes[..end], &DecodeLimits::DEFAULT).is_err(),
            "{}: truncated prefix of {end} bytes decoded",
            path.display()
        );
    }

    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    payload.canon_hash.0[0] ^= 0xff;
    let mut tampered = Vec::new();
    ciborium::ser::into_writer(&payload, &mut tampered).unwrap();
    assert!(matches!(
        decode(&tampered, &DecodeLimits::DEFAULT),
        Err(CborError::CanonicalHashMismatch { .. })
    ));

    let tiny = DecodeLimits {
        max_message_bytes: u64::try_from(bytes.len().saturating_sub(1)).unwrap(),
        ..DecodeLimits::DEFAULT
    };
    assert!(matches!(
        decode(&bytes, &tiny),
        Err(CborError::MessageTooLarge { .. })
    ));
}

fn memory_evidence_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest directory has a parent")
        .join("target/canonical-cbor-memory.cbor")
}

fn push_child(ir: &mut Ir, parent: DeclId, kind: DeclKind, name: SymId) -> DeclId {
    let id = ir.push_decl(DeclNode {
        kind,
        name,
        parent,
        first_child: DeclId::NONE,
        last_child: DeclId::NONE,
        next_sibling: DeclId::NONE,
        anno: AnnoR::default(),
    });
    ir.decl_hash.push(None);
    let previous = ir.decl_nodes[parent.raw()].last_child;
    if previous.is_none() {
        ir.decl_nodes[parent.raw()].first_child = id;
    } else {
        ir.decl_nodes[previous.raw()].next_sibling = id;
    }
    ir.decl_nodes[parent.raw()].last_child = id;
    id
}

fn incidence_heavy_memory_fixture() -> (Ir, Index, Interner) {
    const NODES: usize = 1_024;
    const EDGES: usize = 4_096;
    const ARITY: usize = 256;

    let mut interner = Interner::new();
    let mut index = Index::new();
    let mut ir = Ir::default();
    let root_name = interner.intern("memory_root");
    let root = ir.push_decl(DeclNode {
        kind: DeclKind::Node,
        name: root_name,
        parent: DeclId::NONE,
        first_child: DeclId::NONE,
        last_child: DeclId::NONE,
        next_sibling: DeclId::NONE,
        anno: AnnoR::default(),
    });
    ir.decl_hash.push(None);
    ir.nodes.push(NodeRec::new(root, Vec::new()));
    ir.bind_node_decl(root, NodeId::new(0)).unwrap();
    index.by_path.insert(PathKey(vec![root_name]), root);

    let nodes = (0..NODES)
        .map(|number| {
            let name = interner.intern(&format!("node_{number}"));
            let decl = push_child(&mut ir, root, DeclKind::Node, name);
            let node = NodeId::new(ir.nodes.len());
            ir.nodes.push(NodeRec::new(decl, Vec::new()));
            ir.bind_node_decl(decl, node).unwrap();
            index.by_path.insert(PathKey(vec![root_name, name]), decl);
            decl
        })
        .collect::<Vec<_>>();
    let arc_name = interner.intern("incidences");

    for edge_number in 0..EDGES {
        let edge_name = interner.intern(&format!("edge_{edge_number}"));
        let edge_decl = push_child(&mut ir, root, DeclKind::Edge, edge_name);
        let edge_id = EdgeId::new(ir.edges.len());
        ir.edges.push(EdgeRec::new(edge_decl, Vec::new()));
        ir.bind_edge_decl(edge_decl, edge_id).unwrap();
        index
            .by_path
            .insert(PathKey(vec![root_name, edge_name]), edge_decl);

        let refs = (0..ARITY)
            .map(|slot| {
                SignedRefR::Neutral(RefAtomR::plain(
                    nodes[(edge_number * ARITY + slot) % nodes.len()],
                ))
            })
            .collect();
        let arc_id = HyperArcId::new(ir.arcs.len());
        ir.arcs.push(ArcRec {
            anno: AnnoR::default(),
            in_edge: edge_decl,
            refs,
        });
        ir.arc_hash.push(None);
        ir.edges[edge_id.raw()].arcs.push(arc_id);
        let arc_decl = push_child(&mut ir, edge_decl, DeclKind::HyperArc, arc_name);
        ir.bind_arc_decl(arc_decl, arc_id).unwrap();
    }
    (ir, index, interner)
}

fn conservative_public_ir_heap_bytes(ir: &Ir) -> usize {
    size_of::<Ir>()
        + ir.decl_hash.capacity() * size_of::<Option<hymeko::ir::hash::HashId>>()
        + ir.arc_hash.capacity() * size_of::<Option<hymeko::ir::hash::HashId>>()
        + ir.decl_nodes.capacity() * size_of::<DeclNode>()
        + ir.nodes.capacity() * size_of::<NodeRec>()
        + ir.edges.capacity() * size_of::<EdgeRec>()
        + ir.arcs.capacity() * size_of::<ArcRec>()
        + ir.nodes
            .iter()
            .map(|node| node.bases.capacity() * size_of::<SignedRefR>())
            .sum::<usize>()
        + ir.edges
            .iter()
            .map(|edge| {
                edge.bases.capacity() * size_of::<SignedRefR>()
                    + edge.arcs.capacity() * size_of::<HyperArcId>()
            })
            .sum::<usize>()
        + ir.arcs
            .iter()
            .map(|arc| arc.refs.capacity() * size_of::<SignedRefR>())
            .sum::<usize>()
}

#[test]
#[ignore = "measurement fixture generator; invoked explicitly by the evidence harness"]
fn write_canonical_cbor_memory_fixture() {
    let (ir, index, interner) = incidence_heavy_memory_fixture();
    let bytes = hymeko::writers::cbor_writer::encode(
        "memory/incidence-heavy.hymeko",
        &ir,
        &index,
        &interner,
        &[],
    )
    .unwrap();
    std::fs::write(memory_evidence_path(), &bytes).unwrap();
    eprintln!(
        "[cbor memory fixture] wire_bytes={} ir_heap_bytes={}",
        bytes.len(),
        conservative_public_ir_heap_bytes(&ir)
    );
}

#[test]
#[ignore = "measurement cell; invoked in a fresh process by the evidence harness"]
fn canonical_cbor_memory_read_baseline() {
    let bytes = std::fs::read(memory_evidence_path()).unwrap();
    eprintln!("[cbor memory baseline] wire_bytes={}", bytes.len());
    black_box(&bytes);
    std::thread::sleep(Duration::from_millis(250));
}

#[test]
#[ignore = "diagnostic cell; separates materialization from semantic/hash validation"]
fn canonical_cbor_memory_materialize_only() {
    let bytes = std::fs::read(memory_evidence_path()).unwrap();
    let payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    eprintln!(
        "[cbor memory materialize] wire_bytes={} ir_heap_bytes={}",
        bytes.len(),
        conservative_public_ir_heap_bytes(&payload.ir)
    );
    black_box(&payload);
    std::thread::sleep(Duration::from_millis(250));
}

#[test]
#[ignore = "measurement cell; invoked in a fresh process by the evidence harness"]
fn canonical_cbor_memory_decode() {
    let bytes = std::fs::read(memory_evidence_path()).unwrap();
    let decoded = decode(&bytes, &DecodeLimits::DEFAULT).unwrap();
    eprintln!(
        "[cbor memory decode] wire_bytes={} ir_heap_bytes={}",
        bytes.len(),
        conservative_public_ir_heap_bytes(decoded.ir())
    );
    black_box(&decoded);
    std::thread::sleep(Duration::from_millis(250));
}
