# Part A — Index consumer & contract audit

Branch `feat/core-hierarchy-authority` @ `67771c9`, 2026-08-31. Read-only map of
the `resolution::Index` lifecycle and every post-lowering dependency, to separate
what must stay from what U2 would remove.

## Index lifecycle

- **Created** in resolution: `intern_pass` builds the symbol AST; `resolve.rs`
  (`build_index` / lowering-time inserts at `resolve.rs:202`,
  `module_store.rs:224-231`) populates `Index.by_path : HashMap<PathKey, DeclId>`
  — a full path → declaration map. `PathKey` is the sequence of name `SymId`s,
  **namespace-prefixed for imports** (`lower_program_to_ir` lowers each import
  under `&[ns]`).
- **Consumed at resolution/lowering time** (legitimately): `resolve_signed_refs`,
  `resolve_anno`, `resolve_node_bases`, `const_resolve`, and `lower.rs`
  (`parent_decl`, `decl_id_of`) — all resolve names to `DeclId`s while building
  the IR. `lower.rs` also **builds the intrusive links from the Index**
  (`parent = idx.by_path[scope]`, then `link_decl_child`).
- **Consumed post-lowering** (the U2 target):
  - `canonical_hash::canonical_program_hash` — `build_did_to_path(idx,…)` and
    `build_skeleton_forest(idx,…)`; everything downstream uses only `did_to_path`
    + `ir`.
  - `module_store::CompiledProgram` stores `idx` **alongside** `ir` and hands it
    to the hash (`module_store.rs:263`) — the second post-lowering authority.
  - `coherence.rs` validates Index ↔ Ir agreement (transitional).
  - `IrWriter` / `pretty_print` already walk the **links**, not the Index.

## Four categories (per the brief)

1. **resolution-time Index — keep.** `resolve.rs`, `const_resolve`, `intern_pass`,
   and `lower.rs`'s use of `by_path` to resolve refs and build the links.
2. **lowering-boundary coherence — transitionally needed.** `coherence.rs`
   (Index-path hierarchy ≡ Ir links).
3. **post-lowering derived Index/path — the removal target.** `build_did_to_path`
   + `build_skeleton_forest` in the hash, and `CompiledProgram.idx` kept only for
   the hash.
4. **real canonical hierarchy state — Ir links.** `DeclNode.parent/first_child/
   last_child/next_sibling/name/kind`.

## The load-bearing finding

Category 3 is **not fully derivable from category 4.** The Ir links encode the
*tree*, but **not the import namespace**: for an imported module, lowering leaves
the imported root with `parent == NONE`, and the namespace (`meta_kinematics`)
lives only in the `Index` `PathKey`. The `Ir` struct has no field for it
(fields: meta, doc_hash, decl_hash, arc_hash, decl_nodes, nodes, edges, arcs,
decl_to_record). The v1 canonical hash *embeds* that prefix (via `did_to_path`),
so a pure-links hash diverges on every multi-module program. Measured differential
(link-derived vs index-derived over the corpus): **35 agree, 61 diverge — all 61
import-bearing.** This blocks a bit-identical Part D flip; see the decision report.

## Contracts that must not move (from the frozen oracle, Part B)

`algo_version = 1`; every hash byte; arc/ref-slot order; NaN/Inf/−0.0 encoding;
the v1 `node.bases` coverage gap (unchanged, not silently fixed); the known merkle
`incidence.target` defect (not touched); writer output byte-identical; the
malformed-hierarchy typed errors.
