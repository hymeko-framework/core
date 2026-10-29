Native Core development linkage -- 2026-10-28

Run from the Core workspace:
  cargo dev-test --test circular_buffer
  cargo dev-test --test mod product_cli
  cargo dev-test --test mod cbor_roundtrip
  cargo dev-run -- PATH_TO_MODEL.hymeko

The aliases select a publish=false downstream client and the dev-dll profile.
The DLL is a re-export of the existing local hymeko_core crate. Test and CLI
targets point directly to their original source files: no source copy, second
compiler, new IR, or separate algorithm implementation exists here.

The two original integration-test targets are available. Select correctness
filters before execution; ignored measurement cells remain off by default.
Production library unit tests remain under cargo test -p hymeko_core --lib.
Allocation instrumentation, benchmarks and publication controls retain their
original static targets/profiles; the DLL path is not new benchmark evidence.

The profile keeps incremental compilation and line-table debug information.
Source-level backtrace locations remain, but full local-variable debugging does
not. Use the ordinary dev profile when full variable/type debugging is needed.
No global RUSTFLAGS, compiler replacement or new external dependency is used.

Cargo owns DLL discovery. Run through cargo dev-run / cargo dev-test; copying an
EXE alone is not a distributable application. A standalone bundle also needs the
matching Core DLL, Rust standard-library DLL and native runtime dependencies.
Rust dylibs are development artifacts tied to the exact compiler and dependency
build; they are not a stable C ABI or drop-in upgrade boundary.

Account for the DLL once when comparing several clients. A smaller EXE does
not by itself establish lower total disk use; a small standalone static client
may be smaller than the complete DLL bundle.

Changing source, toolchain, profile, features or dependency revisions can still
require compilation. Unchanged inputs should be Fresh. Avoid cargo clean: the
machine's shared build.build-dir is also used by other workspaces. Cargo JSON
compiler-artifact records, not guessed target/deps paths, identify the outputs.

CACHE IDENTITY
The aliases place intermediates under
  {cargo-cache-home}/hymeko-dev-build/{workspace-path-hash}
Cargo expands these placeholders using the actual workspace manifest. Clients
in one workspace share this cache; distinct source roots cannot reuse each
other's local package outputs. A first build for each workspace is required.
This prevents false Fresh results observed with an unqualified shared build-dir
when switching between an edited checkout and a different-source verification
export. Ordinary static/benchmark profiles are not changed by the aliases.
The first location is Cargo's cache home, not a hard-coded machine path.
