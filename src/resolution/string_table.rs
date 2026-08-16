use crate::common::ids::SymId;
use crate::resolution::interner::Interner;
use std::sync::Arc;

// ================================================
// StringTable: lightweight wrapper around the Interner for Python access (frozen interner snapshot)
// ================================================

#[derive(Clone, Debug)]
pub struct StringTable(Arc<Vec<String>>);

impl StringTable {
    pub fn from_interner(interner: &Interner) -> Self {
        let strings = interner.iter().map(|(_, s)| s.to_string()).collect();
        Self(Arc::new(strings))
    }

    pub fn from_vec(strings: Vec<String>) -> Self {
        Self(Arc::new(strings))
    }

    #[inline]
    pub fn resolve(&self, id: SymId) -> &str {
        &self.0[id.0]
    }

    pub fn to_vec(&self) -> Vec<String> {
        // self.0 accesses the Arc inside the tuple struct
        // (*self.0) dereferences the Arc to get the actual Vec
        // .clone() duplicates the vector for the CborPayload
        (*self.0).clone()
    }
}
