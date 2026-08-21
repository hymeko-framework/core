use crate::common::ids::SymId;
use rustc_hash::FxHashMap;

#[derive(Default)]
pub struct Interner {
    map: FxHashMap<&'static str, SymId>, // Pointer back into 'vec'
    vec: Vec<Box<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> SymId {
        // Fast path: String already exists
        if let Some(&id) = self.map.get(s) {
            return id;
        }

        // Slow path: One-time allocation
        let id = SymId::new(self.vec.len());
        let boxed: Box<str> = s.into();
        let static_ref: &'static str = unsafe { &*(boxed.as_ref() as *const str) };
        // Safety: We manage the lifetime of the string in the 'vec'.
        // As long as the interner isn't dropped and we don't remove from 'vec',
        // the pointer in 'map' remains valid.

        self.vec.push(boxed);
        self.map.insert(static_ref, id);
        id
    }

    pub fn resolve(&self, id: SymId) -> &str {
        &self.vec[id.0]
    }

    /// Iterator over all interned strings and their SymIds.
    pub fn iter(&self) -> impl Iterator<Item = (SymId, &str)> + '_ {
        self.vec.iter().enumerate().map(|(i, s)| {
            // Stable alternative to .as_str()
            (SymId::new(i), s.as_ref())
        })
    }
}

impl Interner {
    pub fn get_id(&self, s: &str) -> Option<SymId> {
        self.map.get(s).copied()
    }
}
