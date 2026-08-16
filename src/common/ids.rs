use std::marker::PhantomData;

/// Marker for declaration-level IDs (indexes `Ir::decl_nodes`).
pub struct DeclTag;
/// Marker for node record IDs (indexes `Ir::nodes`).
pub struct NodeTag;
/// Marker for edge record IDs (indexes `Ir::edges`).
pub struct EdgeTag;
/// Marker for hyper-arc record IDs (indexes `Ir::arcs`).
pub struct HyperArcTag;
/// Marker for interned symbol IDs (indexes the `Interner` string table).
pub struct SymTag;

/// Index into `Ir::decl_nodes` — the unified declaration table.
pub type DeclId = Id<DeclTag>;

/// Index into `Ir::nodes` (the `NodeRec` table).
pub type NodeId = Id<NodeTag>;

/// Index into `Ir::edges` (the `EdgeRec` table).
pub type EdgeId = Id<EdgeTag>;

/// Index into `Ir::arcs` (the `ArcRec` table).
pub type HyperArcId = Id<HyperArcTag>;

/// Index into the `Interner` / `StringTable` symbol table.
pub type SymId = Id<SymTag>;

#[repr(C)] // Stable layout for serde and FFI
pub struct Id<T>(pub usize, PhantomData<T>);

impl<T> Id<T> {
    /// Sentinel value representing "no ID" / null.
    pub const NONE: Self = Self(usize::MAX, PhantomData);

    /// Create a new ID from a raw index.
    #[inline(always)]
    pub const fn new(raw: usize) -> Self {
        Self(raw, PhantomData)
    }

    /// The raw index value.
    #[inline(always)]
    pub const fn raw(self) -> usize {
        self.0
    }

    /// Returns `true` if this ID is the sentinel NONE value.
    #[inline(always)]
    pub const fn is_none(self) -> bool {
        self.0 == usize::MAX
    }

    /// Returns `true` if this ID is NOT the sentinel NONE value.
    #[inline(always)]
    pub const fn is_some(self) -> bool {
        self.0 != usize::MAX
    }
}
