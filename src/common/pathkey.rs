use crate::common::ids::SymId;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PathKey(pub Vec<SymId>);

impl PathKey {
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = SymId> + '_ {
        self.0.iter().copied()
    }
}

impl Borrow<[SymId]> for PathKey {
    fn borrow(&self) -> &[SymId] {
        &self.0
    }
}
