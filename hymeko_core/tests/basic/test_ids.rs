#[cfg(test)]
mod tests {
    use super::*;
    use hymeko::common::ids::{DeclId, EdgeId, NodeId, SymId};

    #[test]
    fn test_sealing_is_size_neutral() {
        use std::mem::size_of;
        // `#[repr(transparent)]` over a single `usize` field: every domain id is
        // exactly a `usize` in memory, unchanged by sealing. This is the Phase-F
        // size invariant — a generational representation (a later phase) is what
        // may change these, not this sealing-only phase.
        assert_eq!(size_of::<DeclId>(), size_of::<usize>(), "DeclId is a usize");
        assert_eq!(size_of::<NodeId>(), size_of::<usize>(), "NodeId is a usize");
        assert_eq!(size_of::<EdgeId>(), size_of::<usize>(), "EdgeId is a usize");
        assert_eq!(size_of::<SymId>(), size_of::<usize>(), "SymId is a usize");
        // No niche today (usize::MAX is the live NONE sentinel), so Option doubles.
        // Recorded, not asserted as a target: reducing this is the generational
        // phase's job, not this one.
        assert_eq!(size_of::<Option<DeclId>>(), 2 * size_of::<usize>());
    }

    #[test]
    fn test_none_sentinel() {
        let d = DeclId::NONE;
        assert!(d.is_none());
        assert!(!d.is_some());
        assert_eq!(d.raw(), usize::MAX);
    }

    #[test]
    fn test_new_and_raw() {
        let d = DeclId::new(42);
        assert_eq!(d.raw(), 42);
        assert_eq!(d.raw(), 42);
        assert!(d.is_some());
        assert!(!d.is_none());
    }

    #[test]
    fn test_equality_across_same_tag() {
        assert_eq!(DeclId::new(1), DeclId::new(1));
        assert_ne!(DeclId::new(1), DeclId::new(2));
    }

    #[test]
    fn test_ordering() {
        assert!(DeclId::new(1) < DeclId::new(2));
        assert!(SymId::new(10) > SymId::new(5));
    }

    #[test]
    fn test_vec_indexing() {
        let data = vec!["zero", "one", "two", "three"];
        let id = NodeId::new(2);
        assert_eq!(data[id.raw()], "two");
    }

    #[test]
    fn test_vec_index_mut() {
        let mut data = vec![0, 0, 0];
        let id = EdgeId::new(1);
        data[id.raw()] = 42;
        assert_eq!(data[id.raw()], 42);
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(DeclId::new(5));
        assert!(set.contains(&DeclId::new(5)));
        assert!(!set.contains(&DeclId::new(6)));
    }

    #[test]
    fn test_explicit_construction_seam() {
        // M2c sealing: implicit `usize <-> Id` conversion was removed. The only
        // construction/extraction is the explicit `new` / `raw` seam. (The
        // compile-fail of `7.into()` / `id.into()` / `id.0` is asserted by the
        // `compile_fail` doctests on `Id` in `common/ids.rs`.)
        let id = DeclId::new(7);
        assert_eq!(id.raw(), 7);
        // Explicit raw reconstruction across a wire/FFI boundary stays available
        // and is domain-typed.
        let same: DeclId = DeclId::new(id.raw());
        assert_eq!(id, same);
    }

    #[test]
    fn test_serde_roundtrip() {
        let original = DeclId::new(123);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "123");
        let recovered: DeclId = serde_json::from_str(&json).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_type_safety_demo() {
        // This test demonstrates what the refactor PREVENTS.
        // With the old code, all IDs were interchangeable:
        //   let nid = NodeId(3);
        //   ir.edges[nid.0]  // compiles but semantically wrong
        //
        // With the new code, `ir.edges[nid]` would fail IF we
        // constrain the Index impl per-domain (see migration notes).
        //
        // For now, Id<T> indexes ANY Vec<V> — the tag only prevents
        // assignment confusion (NodeId into DeclId variable).
        let _d: DeclId = DeclId::new(1);
        let _n: NodeId = NodeId::new(1);
        // let _wrong: DeclId = _n;  // ← COMPILE ERROR (type mismatch)
    }
}
