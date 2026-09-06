---
source: CLAUDE.md §11 Authoring HyMeKo models, lines 665-709, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: verbatim extraction — not yet de-projectified
---

**Contains:** §11 the CLI gate · where the language is written down · the failure mode · adding an attribute

## 11. Authoring HyMeKo models

`.hymeko` files are **source**, and they get the same discipline as Rust: nothing is claimed to work
until it has been through the compiler.

### The gate

```bash
cargo build --release            # once
target/release/hymeko_core <file.hymeko>
```

Exit `0` compiles, `1` is a parse or resolution failure, `2` is a usage error. Run it on every model
you write or edit, and on every example you put in a document. **A model that has not been through
the CLI is a draft**, and an example in prose that has not been through it is a liability — it will
be copied.

### Where the language is written down

[`docs/hymeko-language.md`](../../hymeko-language.md) — what the language can express, derived from
the grammar (`parser/src/hymeko.lalrpop`), with every construct CLI-verified. Read it before
authoring a model rather than pattern-matching off the nearest existing file; several things that
look like syntax are conventions a particular consumer reads, and the two are easy to confuse.

### The failure mode that matters

HyMeKo has **no schema validation**. Inheritance is a relation, not a type: `link` is something to
inherit from, not a declaration of which fields are legal. So a misspelled attribute is not an error
— `mas 2.0;` compiles cleanly, exit 0, and simply means nothing to the consumer looking for `mass`.

A green compile therefore proves the model *parses and resolves*, not that it says what you meant.
Check the CLI's declaration hierarchy dump, and prefer a test that reads the model back through its
actual consumer (see `transformation/hymeko_geometry/tests/end_to_end.rs`, which drives a fixture
from source to link poses) over eyeballing the source.

### Adding a new attribute

Name it, give it a block, reference it — the `limit -> joint_rev_limit` pattern in
`data/minimal_examples/constants/meta_kinematics.hymeko`. Attributes declared this way are
inheritable, shareable and self-describing. Do **not** encode new meaning positionally in an
incidence's value list; that is how the joint origin travels (`[[xyz],[rpy]]`) and it is already the
fragile part of the model — a swapped pair is silently a different pose.
