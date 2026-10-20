//! Canonical IR records with contiguous default and opt-in paged snapshots.
//! Indexed paged writes detach only their page. Explicit contiguous Vec access
//! materializes a cached view; mutable Vec access returns to owned flat storage.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    ops::{Deref, DerefMut, Index, IndexMut},
    sync::{Arc, OnceLock},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrPageSizeError(pub usize);

impl std::fmt::Display for IrPageSizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "IR page size must be a nonzero power of two, got {}",
            self.0
        )
    }
}
impl std::error::Error for IrPageSizeError {}

struct PagedRecords<T> {
    pages: Vec<Arc<Vec<T>>>,
    len: usize,
    shift: u32,
    flat: OnceLock<Vec<T>>,
    // Installed only by share_pages for T: Clone. This cold callback preserves
    // generic read/Deref support without imposing Clone on ordinary flat tables.
    flatten: fn(&[Arc<Vec<T>>], usize) -> Vec<T>,
}

impl<T> Clone for PagedRecords<T> {
    fn clone(&self) -> Self {
        Self {
            pages: self.pages.clone(),
            len: self.len,
            shift: self.shift,
            flat: OnceLock::new(),
            flatten: self.flatten,
        }
    }
}
impl<T> PagedRecords<T> {
    fn record(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        self.pages[index >> self.shift].get(index & ((1usize << self.shift) - 1))
    }
    fn contiguous(&self) -> &Vec<T> {
        self.flat
            .get_or_init(|| (self.flatten)(&self.pages, self.len))
    }
}

enum Storage<T> {
    Owned(Vec<T>),
    Shared(Arc<Vec<T>>),
    Paged(Arc<PagedRecords<T>>),
}

/// Snapshot-isolated canonical records. Default storage is contiguous.
///
/// Cloning published storage shares it; mutable construction clones deeply.
/// Indexed writes to paged declarations detach the directory and affected pages.
/// `iter()` and indexed access stream without flattening. Explicit Vec/slice
/// access may materialize O(N) cloned records; mutable Vec operations materialize
/// owned contiguous storage. Serialization is the same ordinary sequence.
pub struct IrTable<T> {
    data: Storage<T>,
    // Construction intent only; never changes logical length or wire content.
    capacity_hint: usize,
}

impl<T: Clone> Clone for IrTable<T> {
    fn clone(&self) -> Self {
        Self {
            capacity_hint: self.capacity_hint,
            data: match &self.data {
                Storage::Owned(records) => Storage::Owned(records.clone()),
                Storage::Shared(records) => Storage::Shared(Arc::clone(records)),
                Storage::Paged(records) => Storage::Paged(Arc::clone(records)),
            },
        }
    }
}
impl<T> Default for IrTable<T> {
    fn default() -> Self {
        Self {
            data: Storage::Owned(Vec::new()),
            capacity_hint: 0,
        }
    }
}
impl<T> From<Vec<T>> for IrTable<T> {
    fn from(records: Vec<T>) -> Self {
        Self {
            data: Storage::Shared(Arc::new(records)),
            capacity_hint: 0,
        }
    }
}
impl<T> FromIterator<T> for IrTable<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl<T> IrTable<T> {
    /// Record an expected final capacity without allocating or adding records.
    /// The first mutable Vec access reserves at least this many slots. Merely
    /// reading, iterating, serializing or cloning an empty table does not allocate
    /// its record storage. As with Vec, allocation/overflow can fail at mutation.
    pub fn defer_capacity(&mut self, capacity: usize) {
        self.capacity_hint = self.capacity_hint.max(capacity);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match &self.data {
            Storage::Owned(records) => records.len(),
            Storage::Shared(records) => records.len(),
            Storage::Paged(records) => records.len,
        }
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get_record(&self, index: usize) -> Option<&T> {
        match &self.data {
            Storage::Owned(records) => records.get(index),
            Storage::Shared(records) => records.get(index),
            Storage::Paged(records) => records.record(index),
        }
    }

    pub fn iter(&self) -> IrTableIter<'_, T> {
        let inner = match &self.data {
            Storage::Owned(records) => TableIteration::Flat(records.iter()),
            Storage::Shared(records) => TableIteration::Flat(records.iter()),
            Storage::Paged(records) => TableIteration::Paged {
                records,
                front: 0,
                back: records.len,
            },
        };
        IrTableIter { inner }
    }

    pub(crate) fn share(&mut self) {
        if let Storage::Owned(records) = &mut self.data {
            // Optional tables stay allocation-free until they contain data.
            if !records.is_empty() {
                self.data = Storage::Shared(Arc::new(std::mem::take(records)));
            }
        }
    }
}

impl<T: Clone> IrTable<T> {
    /// Release unused capacity and cancel an unmaterialized construction hint.
    pub fn shrink_to_fit(&mut self) {
        self.capacity_hint = 0;
        self.deref_mut().shrink_to_fit();
    }

    pub(crate) fn share_pages(&mut self, page_size: usize) -> Result<(), IrPageSizeError> {
        if !page_size.is_power_of_two() {
            return Err(IrPageSizeError(page_size));
        }
        let shift = page_size.trailing_zeros();
        if matches!(&self.data, Storage::Paged(records) if records.shift == shift) {
            return Ok(());
        }
        let data = std::mem::replace(&mut self.data, Storage::Owned(Vec::new()));
        let mut records = Self::into_vec(data).into_iter();
        let len = records.len();
        let mut pages = Vec::with_capacity(len.div_ceil(page_size));
        while records.len() != 0 {
            pages.push(Arc::new(records.by_ref().take(page_size).collect()));
        }
        self.data = Storage::Paged(Arc::new(PagedRecords {
            pages,
            len,
            shift,
            flat: OnceLock::new(),
            flatten: Self::flatten_pages,
        }));
        Ok(())
    }

    fn flatten_pages(pages: &[Arc<Vec<T>>], len: usize) -> Vec<T> {
        let mut records = Vec::with_capacity(len);
        for page in pages {
            records.extend(page.iter().cloned());
        }
        records
    }

    fn into_vec(data: Storage<T>) -> Vec<T> {
        match data {
            Storage::Owned(records) => records,
            Storage::Shared(records) => Arc::unwrap_or_clone(records),
            Storage::Paged(records) => {
                let records = Arc::unwrap_or_clone(records);
                if let Some(flat) = records.flat.into_inner() {
                    return flat;
                }
                let mut flat = Vec::with_capacity(records.len);
                for page in records.pages {
                    flat.extend(Arc::unwrap_or_clone(page));
                }
                flat
            }
        }
    }

    /// An explicit owned copy, without retaining a contiguous cache on the source.
    #[must_use]
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    /// Mutable whole-table iteration materializes owned contiguous records.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.deref_mut().iter_mut()
    }
}

impl<T> Deref for IrTable<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Self::Target {
        match &self.data {
            Storage::Owned(records) => records,
            Storage::Shared(records) => records,
            Storage::Paged(records) => records.contiguous(),
        }
    }
}
impl<T: Clone> DerefMut for IrTable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if !matches!(self.data, Storage::Owned(_)) {
            let data = std::mem::replace(&mut self.data, Storage::Owned(Vec::new()));
            self.data = Storage::Owned(Self::into_vec(data));
        }
        let Storage::Owned(records) = &mut self.data else {
            unreachable!()
        };
        if self.capacity_hint > records.capacity() {
            records.reserve_exact(self.capacity_hint.saturating_sub(records.len()));
        }
        self.capacity_hint = 0;
        records
    }
}
impl<T> Index<usize> for IrTable<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        self.get_record(index)
            .expect("IR table index out of bounds")
    }
}
impl<T: Clone> IndexMut<usize> for IrTable<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        if matches!(self.data, Storage::Paged(_)) {
            let Storage::Paged(records) = &mut self.data else {
                unreachable!()
            };
            let records = Arc::make_mut(records);
            assert!(index < records.len, "IR table index out of bounds");
            records.flat.take();
            let page = Arc::make_mut(&mut records.pages[index >> records.shift]);
            &mut page[index & ((1usize << records.shift) - 1)]
        } else {
            &mut self.deref_mut()[index]
        }
    }
}
macro_rules! range_index {
    ($($range:ty),*) => { $(
        impl<T> Index<$range> for IrTable<T> {
            type Output = [T];
            fn index(&self, index: $range) -> &[T] { &self.deref()[index] }
        }
        impl<T: Clone> IndexMut<$range> for IrTable<T> {
            fn index_mut(&mut self, index: $range) -> &mut [T] { &mut self.deref_mut()[index] }
        }
    )* };
}
range_index!(
    std::ops::Range<usize>,
    std::ops::RangeFrom<usize>,
    std::ops::RangeTo<usize>,
    std::ops::RangeInclusive<usize>,
    std::ops::RangeToInclusive<usize>,
    std::ops::RangeFull
);

enum TableIteration<'a, T> {
    Flat(std::slice::Iter<'a, T>),
    Paged {
        records: &'a PagedRecords<T>,
        front: usize,
        back: usize,
    },
}

/// Streaming borrowed records; asking for a slice may materialize a paged view.
pub struct IrTableIter<'a, T> {
    inner: TableIteration<'a, T>,
}
impl<T> Clone for IrTableIter<'_, T> {
    fn clone(&self) -> Self {
        Self {
            inner: match &self.inner {
                TableIteration::Flat(iter) => TableIteration::Flat(iter.clone()),
                TableIteration::Paged {
                    records,
                    front,
                    back,
                } => TableIteration::Paged {
                    records,
                    front: *front,
                    back: *back,
                },
            },
        }
    }
}
impl<'a, T> IrTableIter<'a, T> {
    pub fn as_slice(&self) -> &'a [T] {
        match &self.inner {
            TableIteration::Flat(iter) => iter.as_slice(),
            TableIteration::Paged {
                records,
                front,
                back,
            } => {
                let records = *records;
                &records.contiguous()[*front..*back]
            }
        }
    }
}
impl<'a, T> Iterator for IrTableIter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            TableIteration::Flat(iter) => iter.next(),
            TableIteration::Paged {
                records,
                front,
                back,
            } => {
                if *front == *back {
                    return None;
                }
                let index = *front;
                *front += 1;
                let records = *records;
                records.record(index)
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match &mut self.inner {
            TableIteration::Flat(iter) => iter.nth(n),
            TableIteration::Paged { front, back, .. } => {
                if n >= *back - *front {
                    *front = *back;
                    None
                } else {
                    *front += n;
                    self.next()
                }
            }
        }
    }
}
impl<T> DoubleEndedIterator for IrTableIter<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            TableIteration::Flat(iter) => iter.next_back(),
            TableIteration::Paged {
                records,
                front,
                back,
            } => {
                if *front == *back {
                    return None;
                }
                *back -= 1;
                let records = *records;
                records.record(*back)
            }
        }
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        match &mut self.inner {
            TableIteration::Flat(iter) => iter.nth_back(n),
            TableIteration::Paged { front, back, .. } => {
                if n >= *back - *front {
                    *back = *front;
                    None
                } else {
                    *back -= n;
                    self.next_back()
                }
            }
        }
    }
}
impl<T> ExactSizeIterator for IrTableIter<'_, T> {
    fn len(&self) -> usize {
        match &self.inner {
            TableIteration::Flat(iter) => iter.len(),
            TableIteration::Paged { front, back, .. } => back - front,
        }
    }
}
impl<T> std::iter::FusedIterator for IrTableIter<'_, T> {}
impl<T: std::fmt::Debug> std::fmt::Debug for IrTableIter<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}
impl<'a, T> IntoIterator for &'a IrTable<T> {
    type Item = &'a T;
    type IntoIter = IrTableIter<'a, T>;
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
        Self::into_vec(self.data).into_iter()
    }
}
impl<T: Serialize> Serialize for IrTable<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}
impl<T: std::fmt::Debug> std::fmt::Debug for IrTable<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
impl<T: PartialEq> PartialEq for IrTable<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
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
    fn capacity_plans_stay_unallocated_until_population_and_do_not_enter_wire() {
        let mut table = IrTable::<u64>::default();
        table.defer_capacity(32);
        table.defer_capacity(4);
        table.share();
        let untouched = table.clone();
        assert_eq!(table.len(), 0);
        assert_eq!(table.capacity(), 0);
        assert_eq!(table.iter().count(), 0);
        assert!(matches!(table.data, Storage::Owned(_)));
        assert_eq!(serde_json::to_string(&table).unwrap(), "[]");
        table.push(7);
        assert!(table.capacity() >= 32);
        let address = table.as_ptr();
        table.extend(8..39);
        assert_eq!(table.len(), 32);
        assert_eq!(table.as_ptr(), address);
        assert_eq!(untouched.capacity(), 0);
        assert!(untouched.is_empty());
        let mut cancelled = untouched.clone();
        cancelled.shrink_to_fit();
        assert_eq!(cancelled.capacity(), 0);
        assert_eq!(cancelled.capacity_hint, 0);
        let decoded: IrTable<u64> = serde_json::from_str("[]").unwrap();
        assert_eq!(decoded.capacity_hint, 0);
    }

    #[test]
    fn capacity_plans_do_not_detach_an_existing_snapshot_on_read() {
        let original: IrTable<_> = vec![1u64, 2].into();
        let mut next = original.clone();
        next.defer_capacity(64);
        assert_eq!(original.as_ptr(), next.as_ptr());
        assert_eq!(next.iter().copied().collect::<Vec<_>>(), vec![1, 2]);
        next.push(3);
        assert!(next.capacity() >= 64);
        assert_ne!(original.as_ptr(), next.as_ptr());
        assert_eq!(&**original, &[1, 2]);
        assert_eq!(&**next, &[1, 2, 3]);
    }

    #[test]
    fn paged_writes_copy_only_affected_pages_and_invalidate_contiguous_views() {
        let mut edited: IrTable<_> = (0..11).map(|i| format!("record-{i}")).collect();
        for bad in [0, 3] {
            let error = edited.share_pages(bad).unwrap_err();
            assert_eq!(error, IrPageSizeError(bad));
            assert!(error.to_string().contains(&bad.to_string()));
        }
        edited.share_pages(4).unwrap();
        let old = edited.clone();
        let Storage::Paged(before) = &old.data else {
            panic!("paged");
        };
        edited[5].push('!');
        let Storage::Paged(after) = &edited.data else {
            panic!("paged");
        };
        assert!(!Arc::ptr_eq(before, after));
        for i in 0..3 {
            assert_eq!(Arc::ptr_eq(&before.pages[i], &after.pages[i]), i != 1);
        }
        assert!(after.flat.get().is_none());
        assert_eq!(old[5], "record-5");
        assert_eq!(edited.as_slice()[5], "record-5!");
        edited[6].push('?');
        let Storage::Paged(after) = &edited.data else {
            panic!("paged");
        };
        assert!(after.flat.get().is_none());
        assert_eq!(&edited[5..7], ["record-5!", "record-6?"]);
        assert_eq!(&edited[..2], ["record-0", "record-1"]);
        assert_eq!(&edited[9..], ["record-9", "record-10"]);
        assert_eq!(&edited[0..=1], ["record-0", "record-1"]);
        assert_eq!(&edited[..=0], ["record-0"]);
        assert_eq!(edited[..].len(), 11);
        edited[2..5].fill("changed".into());
        assert!(matches!(edited.data, Storage::Owned(_)));
        assert_eq!(&edited[2..5], ["changed", "changed", "changed"]);
        assert_eq!(old[2], "record-2");
        edited.share_pages(2).unwrap();
        for value in &mut edited {
            value.push('.');
        }
        assert!(matches!(edited.data, Storage::Owned(_)));
        assert_eq!(edited[2], "changed.");
    }

    #[test]
    fn paged_iteration_matches_slices_in_both_directions() {
        for size in [0, 1, 3, 4, 5, 9] {
            for page_size in [1, 2, 4, 16] {
                let expected: Vec<_> = (0..size).collect();
                let mut table: IrTable<_> = expected.clone().into();
                table.share_pages(page_size).unwrap();
                assert_eq!(table.iter().copied().collect::<Vec<_>>(), expected);
                assert_eq!(
                    table.iter().rev().copied().collect::<Vec<_>>(),
                    expected.iter().rev().copied().collect::<Vec<_>>()
                );
                assert_eq!(table.iter().count(), size);
                assert_eq!(table.iter().last(), expected.last());
                let Storage::Paged(data) = &table.data else {
                    panic!("paged");
                };
                assert!(data.flat.get().is_none(), "streaming must not flatten");
                let mut iter = table.iter();
                let mut reference = expected.iter();
                for step in 0..size + 3 {
                    assert_eq!(iter.len(), reference.len());
                    assert_eq!(iter.size_hint(), reference.size_hint());
                    if step % 2 == 0 {
                        assert_eq!(iter.next(), reference.next());
                    } else {
                        assert_eq!(iter.next_back(), reference.next_back());
                    }
                }
                for skip in [0, 1, size, usize::MAX] {
                    let mut iter = table.iter();
                    let mut reference = expected.iter();
                    assert_eq!(iter.nth(skip), reference.nth(skip));
                    assert_eq!(iter.clone().as_slice(), reference.as_slice());
                    let mut iter = table.iter();
                    let mut reference = expected.iter();
                    assert_eq!(iter.nth_back(skip), reference.nth_back(skip));
                    assert_eq!(iter.as_slice(), reference.as_slice());
                }
                assert_eq!(format!("{:?}", table.iter()), format!("{expected:?}"));
                assert_eq!(table.to_vec(), expected);
            }
        }
    }

    #[test]
    fn paged_encoding_is_identical_and_unique_consumption_moves_payloads() {
        let values = vec!["first".to_owned(), "second".to_owned(), "last".to_owned()];
        let mut table: IrTable<_> = values.clone().into();
        table.share_pages(2).unwrap();
        let allocation = table[2].as_ptr();
        let mut actual = Vec::new();
        let mut expected = Vec::new();
        ciborium::into_writer(&table, &mut actual).unwrap();
        ciborium::into_writer(&values, &mut expected).unwrap();
        assert_eq!(actual, expected);
        let Storage::Paged(data) = &table.data else {
            panic!("paged");
        };
        assert!(data.flat.get().is_none());
        let restored: IrTable<String> = ciborium::from_reader(actual.as_slice()).unwrap();
        assert_eq!(restored, table);
        let owned: Vec<_> = table.into_iter().collect();
        assert_eq!(owned, values);
        assert_eq!(owned[2].as_ptr(), allocation);
        let mut cached: IrTable<_> = values.clone().into();
        cached.share_pages(2).unwrap();
        let cached_ptr = cached.as_slice()[0].as_ptr();
        let first = cached.into_iter().next().unwrap();
        assert_eq!(first.as_ptr(), cached_ptr);
    }

    #[test]
    fn flat_tables_keep_generic_nonclone_read_support() {
        #[derive(Debug, PartialEq, serde::Serialize)]
        struct NonClone(u32);
        let table: IrTable<_> = vec![NonClone(7)].into();
        assert_eq!(table.as_slice(), [NonClone(7)]);
        assert_eq!(table.iter().clone().next(), Some(&NonClone(7)));
        assert_eq!(serde_json::to_string(&table).unwrap(), "[7]");
    }

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
