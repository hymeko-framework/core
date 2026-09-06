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
        // NONE has one wire value on both native 64-bit and wasm32 targets.
        let wire = if self.is_none() {
            u64::MAX
        } else {
            self.raw() as u64
        };
        wire.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = u64::deserialize(deserializer)?;
        if wire == u64::MAX {
            return Ok(Id::NONE);
        }
        let raw = usize::try_from(wire).map_err(serde::de::Error::custom)?;
        if raw == usize::MAX {
            return Err(serde::de::Error::custom(
                "ID collides with the target NONE sentinel",
            ));
        }
        Ok(Id::new(raw))
    }
}

// --- No implicit `usize <-> Id` conversion (M2c sealing) ---
//
// `From<usize> for Id<T>` and `From<Id<T>> for usize` were removed under
// `APPROVED-CORE-EDIT: seal-canonical-entity-ids`. Implicit conversion hid
// representation-dependent construction and allowed an integer to slide into any
// ID domain via `.into()`. The explicit seam is `Id::new` / `Id::raw` (see
// `ids.rs`); ordinary serde indices are unchanged. NONE has one u64 wire value
// independent of the target's pointer width.

#[cfg(test)]
mod portable_wire_tests {
    use crate::common::ids::DeclId;

    #[test]
    fn none_uses_the_native_64_bit_wire_sentinel_on_every_target() {
        let bytes = [0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let decoded: DeclId = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert!(decoded.is_none());
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&DeclId::NONE, &mut encoded).unwrap();
        assert_eq!(encoded, bytes);
    }

    #[test]
    fn ordinary_id_bytes_remain_unchanged() {
        let decoded: DeclId = ciborium::de::from_reader([0x18, 0x2a].as_slice()).unwrap();
        assert_eq!(decoded.raw(), 42);
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&decoded, &mut encoded).unwrap();
        assert_eq!(encoded, [0x18, 0x2a]);
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    fn oversized_ids_and_reserved_native_index_are_rejected() {
        for wire in [u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX - 1] {
            let mut encoded = Vec::new();
            ciborium::ser::into_writer(&wire, &mut encoded).unwrap();
            assert!(ciborium::de::from_reader::<DeclId, _>(encoded.as_slice()).is_err());
        }
    }
}
