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

/// A phantom-typed index into one of the canonical `Ir` tables.
///
/// # Sealing (M2c, `APPROVED-CORE-EDIT: seal-canonical-entity-ids`)
///
/// The tuple field is `pub(crate)`, not `pub`: the raw representation is owned by
/// `hymeko_core` and is not part of the public contract. External crates cannot
/// read or construct it by field, and there is **no** implicit `usize <-> Id`
/// conversion (`From<usize>` / `From<Id>` were removed). This makes every
/// representation-dependent construction explicit and greppable *before* the
/// physical layout changes (generational arenas, a later phase).
///
/// The two explicit, documented seam methods are [`Id::new`] and [`Id::raw`].
/// They are the single place a future generational representation will change,
/// and the only sanctioned raw/wire reconstruction path. Cross-domain
/// construction is still a compile error — the phantom tag makes `DeclId`,
/// `NodeId`, … distinct types. Serde retains bare integer indices; NONE uses
/// `u64::MAX` on every target so native and wasm32 payloads agree.
///
/// # Sealed forms no longer compile (with their explicit twins)
///
/// The raw tuple field is not public:
/// ```compile_fail
/// use hymeko::common::ids::DeclId;
/// let d = DeclId::new(7);
/// let _raw = d.0; // error[E0616]: field `0` of struct `Id` is private
/// ```
/// There is no implicit `usize -> Id`:
/// ```compile_fail
/// use hymeko::common::ids::DeclId;
/// let _d: DeclId = 7usize.into(); // error[E0277]: From<usize> not implemented
/// ```
/// …nor `Id -> usize`:
/// ```compile_fail
/// use hymeko::common::ids::DeclId;
/// let d = DeclId::new(7);
/// let _r: usize = d.into(); // error[E0277]: From<Id> not implemented
/// ```
/// Cross-domain construction through the (removed) `From` is rejected — an
/// `EdgeId` cannot be minted from a `NodeId`'s raw index implicitly:
/// ```compile_fail
/// use hymeko::common::ids::{EdgeId, NodeId};
/// let n = NodeId::new(3);
/// let _e = EdgeId::from(n.raw()); // error[E0277]: From<usize> not implemented
/// ```
/// (The explicit, greppable `EdgeId::new(n.raw())` *does* compile — that is the
/// sanctioned intentional raw/wire reconstruction, and it is deliberately loud:
/// you must name both `raw()` and the target domain. A raw index is not a
/// validated handle; validation happens on lookup against the owning `Ir`.)
/// The explicit seam is the compiling twin — construction, extraction, and
/// domain-typed raw reconstruction all work:
/// ```
/// use hymeko::common::ids::{DeclId, EdgeId};
/// let d = DeclId::new(7);
/// assert_eq!(d.raw(), 7);
/// let _e = EdgeId::new(d.raw()); // explicit, greppable raw reconstruction
/// ```
// `#[repr(transparent)]`, not `#[repr(C)]`: an `Id<T>` is exactly a `usize` in
// memory (the `PhantomData<T>` is zero-sized), so it has the same in-process ABI
// as `usize`. The former `repr(C)` comment claimed a "stable layout for serde and
// FFI" — that was misleading: serde serializes the *value* (a bare integer, via
// the impls in `ids_impl.rs`), not the struct layout, and `usize` is **not** a
// stable cross-target wire format (its width is 32/64-bit host-dependent). Serde
// writes a u64 index, with u64::MAX reserved for NONE on every target. Decode
// translates that sentinel to local NONE and rejects ordinary indices which do
// not fit the target or collide with its sentinel. The owning Ir still validates
// whether a representable index actually identifies a declaration.
#[repr(transparent)]
pub struct Id<T>(pub(crate) usize, PhantomData<T>);

impl<T> Id<T> {
    /// Sentinel value representing "no ID" / null.
    pub const NONE: Self = Self(usize::MAX, PhantomData);

    /// Create an ID from a raw index — the **explicit, representation-dependent**
    /// construction seam.
    ///
    /// This is the sanctioned way to reconstruct an ID from a raw/wire index
    /// (e.g. after deserialization or at an FFI boundary). It is deliberately
    /// domain-typed: `DeclId::new(i)` cannot silently become a `NodeId`. When the
    /// physical representation changes, this signature is one of the two seam
    /// points (with [`Id::raw`]) that migrate.
    #[inline(always)]
    pub const fn new(raw: usize) -> Self {
        Self(raw, PhantomData)
    }

    /// The raw index value — the **explicit, representation-dependent** extraction
    /// seam.
    ///
    /// Prefer the accessor API for lookups; use `raw()` only at a genuine
    /// serialization / wire / FFI boundary. It replaces the former public `.0`
    /// field access; the two together ([`Id::new`], `raw`) are the only place a
    /// future generational representation will change.
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
