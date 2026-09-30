//! Canonical, portable CBOR interchange for a compiled HyMeKo program.
//!
//! This module is the only owner of the CBOR schema. Transports carry the encoded bytes unchanged;
//! consumers call [`decode`] before using an IR. Decoding is fail-closed: the byte stream is
//! allocation-preflighted, the string table and IDs are checked, the path index is rebuilt from the
//! canonical IR, structural coherence is validated, and the canonical hash is recomputed.
//!
//! [`encode_snapshot_messages`] and [`decode_snapshot`] carry one encoded document through an
//! `ir_stream` snapshot transaction in `CanonicalIrCbor` fragments; the consumer side ends in
//! [`decode`].

mod preflight;
// APPROVED-CORE-EDIT: ir-stream-cbor-snapshot (2026-09-15, user chat).
mod stream;
#[cfg(test)]
mod tests;
mod validate;

pub use stream::{
    CborStreamError, SnapshotMessageDraft, decode_snapshot, encode_snapshot_messages,
};

use crate::common::ids::SymId;
use crate::ir::canonical_hash::{Canon, CanonHashCfg};
use crate::ir::hash::HashId;
use crate::ir::ir::Ir;
use crate::ir_stream::DecodeLimits;
use crate::module_store::module_store::{CompiledProgram, ModuleKey};
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Current schema on `hymeko/model/ir/cbor/v1`.
pub const CBOR_SCHEMA_VERSION: u16 = 1;

/// Lossless compiled-program payload.
///
/// `index` and `canon_hash` are redundant witnesses, not authorities. [`decode`] reconstructs and
/// verifies them against `ir`; callers never have to trust serialized derived state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CborPayload {
    /// Explicit wire schema. Missing and unsupported versions are rejected.
    pub schema_version: u16,
    /// Workspace-relative entry module path. Absolute and parent-traversing paths are rejected.
    pub root_path: PathBuf,
    pub ir: Ir,
    pub index: Index,
    pub interned_strings: Vec<String>,
    pub canon_hash: HashId,
    pub imports: Vec<(SymId, ModuleKey)>,
}

/// A payload that has passed every core-owned CBOR boundary check.
/// Its table contains only referenced symbols. Dense symbol IDs may differ from
/// the wire input; declaration IDs and canonical model meaning are preserved.
pub struct DecodedCborPayload {
    payload: CborPayload,
    interner: Interner,
}

impl DecodedCborPayload {
    #[must_use]
    pub fn payload(&self) -> &CborPayload {
        &self.payload
    }

    #[must_use]
    pub fn ir(&self) -> &Ir {
        &self.payload.ir
    }

    #[must_use]
    pub fn index(&self) -> &Index {
        &self.payload.index
    }

    #[must_use]
    pub fn interner(&self) -> &Interner {
        &self.interner
    }

    #[must_use]
    pub fn canonical_hash(&self) -> HashId {
        self.payload.canon_hash
    }

    #[must_use]
    pub fn into_parts(self) -> (CborPayload, Interner) {
        (self.payload, self.interner)
    }
}

#[derive(Debug)]
pub enum CborError {
    MessageTooLarge {
        got: u64,
        max: u64,
    },
    LimitExceeded {
        what: &'static str,
        got: u64,
        max: u64,
    },
    InvalidCbor(String),
    UnsupportedSchema {
        got: u16,
        expected: u16,
    },
    NonPortablePath {
        what: &'static str,
    },
    DuplicateInternedString {
        index: usize,
    },
    SymbolOutOfRange {
        field: &'static str,
        symbol: usize,
    },
    IdOutOfRange {
        field: &'static str,
        id: usize,
    },
    IndexMismatch,
    NamespaceMismatch,
    IncoherentIr {
        findings: usize,
    },
    CanonicalHashMismatch {
        expected: HashId,
        got: HashId,
    },
}

impl fmt::Display for CborError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MessageTooLarge { got, max } => {
                write!(f, "CBOR message has {got} bytes; limit is {max}")
            }
            Self::LimitExceeded { what, got, max } => {
                write!(f, "CBOR {what} is {got}; limit is {max}")
            }
            Self::InvalidCbor(message) => write!(f, "invalid CBOR: {message}"),
            Self::UnsupportedSchema { got, expected } => {
                write!(f, "unsupported CBOR schema {got}; expected {expected}")
            }
            Self::NonPortablePath { what } => {
                write!(f, "{what} must be a workspace-relative portable path")
            }
            Self::DuplicateInternedString { index } => {
                write!(f, "interned string {index} duplicates an earlier entry")
            }
            Self::SymbolOutOfRange { field, symbol } => {
                write!(f, "{field} contains out-of-range symbol {symbol}")
            }
            Self::IdOutOfRange { field, id } => {
                write!(f, "{field} contains out-of-range id {id}")
            }
            Self::IndexMismatch => write!(f, "serialized index disagrees with canonical IR"),
            Self::NamespaceMismatch => {
                write!(f, "serialized namespace paths disagree with canonical IR")
            }
            Self::IncoherentIr { findings } => write!(
                f,
                "decoded IR failed structural coherence with {findings} findings"
            ),
            Self::CanonicalHashMismatch { expected, got } => write!(
                f,
                "canonical hash mismatch: expected {:?}, got {:?}",
                expected, got
            ),
        }
    }
}

impl std::error::Error for CborError {}

/// Encode the canonical result returned by [`crate::module_store::module_store::ModuleStore`].
///
/// This is the producer facade. It converts the store's canonical host paths into workspace-relative
/// wire paths and then delegates to [`encode`]. No caller has to reconstruct `CborPayload` by hand.
pub fn encode_compiled(
    workspace_root: &Path,
    program: &CompiledProgram,
    interner: &Interner,
) -> Result<Vec<u8>, CborError> {
    // `StdFsProvider` stores canonical paths. On Windows those commonly carry the `\\?\` prefix,
    // while callers naturally pass an ordinary workspace path; compare canonical forms first.
    let canonical_workspace_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let root_path = program
        .root
        .0
        .strip_prefix(&canonical_workspace_root)
        .or_else(|_| program.root.0.strip_prefix(workspace_root))
        .map_err(|_| CborError::NonPortablePath { what: "root_path" })?
        .to_path_buf();
    let imports: Vec<(SymId, ModuleKey)> = program
        .imports
        .iter()
        .map(|(namespace, module)| {
            module
                .0
                .strip_prefix(&canonical_workspace_root)
                .or_else(|_| module.0.strip_prefix(workspace_root))
                .map(|path| (*namespace, ModuleKey(path.to_path_buf())))
                .map_err(|_| CborError::NonPortablePath {
                    what: "import path",
                })
        })
        .collect::<Result<_, _>>()?;
    encode(root_path, &program.ir, &program.idx, interner, &imports)
}

/// Encode one compiled program without trusting caller-supplied derived witnesses.
pub fn encode(
    root_path: impl Into<PathBuf>,
    ir: &Ir,
    index: &Index,
    interner: &Interner,
    imports: &[(SymId, ModuleKey)],
) -> Result<Vec<u8>, CborError> {
    let root_path = validate::normalize_portable_path(&root_path.into(), "root_path")?;
    let imports = imports
        .iter()
        .map(|(namespace, module)| {
            validate::normalize_portable_path(&module.0, "import path")
                .map(|path| (*namespace, ModuleKey(path)))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let strings: Vec<String> = interner.iter().map(|(_, value)| value.to_owned()).collect();
    let payload = CborPayload {
        schema_version: CBOR_SCHEMA_VERSION,
        root_path,
        ir: ir.clone(),
        index: index.clone(),
        interned_strings: strings,
        canon_hash: HashId([0; 32]),
        imports,
    };
    let validated = validate::payload(payload, false)?;
    let (mut payload, reconstructed_interner) = validated.into_parts();
    payload.canon_hash = Canon::new(&payload.ir, &reconstructed_interner)
        .structure(CanonHashCfg::default())
        .0;

    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&payload, &mut bytes)
        .map_err(|error| CborError::InvalidCbor(error.to_string()))?;
    Ok(bytes)
}

/// Decode untrusted CBOR into a core-validated IR and its reconstructed interner.
///
/// A structural preflight runs before serde is allowed to allocate. Only definite-length CBOR is
/// accepted; every string and container length is checked against the existing [`DecodeLimits`].
pub fn decode(bytes: &[u8], limits: &DecodeLimits) -> Result<DecodedCborPayload, CborError> {
    preflight::check(bytes, limits)?;
    let payload: CborPayload = ciborium::de::from_reader(bytes)
        .map_err(|error| CborError::InvalidCbor(error.to_string()))?;
    validate::limits(&payload, limits)?;
    validate::payload(payload, true)
}
