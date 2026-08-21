use crate::common::ids::SymId;
use crate::ir::hash::HashId;
use crate::ir::ir::Ir;
use crate::module_store::module_store::ModuleKey;
use crate::resolution::resolve::Index;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The exact binary payload we push into the CBOR / QR code
#[derive(Serialize, Deserialize)]
pub struct CborPayload {
    pub root_path: PathBuf,
    pub ir: Ir,
    pub index: Index, // Note: Your Index and PathKey must derive Serialize/Deserialize
    pub interned_strings: Vec<String>,
    pub canon_hash: HashId,
    pub imports: Vec<(SymId, ModuleKey)>,
}
