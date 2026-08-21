// Wall-clock helpers used by `lower_to_ir` / `lower_program_to_ir` to
// stamp `Meta::created_at_unix_ns`. On wasm32 the std::time module is
// `not implemented` and panics with "time not implemented on this
// platform" — that bubbles up to the browser as a wasm-trap
// "unreachable" and breaks the demo's `parse_and_compile`. Gate the
// real impl behind cfg(not(wasm32)) and return 0 on wasm so the build
// stays portable. The Meta timestamp is informational only — canonical
// hashing in `canonical_hash::canonical_program_hash` deliberately
// excludes Meta from the hash, so a 0 stamp on wasm doesn't change
// hash compatibility with native builds.

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
pub fn now_ns() -> i128 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime before UNIX_EPOCH");
    (d.as_secs() as i128) * 1_000_000_000 + (d.subsec_nanos() as i128)
}

#[cfg(target_arch = "wasm32")]
pub fn now_ns() -> i128 {
    0
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now_ns_u128() -> u128 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime before UNIX_EPOCH");
    (d.as_secs() as u128) * 1_000_000_000 + (d.subsec_nanos() as u128)
}

#[cfg(target_arch = "wasm32")]
pub fn now_ns_u128() -> u128 {
    0
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> i64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime before UNIX_EPOCH");
    (d.as_secs() as i64) * 1000 + (d.subsec_millis() as i64)
}

#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> i64 {
    0
}
