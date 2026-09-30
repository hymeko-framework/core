# Declaration source files and composed modules

Updated: 2026-09-09. Scope: `course-module-compilation`.

Core compiles the complete reachable import graph. Each canonical file is linked
once, in deterministic breadth-first order, with direct imports first. Each file's
`using path as alias` statements resolve against that file's own imports and local
declarations. An entry's aliases do not become implicit aliases in its dependencies.
Different modules may reuse an alias for different dependencies. Unresolved using
statements are reported, including those in imported files.

Relative imports resolve from the importing file's directory. `.` and `..` are
normalized before virtual file lookup; traversal above the provider root is an
error. The filesystem provider retains filesystem canonicalization and symlink
resolution after checking root traversal. SDK file-map keys remain normalized,
relative POSIX paths: `lectures/l01.hymeko` may import `../course.hymeko`, while the
map stores the latter as `course.hymeko`.

## Inspection fields

The canonical `Ir` has two additive source metadata tables, also returned inside
`inspect().ir` by the JavaScript/WASM SDK:

| Field | Meaning |
|---|---|
| `files: string[]` | Canonical source paths, in deterministic linked-module order; entry first. |
| `decl_file: u32[]` | Source-file table index for each **declaration index**, parallel to `decl_nodes`. |

For compiler-produced IR every declaration has a valid file index, including
anonymous arcs, which inherit their containing hyperedge's source file. The tables
describe reachable sources, so unused entries in the supplied file map do not
appear. Memory-provider paths use `/`; filesystem-provider paths are canonical
host paths and may be absolute. Source paths are metadata, not a portable content
identifier or an instruction to read a remote filesystem.

Legacy IR without these tables remains readable. If either field is present, both
must be present and declaration counts/file indices must be valid. JSON and CBOR
round trips preserve the tables. A synthetic declaration appended after compilation
uses `0xffffffff` to mean unknown source; clients must handle that sentinel. An empty
legacy table means provenance is unavailable, rather than assigning every declaration
to the first file. Clients that reorder declarations must reorder `decl_file` too.

```js
const { ir } = document.inspect();
const fileId = ir.decl_file?.[declaration.id];
const sourceFile = fileId === undefined || fileId === 0xffffffff
  ? undefined
  : ir.files?.[fileId];
```

`arcs[].refs` target **DeclIds**, not NodeIds. Follow the declaration dispatch
tables/accessors to determine whether the target is a node or a hyperedge. A
hyperedge can be a member of another hyperedge. The equality of a NodeId and a
DeclId in a small single-file example is incidental and must not be assumed.

## Identity compatibility

Source metadata is excluded from `ProgramStructureHashV1` and
`SemanticModelDigestV2`; moving a file does not itself change either digest.
Serialized IR bytes gain the new fields, while its existing fields retain their
values. Fixing a string's previously corrupted UTF-8 does change its value and can
change a complete semantic digest; a structural hash is not a content digest.

The existing 102-file successfully compiled corpus retains every structural hash.
Complete transitive imports additionally make `pick_place_scenario.hymeko`
compilable. A document whose dependency declarations were previously silently
omitted can gain structure under the corrected compiler; that can change its
structural hash. No hash algorithm or schema version was changed.

The resolution `Index` retains transient module-local environments only during
compilation. Those environments are not serialized: the resulting IR already has
resolved declaration references. Consumers must use the canonical IR after lowering.
