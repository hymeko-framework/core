use crate::common::ids::Id;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

impl<T> Copy for Id<T> {}

impl<T> Clone for Id<T> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> PartialEq for Id<T> {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T> Eq for Id<T> {}

impl<T> Hash for Id<T> {
    #[inline(always)]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T> PartialOrd for Id<T> {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Id<T> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Prints as "Id(42)" — same as the old tuple struct debug output.
        // For nicer output, specializations below override this.
        write!(f, "Id({})", self.0)
    }
}

impl<T> Serialize for Id<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        usize::deserialize(deserializer).map(Id::new)
    }
}

// --- No implicit `usize <-> Id` conversion (M2c sealing) ---
//
// `From<usize> for Id<T>` and `From<Id<T>> for usize` were removed under
// `APPROVED-CORE-EDIT: seal-canonical-entity-ids`. Implicit conversion hid
// representation-dependent construction and allowed an integer to slide into any
// ID domain via `.into()`. The explicit seam is `Id::new` / `Id::raw` (see
// `ids.rs`); serde still (de)serializes the bare index, so the wire schema is
// unchanged.
