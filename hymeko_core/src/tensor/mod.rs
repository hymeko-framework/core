pub mod aggregation;
pub mod aggregation_impl;
#[cfg(feature = "arrow-schema")]
pub mod arrow_schema;
pub mod common;
pub mod compute;
pub mod conv;
mod decomposition;
pub mod representations;
#[cfg(feature = "ipc")]
pub mod shared_state;
/// Deterministic sparse-CSR workload generator (benchmark/test support; see module docs).
#[doc(hidden)]
pub mod sparse_workload;
pub mod tensor_convert;
pub mod tensor_val;
pub mod util;
