use crate::common::ids::SymId;
use rustc_hash::FxHashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct Interner {
    data: Arc<InternerData>,
}

#[derive(Default, Clone)]
struct InternerData {
    map: FxHashMap<Arc<str>, SymId>,
    vec: Vec<Arc<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> SymId {
        // Fast path: String already exists
        if let Some(&id) = self.data.map.get(s) {
            return id;
        }

        // Detach only on insertion. Existing strings remain immutable and shared
        // with readers; the map owns its keys without artificial lifetimes.
        let data = Arc::make_mut(&mut self.data);
        let id = SymId::new(data.vec.len());
        let text: Arc<str> = s.into();
        data.map.insert(Arc::clone(&text), id);
        data.vec.push(text);
        id
    }

    pub fn resolve(&self, id: SymId) -> &str {
        &self.data.vec[id.0]
    }

    /// Iterator over all interned strings and their SymIds.
    pub fn iter(&self) -> impl Iterator<Item = (SymId, &str)> + '_ {
        self.data.vec.iter().enumerate().map(|(i, s)| {
            // Stable alternative to .as_str()
            (SymId::new(i), s.as_ref())
        })
    }
}

impl Interner {
    pub fn get_id(&self, s: &str) -> Option<SymId> {
        self.data.map.get(s).copied()
    }

    pub(crate) fn fork(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
        }
    }

    /// Keep live strings in their original relative order. A mapping is needed
    /// only when a retained ID moves; removing a trailing suffix does not move IDs.
    pub(crate) fn retained(&self, used: &[bool]) -> (Self, Option<Vec<SymId>>) {
        // SymbolCompaction creates one mark per source symbol before its checked
        // walk. A different length is an internal caller invariant violation.
        assert_eq!(used.len(), self.data.vec.len());
        if used.iter().all(|&live| live) {
            return (self.fork(), None);
        }
        let mut mapping = vec![SymId::NONE; used.len()];
        let mut renumbered = false;
        let vec: Vec<_> = self
            .data
            .vec
            .iter()
            .enumerate()
            .filter_map(|(old, text)| {
                if !used[old] {
                    return None;
                }
                Some((old, Arc::clone(text)))
            })
            .enumerate()
            .map(|(new, (old, text))| {
                mapping[old] = SymId::new(new);
                renumbered |= old != new;
                text
            })
            .collect();
        let map = vec
            .iter()
            .enumerate()
            .map(|(id, text)| (Arc::clone(text), SymId::new(id)))
            .collect();
        // Only live string allocations are shared, never the old table itself.
        let interner = Self {
            data: Arc::new(InternerData { map, vec }),
        };
        (interner, renumbered.then_some(mapping))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_detaches_on_new_text_and_preserves_both_readers() {
        let mut base = Interner::new();
        let first = base.intern("first");
        let mut next = base.fork();
        assert_eq!(next.intern("first"), first);
        assert!(Arc::ptr_eq(&base.data, &next.data));
        let second = next.intern("second");
        assert_eq!(second, SymId::new(1));
        assert_eq!(base.get_id("second"), None);
        assert_eq!(base.intern("third"), second);
        assert_eq!(next.get_id("third"), None);
        drop(base);
        assert_eq!(next.resolve(first), "first");
        assert_eq!(next.resolve(second), "second");
    }

    #[test]
    fn retention_releases_dead_strings_without_retaining_the_source_table() {
        let mut source = Interner::new();
        for text in ["first", "dead", "last"] {
            source.intern(text);
        }
        let dead = Arc::downgrade(&source.data.vec[1]);
        let (all, mapping) = source.retained(&[true, true, true]);
        assert!(mapping.is_none());
        assert!(Arc::ptr_eq(&source.data, &all.data));
        let (kept, mapping) = source.retained(&[true, false, true]);
        assert_eq!(
            mapping.unwrap(),
            [SymId::new(0), SymId::NONE, SymId::new(1)]
        );
        assert!(Arc::ptr_eq(&source.data.vec[2], &kept.data.vec[1]));
        assert_eq!(kept.get_id("dead"), None);
        drop(source);
        assert!(
            dead.upgrade().is_some(),
            "explicit reader still owns old text"
        );
        drop(all);
        assert!(
            dead.upgrade().is_none(),
            "compacted table must not keep dead text"
        );
        assert_eq!(kept.resolve(SymId::new(1)), "last");
        let (prefix, mapping) = kept.retained(&[true, false]);
        assert!(mapping.is_none());
        assert_eq!(
            prefix.iter().collect::<Vec<_>>(),
            [(SymId::new(0), "first")]
        );
        let (empty, mapping) = kept.retained(&[false, false]);
        assert!(mapping.is_none());
        assert_eq!(empty.iter().count(), 0);
    }
}
