// APPROVED-CORE-EDIT: unified-ir-m0-6-hash-and-traversal (2026-08-28).
//
// Additive: a read-only validator. No existing type, storage layout or hash is
// changed by this module.

//! Hierarchy / `Index` coherence validation.
//!
//! The IR hierarchy is canonical. This validator proves that a derived resolution
//! `Index` describes the same declarations before the compiler discards that index.
//!
//! # Preconditions
//!
//! None. The validator is total: it accepts any `Ir`/`Index` pair, including
//! malformed ones, and terminates on all of them. That is the point — it is the
//! thing that reports malformation.
//!
//! # Postconditions
//!
//! Returns `Ok(())` when no check fires, otherwise `Err(CoherenceReport)` whose
//! `findings` are ordered deterministically and capped.

use crate::common::ids::{DeclId, SymId};
use crate::ir::ir::{DeclKind, Ir, dispatch};
use crate::ir::ir::{ModelFacet, PortDirection, TensorRole};
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;

/// How much the validator is allowed to report before it stops collecting.
#[derive(Copy, Clone, Debug)]
pub struct CoherenceLimits {
    /// Maximum findings retained. Further findings are counted, not stored.
    pub max_findings: usize,
    /// Maximum links followed in one sibling or ancestor walk before the walk
    /// is declared cyclic. Bounds the validator on hostile input.
    pub max_walk: usize,
}

impl Default for CoherenceLimits {
    fn default() -> Self {
        Self {
            max_findings: 64,
            max_walk: 1 << 20,
        }
    }
}

/// One divergence. Deliberately carries ids and small counts only — never a
/// caller-supplied string — so a hostile `Ir` cannot make the report echo a
/// large payload back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// A compact payload handle is invalid, or an editable annotation slot is
    /// shared by another declaration, arc or reference.
    InvalidPayload { field: &'static str, id: usize },
    /// Optional source-file metadata must cover declarations with valid file IDs.
    InvalidSourceProvenance,
    /// The `Index` maps a path to a `DeclId` outside `decl_nodes`.
    IndexedDeclOutOfRange { decl: DeclId, decl_count: usize },
    /// Two different paths map to the same declaration.
    DeclHasMultiplePaths { decl: DeclId },
    /// A declaration reachable in the arena has no path in the `Index`.
    DeclMissingFromIndex { decl: DeclId, kind: DeclKind },
    /// Path-prefix parenthood and the intrusive `parent` link disagree.
    ParentMismatch {
        decl: DeclId,
        by_path: DeclId,
        by_link: DeclId,
    },
    /// A child's `parent` does not point back at the declaration listing it.
    ChildBacklinkMismatch {
        parent: DeclId,
        child: DeclId,
        child_parent: DeclId,
    },
    /// `first_child` is set but `last_child` is not reachable from it, or the
    /// sibling chain exceeded `max_walk`.
    SiblingChainBroken { parent: DeclId, walked: usize },
    /// Following `parent` links did not reach a root within `max_walk`.
    ParentCycle { decl: DeclId, walked: usize },
    /// `decl_to_node` / `decl_to_edge` / `decl_to_arc` disagrees with `kind`.
    KindMapMismatch { decl: DeclId, kind: DeclKind },
    /// A declaration maps into more than one concrete record table.
    MultipleConcreteRecords { decl: DeclId },
    /// A compact dispatch slot carries the reserved tag or is otherwise malformed.
    ReservedDispatchSlot { decl: DeclId },
    /// A concrete record's `decl` back-pointer does not match the declaration
    /// that maps to it.
    RecordBacklinkMismatch { decl: DeclId, record_decl: DeclId },
    /// A parallel array is not the same length as `decl_nodes`.
    ParallelArrayLengthMismatch {
        array: &'static str,
        len: usize,
        expected: usize,
    },
}

/// The result of a validation run.
#[derive(Clone, Debug)]
pub struct CoherenceReport {
    /// Retained findings, in deterministic order (declaration index, then the
    /// order the checks are written).
    pub findings: Vec<Finding>,
    /// Total findings observed, which may exceed `findings.len()`.
    pub total: usize,
    /// True when `total` exceeded `max_findings`.
    pub truncated: bool,
}

impl CoherenceReport {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

struct Collector {
    limits: CoherenceLimits,
    findings: Vec<Finding>,
    total: usize,
}

#[derive(Clone, Copy)]
enum ParentChain {
    Unseen,
    Active,
    Rooted(usize),
    Invalid(usize),
    Cyclic,
}

/// Classify the parent functional graph once, then report in declaration order.
/// Distances include the classified vertex; diagnostics start at its parent.
/// Each vertex enters/exits the explicit stack at most once, including cycles.
fn validate_parent_chains(ir: &Ir, collector: &mut Collector) {
    let n = ir.decl_nodes.len();
    let mut chains = vec![ParentChain::Unseen; n];
    let mut path = Vec::with_capacity(n);
    for start in 0..n {
        if !matches!(chains[start], ParentChain::Unseen) {
            continue;
        }
        let mut current = DeclId::new(start);
        let mut outcome = loop {
            if current.is_none() {
                break ParentChain::Rooted(0);
            }
            if current.raw() >= n {
                break ParentChain::Invalid(0);
            }
            let vertex = current.raw();
            match chains[vertex] {
                ParentChain::Unseen => {
                    chains[vertex] = ParentChain::Active;
                    path.push(vertex);
                    current = ir.decl_nodes[vertex].parent;
                }
                ParentChain::Active => break ParentChain::Cyclic,
                completed => break completed,
            }
        };
        while let Some(vertex) = path.pop() {
            outcome = match outcome {
                ParentChain::Rooted(distance) => ParentChain::Rooted(distance + 1),
                ParentChain::Invalid(distance) => ParentChain::Invalid(distance + 1),
                ParentChain::Cyclic => ParentChain::Cyclic,
                _ => unreachable!("parent walk must have a completed outcome"),
            };
            chains[vertex] = outcome;
        }
    }
    // Preserve the existing finite-limit diagnostics without performing those
    // repeated walks. Saturation also makes a cyclic MAX-limit input terminate.
    let stopped = collector.limits.max_walk.saturating_add(1);
    for (declaration, chain) in chains.into_iter().enumerate() {
        let walked = match chain {
            ParentChain::Rooted(distance) => (distance - 1 > stopped).then_some(stopped),
            ParentChain::Invalid(distance) => Some((distance - 1).min(stopped)),
            ParentChain::Cyclic => Some(stopped),
            _ => unreachable!("every parent chain must have been classified"),
        };
        if let Some(walked) = walked {
            collector.push(Finding::ParentCycle {
                decl: DeclId::new(declaration),
                walked,
            });
        }
    }
}

impl Collector {
    fn payloads(&mut self, ir: &Ir) {
        let mut owned = rustc_hash::FxHashSet::default();
        for id in ir
            .decl_nodes
            .iter()
            .map(|d| d.anno)
            .chain(ir.arcs.iter().map(|a| a.anno))
        {
            if id.is_some() && (ir.annotation(id).is_none() || !owned.insert(id)) {
                self.push(Finding::InvalidPayload {
                    field: "exclusive annotation",
                    id: id.raw(),
                });
            }
        }
        for id in ir
            .nodes
            .iter()
            .map(|n| n.bases)
            .chain(ir.edges.iter().map(|e| e.bases))
        {
            match ir.bases(id) {
                Some(refs) => self.reference_payloads(ir, refs, &owned),
                None => self.push(Finding::InvalidPayload {
                    field: "base list",
                    id: id.raw(),
                }),
            }
        }
        for arc in &ir.arcs {
            self.reference_payloads(ir, &arc.refs, &owned);
        }
    }

    fn reference_payloads(
        &mut self,
        ir: &Ir,
        refs: &[crate::ir::ir::SignedRefR],
        owned: &rustc_hash::FxHashSet<crate::common::ids::AnnotationId>,
    ) {
        for reference in refs {
            let id = reference.annotation_id();
            if owned.contains(&id) {
                self.push(Finding::InvalidPayload {
                    field: "exclusive annotation",
                    id: id.raw(),
                });
            }
            if let Err(error) = ir.reference_atom(reference) {
                let (field, id) = match error {
                    crate::ir::ir::ReferenceError::Annotation(id) => {
                        ("reference annotation", id.raw())
                    }
                    crate::ir::ir::ReferenceError::Weights(id) => ("reference weights", id.raw()),
                };
                self.push(Finding::InvalidPayload { field, id });
            }
        }
    }

    fn push(&mut self, f: Finding) {
        self.total += 1;
        if self.findings.len() < self.limits.max_findings {
            self.findings.push(f);
        }
    }
}

/// Validate that the intrusive hierarchy and the path `Index` describe the same
/// structure, and that the declaration/record maps are self-consistent.
///
/// # Errors
///
/// Returns [`CoherenceReport`] when any check fires. The report is bounded by
/// `limits` and ordered deterministically.
pub fn validate_hierarchy_index_coherence(
    ir: &Ir,
    index: &Index,
    it: &Interner,
    limits: CoherenceLimits,
) -> Result<(), CoherenceReport> {
    let n = ir.decl_nodes.len();
    let mut c = Collector {
        limits,
        findings: Vec::new(),
        total: 0,
    };
    if ir.validate_source_provenance().is_err() {
        c.push(Finding::InvalidSourceProvenance);
    }
    c.payloads(ir);

    // --- parallel array lengths -------------------------------------------
    if ir.decl_to_record.len() != n {
        c.push(Finding::ParallelArrayLengthMismatch {
            array: "decl_to_record",
            len: ir.decl_to_record.len(),
            expected: n,
        });
    }

    // --- index -> declaration ---------------------------------------------
    // Deterministic order: sort the index by (decl index, path) before use, so
    // a `HashMap` iteration order cannot reorder the report.
    // Paths stay in the existing Index. Keep full-width typed IDs: narrowing
    // them would let distinct malformed prefixes compare equal.
    let mut indexed: Vec<(DeclId, &[SymId])> = index
        .by_path
        .iter()
        .map(|(pk, &d)| (d, pk.0.as_slice()))
        .collect();
    // Path keys are unique, so the total ordering has no ties requiring
    // stability; an unstable sort also avoids a second sorting buffer.
    indexed.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    let mut path_of: Vec<Option<usize>> = vec![None; n];
    for (slot, (decl, _)) in indexed.iter().enumerate() {
        if decl.0 >= n {
            c.push(Finding::IndexedDeclOutOfRange {
                decl: *decl,
                decl_count: n,
            });
            continue;
        }
        if path_of[decl.0].is_some() {
            c.push(Finding::DeclHasMultiplePaths { decl: *decl });
        } else {
            path_of[decl.0] = Some(slot);
        }
    }

    // Path-prefix parent: the nearest strictly-shorter prefix that is itself
    // indexed. Mirrors what `canonical_hash::SkeletonForest::new` computes.
    for (decl_idx, slot) in path_of.iter().enumerate() {
        let Some(slot) = *slot else {
            // Not in the index. Only a finding for kinds that carry a name;
            // anonymous arcs legitimately have no path (see policy below).
            let kind = ir.decl_nodes[decl_idx].kind;
            if matches!(kind, DeclKind::Node | DeclKind::Edge) {
                c.push(Finding::DeclMissingFromIndex {
                    decl: DeclId::new(decl_idx),
                    kind,
                });
            }
            continue;
        };
        let path = indexed[slot].1;
        let mut by_path = DeclId::NONE;
        for cut in (0..path.len()).rev() {
            if let Some(&p) = index.by_path.get(&path[..cut]) {
                by_path = p;
                break;
            }
        }
        let by_link = ir.decl_nodes[decl_idx].parent;
        if by_path != by_link {
            c.push(Finding::ParentMismatch {
                decl: DeclId::new(decl_idx),
                by_path,
                by_link,
            });
        }
    }

    // --- intrusive child lists --------------------------------------------
    for p in 0..n {
        let first = ir.decl_nodes[p].first_child;
        let last = ir.decl_nodes[p].last_child;
        if first.is_none() {
            continue;
        }
        let mut cur = first;
        let mut walked = 0usize;
        let mut saw_last = false;
        while cur.is_some() {
            if cur.0 >= n || walked > limits.max_walk {
                c.push(Finding::SiblingChainBroken {
                    parent: DeclId::new(p),
                    walked,
                });
                break;
            }
            let child_parent = ir.decl_nodes[cur.0].parent;
            if child_parent.0 != p {
                c.push(Finding::ChildBacklinkMismatch {
                    parent: DeclId::new(p),
                    child: cur,
                    child_parent,
                });
            }
            if cur == last {
                saw_last = true;
            }
            cur = ir.decl_nodes[cur.0].next_sibling;
            walked += 1;
        }
        if !saw_last && walked <= limits.max_walk {
            c.push(Finding::SiblingChainBroken {
                parent: DeclId::new(p),
                walked,
            });
        }
    }

    // --- parent acyclicity -------------------------------------------------
    validate_parent_chains(ir, &mut c);

    validate_record_bindings(ir, &mut c);

    let _ = it; // reserved: name-based findings would need it; none are emitted

    if c.total == 0 {
        Ok(())
    } else {
        let truncated = c.total > c.findings.len();
        Err(CoherenceReport {
            findings: c.findings,
            total: c.total,
            truncated,
        })
    }
}

// Length mismatches are already reported by the caller. Missing slots must
// not be indexed while collecting the remaining bounded diagnostics.
fn validate_record_bindings(ir: &Ir, c: &mut Collector) {
    // --- kind / concrete-record agreement (compact dispatch, option D) ------
    // The single `decl_to_record` slot makes "more than one concrete record" a
    // structural impossibility. We instead validate: the slot decodes (reserved /
    // malformed tags are always an error), the slot tag agrees with `DeclKind`, the
    // record index is in range, and the record's back-pointer matches. NONE is
    // permitted (partial/malformed test states).
    for (d, &slot) in ir
        .decl_to_record
        .iter()
        .take(ir.decl_nodes.len())
        .enumerate()
    {
        let did = DeclId::new(d);
        let kind = ir.decl_nodes[d].kind;
        let decoded = match dispatch::decode_checked(slot) {
            Ok(x) => x,
            Err(_) => {
                c.push(Finding::ReservedDispatchSlot { decl: did });
                continue;
            }
        };
        let Some((tag, idx)) = decoded else { continue };
        let tag_kind = match tag {
            dispatch::TAG_NODE => DeclKind::Node,
            dispatch::TAG_EDGE => DeclKind::Edge,
            _ => DeclKind::HyperArc, // tag 3 already rejected by decode_checked
        };
        if tag_kind != kind {
            c.push(Finding::KindMapMismatch { decl: did, kind });
            continue;
        }
        match kind {
            DeclKind::Node => {
                if ir.nodes.get(idx).map(|r| r.decl) != Some(did) {
                    let record_decl = ir.nodes.get(idx).map_or(DeclId::NONE, |r| r.decl);
                    c.push(Finding::RecordBacklinkMismatch {
                        decl: did,
                        record_decl,
                    });
                }
            }
            DeclKind::Edge => {
                if ir.edges.get(idx).map(|r| r.decl) != Some(did) {
                    let record_decl = ir.edges.get(idx).map_or(DeclId::NONE, |r| r.decl);
                    c.push(Finding::RecordBacklinkMismatch {
                        decl: did,
                        record_decl,
                    });
                }
            }
            DeclKind::HyperArc => {
                // ArcRec carries no `decl` back-pointer; validate the index is in range.
                if idx >= ir.arcs.len() {
                    c.push(Finding::RecordBacklinkMismatch {
                        decl: did,
                        record_decl: DeclId::NONE,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod parent_chain_tests {
    use super::*;
    use crate::ir::ir::DeclNode;

    #[test]
    fn short_dispatch_table_reports_length_instead_of_panicking() {
        let mut ir = Ir::default();
        ir.push_decl(DeclNode::root(
            DeclKind::Node,
            SymId::new(0),
            Default::default(),
        ));
        ir.decl_to_record.clear();
        let report = validate_hierarchy_index_coherence(
            &ir,
            &Index::default(),
            &Interner::new(),
            CoherenceLimits::default(),
        )
        .unwrap_err();
        assert!(report.findings.iter().any(|finding| matches!(
            finding,
            Finding::ParallelArrayLengthMismatch {
                array: "decl_to_record",
                len: 0,
                expected: 1
            }
        )));
    }

    fn classify(parents: &[DeclId], max_walk: usize, max_findings: usize) -> Collector {
        let ir = Ir {
            decl_nodes: parents
                .iter()
                .map(|&parent| DeclNode {
                    kind: DeclKind::Node,
                    name: SymId::new(0),
                    parent,
                    first_child: DeclId::NONE,
                    last_child: DeclId::NONE,
                    next_sibling: DeclId::NONE,
                    anno: crate::common::ids::AnnotationId::NONE,
                })
                .collect(),
            ..Ir::default()
        };
        let mut collector = Collector {
            limits: CoherenceLimits {
                max_walk,
                max_findings,
            },
            findings: Vec::new(),
            total: 0,
        };
        validate_parent_chains(&ir, &mut collector);
        collector
    }

    #[test]
    fn memoized_parent_chains_preserve_bounded_walk_diagnostics_exhaustively() {
        for n in 0..=4_u32 {
            let choices = n as usize + 2;
            for mut encoding in 0..choices.pow(n) {
                let mut parents = Vec::new();
                for _ in 0..n {
                    let raw = encoding % choices;
                    encoding /= choices;
                    parents.push(if raw == n as usize + 1 {
                        DeclId::NONE
                    } else {
                        DeclId::new(raw)
                    });
                }
                for max_walk in [0, 1, n as usize, 2 * n as usize] {
                    // Independent oracle: literally follow each parent chain,
                    // without memoization, at deliberately small finite limits.
                    let mut expected = Vec::new();
                    for (declaration, &parent) in parents.iter().enumerate() {
                        let mut current = parent;
                        let mut walked = 0;
                        while current.is_some() {
                            if current.raw() >= parents.len() || walked > max_walk {
                                expected.push(Finding::ParentCycle {
                                    decl: DeclId::new(declaration),
                                    walked,
                                });
                                break;
                            }
                            current = parents[current.raw()];
                            walked += 1;
                        }
                    }
                    for cap in [0, 2, 64] {
                        let actual = classify(&parents, max_walk, cap);
                        assert_eq!(actual.total, expected.len());
                        assert_eq!(actual.findings, expected[..expected.len().min(cap)]);
                    }
                }
            }
        }
    }

    #[test]
    fn parent_chains_handle_max_limit_and_shared_long_tails() {
        let parents: Vec<_> = (0..257)
            .map(|i| {
                if i == 0 {
                    DeclId::NONE
                } else {
                    DeclId::new(i - 1)
                }
            })
            .collect();
        assert_eq!(classify(&parents, usize::MAX, 64).total, 0);
        // MAX cannot express MAX+1; cyclic diagnostics saturate and terminate.
        let cycle = classify(
            &[DeclId::new(1), DeclId::new(0), DeclId::new(1)],
            usize::MAX,
            2,
        );
        assert_eq!(cycle.total, 3);
        assert_eq!(
            cycle.findings,
            [
                Finding::ParentCycle {
                    decl: DeclId::new(0),
                    walked: usize::MAX
                },
                Finding::ParentCycle {
                    decl: DeclId::new(1),
                    walked: usize::MAX
                },
            ]
        );
    }
}

// =================================================================================================
// Executable-model contract validation
// APPROVED-CORE-EDIT: executable-model-contract (2026-09-06).
// =================================================================================================

/// A violation of the executable-model contract.
///
/// Ids and bounded numbers only; never caller-supplied text, so a hostile `Ir` cannot make the
/// report echo a large payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFinding {
    /// A tensor facet sits on a declaration that is not a `Node`.
    TensorFacetOnNonNode { decl: DeclId, kind: DeclKind },
    /// An operator facet sits on a declaration that is not an `Edge`.
    OperatorFacetOnNonEdge { decl: DeclId, kind: DeclKind },
    /// The facet table disagrees in length with `decl_nodes`.
    FacetTableLengthMismatch { len: usize, expected: usize },
    /// The model declares no input tensor.
    NoInput,
    /// The model declares no output tensor.
    NoOutput,
    /// An operator's identity symbol resolves to the empty string.
    EmptyOperatorIdentity { decl: DeclId },
    /// A model input or a parameter is declared as an operator's *output* port.
    ///
    /// The control that detects a swapped port wiring: a model input is supplied, and a parameter
    /// is learned, so neither is produced by an operator.
    ProducedInputOrParameter {
        arc: DeclId,
        slot: usize,
        target: DeclId,
        role: TensorRole,
    },
    /// An operator declares port directions but not both an input and an output.
    OperatorPortsIncomplete {
        arc: DeclId,
        has_in: bool,
        has_out: bool,
    },
}

impl core::fmt::Display for ModelFinding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TensorFacetOnNonNode { decl, kind } => write!(
                f,
                "declaration {} carries a tensor facet but is a {kind:?}",
                decl.raw()
            ),
            Self::OperatorFacetOnNonEdge { decl, kind } => write!(
                f,
                "declaration {} carries an operator facet but is a {kind:?}",
                decl.raw()
            ),
            Self::FacetTableLengthMismatch { len, expected } => {
                write!(f, "facet table has {len} slots, expected {expected}")
            }
            Self::NoInput => f.write_str("the model declares no input tensor"),
            Self::NoOutput => f.write_str("the model declares no output tensor"),
            Self::EmptyOperatorIdentity { decl } => write!(
                f,
                "operator declaration {} has an empty identity",
                decl.raw()
            ),
            Self::ProducedInputOrParameter {
                arc,
                slot,
                target,
                role,
            } => write!(
                f,
                "arc {} slot {slot} declares declaration {} ({role:?}) as an output port",
                arc.raw(),
                target.raw()
            ),
            Self::OperatorPortsIncomplete {
                arc,
                has_in,
                has_out,
            } => write!(
                f,
                "arc {} declares directions but has_in={has_in} has_out={has_out}",
                arc.raw()
            ),
        }
    }
}

impl std::error::Error for ModelFinding {}

/// Validate the executable-model contract of an `Ir`.
///
/// # The switch
///
/// A document is an *executable model* exactly when at least one declaration carries a
/// [`ModelFacet`]. **A document with no facet is not a model and is returned as `Ok` without any
/// check** — the tracked corpus of service networks, robotics meta-graphs and P-graphs is exactly
/// that case, and imposing model rules on it would be a category error.
///
/// # What is checked, once the document is a model
///
/// Facet placement against declaration kind, facet-table length, a non-empty operator identity, and
/// the two document-level requirements that make a model executable at all: at least one input and
/// at least one output.
///
/// # What is deliberately NOT checked here
///
/// Shape *inference* and operator-vocabulary meaning. Core owns that a declaration is an operator
/// and which symbol is its identity; what `linear` computes is the consuming framework's contract.
///
/// # Errors
/// Every violation found, in declaration order, then the document-level findings.
pub fn validate_model_contract(ir: &Ir, it: &Interner) -> Result<(), Vec<ModelFinding>> {
    if !ir.declares_executable_model() {
        return Ok(());
    }
    let mut findings = Vec::new();
    let n = ir.decl_nodes.len();
    let table = ir.model_facet_table_len();
    if table != n {
        findings.push(ModelFinding::FacetTableLengthMismatch {
            len: table,
            expected: n,
        });
    }

    let (mut has_input, mut has_output) = (false, false);
    for i in 0..n {
        let d = DeclId::new(i);
        let Some(facet) = ir.model_facet(d) else {
            continue;
        };
        let kind = ir.decl_nodes[i].kind;
        match facet {
            ModelFacet::Tensor(t) => {
                if kind != DeclKind::Node {
                    findings.push(ModelFinding::TensorFacetOnNonNode { decl: d, kind });
                }
                match t.role {
                    TensorRole::Input => has_input = true,
                    TensorRole::Output => has_output = true,
                    _ => {}
                }
            }
            ModelFacet::Operator(o) => {
                if kind != DeclKind::Edge {
                    findings.push(ModelFinding::OperatorFacetOnNonEdge { decl: d, kind });
                }
                if it.resolve(o.identity).is_empty() {
                    findings.push(ModelFinding::EmptyOperatorIdentity { decl: d });
                }
            }
        }
    }
    // Port-direction rules. They engage only where a direction was actually declared, so a legacy
    // incidence -- every one of which is `Unspecified` -- is never judged against them.
    for (ai, arc) in ir.arcs.iter().enumerate() {
        let arc_decl = arc.in_edge;
        let (mut saw_in, mut saw_out, mut declared) = (false, false, false);
        for (slot, reference) in arc.refs.iter().enumerate() {
            match reference.direction() {
                PortDirection::Unspecified => continue,
                PortDirection::In => {
                    declared = true;
                    saw_in = true;
                }
                PortDirection::Out => {
                    declared = true;
                    saw_out = true;
                    if let Some(ModelFacet::Tensor(t)) = ir.model_facet(reference.target()) {
                        if matches!(t.role, TensorRole::Input | TensorRole::Parameter) {
                            findings.push(ModelFinding::ProducedInputOrParameter {
                                arc: DeclId::new(ai),
                                slot,
                                target: reference.target(),
                                role: t.role,
                            });
                        }
                    }
                }
            }
        }
        if declared && !(saw_in && saw_out) {
            findings.push(ModelFinding::OperatorPortsIncomplete {
                arc: arc_decl,
                has_in: saw_in,
                has_out: saw_out,
            });
        }
    }

    if !has_input {
        findings.push(ModelFinding::NoInput);
    }
    if !has_output {
        findings.push(ModelFinding::NoOutput);
    }

    if findings.is_empty() {
        Ok(())
    } else {
        Err(findings)
    }
}
