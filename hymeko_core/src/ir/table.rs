//! Contiguous canonical IR records shared by immutable snapshots.
//!
//! Published tables share storage. Mutable construction uses an owned Vec, so
//! repeated writes need no reference-count operations. A table still serializes
//! as an ordinary sequence, and every older snapshot remains valid.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// A contiguous record table with snapshot-isolated mutation.
///
/// Read access and mutable Vec operations use the existing table interface.
/// `clone()` shares published storage and deep-copies an owned mutable table.
/// Core publishes tables when it returns a compiled snapshot. `to_vec()` always
/// makes an owned deep copy. The first mutation detaches the whole shared table;
/// later writes use its owned Vec. Consuming iteration moves unique records or
/// clones records still held by another reader.
pub struct IrTable<T> {
    // Exactly one storage owns records: when shared is Some, owned is empty.
    owned: Vec<T>,
    shared: Option<Arc<Vec<T>>>,
}

impl<T: Clone> Clone for IrTable<T> {
    fn clone(&self) -> Self {
        match &self.shared {
            Some(shared) => Self {
                owned: Vec::new(),
                shared: Some(Arc::clone(shared)),
            },
            None => Self {
                owned: self.owned.clone(),
                shared: None,
            },
        }
    }
}

impl<T> Default for IrTable<T> {
    fn default() -> Self {
        Self {
            owned: Vec::new(),
            shared: None,
        }
    }
}

impl<T> From<Vec<T>> for IrTable<T> {
    fn from(records: Vec<T>) -> Self {
        Self {
            owned: Vec::new(),
            shared: Some(Arc::new(records)),
        }
    }
}

impl<T> FromIterator<T> for IrTable<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl<T> Deref for IrTable<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Self::Target {
        self.shared.as_deref().unwrap_or(&self.owned)
    }
}

impl<T: Clone> DerefMut for IrTable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if let Some(shared) = self.shared.take() {
            self.owned = Arc::unwrap_or_clone(shared);
        }
        &mut self.owned
    }
}

impl<'a, T> IntoIterator for &'a IrTable<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut IrTable<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: Clone> IntoIterator for IrTable<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        match self.shared {
            Some(shared) => Arc::unwrap_or_clone(shared).into_iter(),
            None => self.owned.into_iter(),
        }
    }
}

impl<T: Serialize> Serialize for IrTable<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.deref().serialize(serializer)
    }
}

impl<T> IrTable<T> {
    pub(crate) fn share(&mut self) {
        if self.shared.is_none() {
            self.shared = Some(Arc::new(std::mem::take(&mut self.owned)));
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for IrTable<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T: PartialEq> PartialEq for IrTable<T> {
    fn eq(&self, other: &Self) -> bool {
        self.deref() == other.deref()
    }
}

impl<T: Eq> Eq for IrTable<T> {}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for IrTable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_and_consumption_preserve_retained_readers() {
        let mut built = IrTable::default();
        built.push(String::from("built"));
        let owned_copy = built.clone();
        built[0].push('!');
        assert_eq!(owned_copy[0], "built");
        let records = built.as_ptr();
        built.share();
        assert_eq!(built.as_ptr(), records, "publishing must move records");
        assert_eq!(built.clone().as_ptr(), records);
        let base: IrTable<_> = vec![String::from("first"), String::from("second")].into();
        let mut edited = base.clone();
        assert_eq!(base.as_ptr(), edited.as_ptr());
        edited[0].push_str(" edited");
        edited.push(String::from("third"));
        for value in &mut edited {
            value.push('!');
        }
        assert_eq!(base.as_slice(), ["first", "second"]);
        assert_eq!(edited.as_slice(), ["first edited!", "second!", "third!"]);
        let consumed: Vec<_> = base.clone().into_iter().collect();
        assert_eq!(consumed, ["first", "second"]);
        assert_eq!(base[0], "first");
        let unique: IrTable<_> = vec![String::from("unique")].into();
        let allocation = unique[0].as_ptr();
        let value = unique.into_iter().next().unwrap();
        assert_eq!(value.as_ptr(), allocation, "unique records must move");
    }

    #[test]
    fn shared_table_has_the_same_sequence_encoding_as_vec() {
        let values = vec![String::from("a"), String::from("b")];
        let table: IrTable<_> = values.clone().into();
        let mut expected = Vec::new();
        let mut actual = Vec::new();
        ciborium::into_writer(&values, &mut expected).unwrap();
        ciborium::into_writer(&table, &mut actual).unwrap();
        assert_eq!(actual, expected);
        let restored: IrTable<String> = ciborium::from_reader(actual.as_slice()).unwrap();
        assert_eq!(restored.as_slice(), values);
    }
}
