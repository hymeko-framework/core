# The HyMeKo language — what it can express

A reference for **authoring `.hymeko` models**, derived from the grammar
(`parser/src/hymeko.lalrpop`) rather than from example files. Every construct below was compiled with
the CLI before being written down. Individual snippets are illustrative and may name files or
vocabulary that exist only in their surrounding context; the complete, compiling example is the
mechanism fixture linked at the end.

HyMeKo describes a **hypergraph**: named declarations (nodes), hyperedges over them, and *signed*
incidences between the two. It is a modelling language, not a programming language — there is no
control flow, and the only computation is constant folding at compile time.

## Verify before you believe it

```bash
hymeko_core <file.hymeko>
```

| exit | meaning |
|---|---|
| `0` | compiles; prints the canonical program hash, index size, IR decl/edge/arc counts and the declaration hierarchy |
| `1` | compile failure — a parse error (with the expected tokens) or an `UnresolvedRef` naming the scope and the target |
| `2` | usage error (no path given) |

The binary is built from `hymeko_core/src/main.rs`; after `cargo build --release` it is
`target/release/hymeko_core`. It takes one path and nothing else — there are no flags.

**A model that has not been through the CLI is a draft.** An unresolved reference is not a warning
here; it fails the compile, which means a typo in a path is caught immediately rather than surfacing
later as a missing link in an emitted URDF.

## Shape of a file

```hymeko
description_name {
    // header (optional): imports, usings, consts, bare declarations
}
// items: nodes, edges, arcs
```

The first identifier names the description. The optional `{ … }` immediately after it is the
**header**, and it is the only place imports, `using` and `const` may appear.

## Header

```hymeko
demo {
    @"constants/meta_kinematics.hymeko";           // import, path relative to this file
    @"other.hymeko" -> mm;                         // import under an alias
    using kinematics.elements as el;               // shorten a path
    const DEG = 0.017453292519943295;              // compile-time constant
    marker_tag;                                    // a bare declaration
}
```

Imports resolve by path relative to the importing file. After `@"06_meta.hymeko" -> mm;` the imported
description is reachable as `mm.…`; without an alias it is reachable under its own description name.

## Declarations (nodes)

Four forms, all verified:

```hymeko
block { inner {} }                    // a block, nesting freely
typed: base_a, base_b { x 1.0; }      // a block with base references — this is instantiation
alias -> some.other.decl;             // a reference assignment
attribute 42.0;                       // a statement declaration — this is how attributes are written
```

An **attribute is just a declaration carrying a value**. `mass 2.0;`, `lower -180.0;`,
`type "joint_state_broadcaster/JointStateBroadcaster";` are all the same construct. There is no
separate attribute syntax to learn, and consumers read them by name
(`find_child_num(ir, res, did, "mass")`).

Because attributes are declarations, they can also be **blocks that are referenced and shared**:

```hymeko
joint_rev_limit {
    lower -180.0;
    upper  180.0;
    effort   50.0;
    velocity  1.0;
}
@rev_joint: + <isa> elements.joint {
    limit -> joint_rev_limit;          // every revolute joint inherits this default
}
```

This is the pattern to copy when a new attribute is needed: name it, give it a block, reference it.
It is inheritable (a base declares the default, an instance overrides it), shareable across
declarations, and self-describing — none of which a positional value list is.

## Values

```hymeko
s   "text";
n   42.0;
neg -3.5;
list [1.0, 2.0, 3.0];
empty [];
ref  some.other.decl;
expr (BASE * 3.0 + 1.0);
```

Lists nest, which is how a joint origin travels as `[[x,y,z],[r,p,y]]`.

### Compile-time expressions

Inside parentheses: `+ - * /`, unary minus, and standard precedence. Two built-ins only — `pi` and
`exp(x)`. A bare identifier is a reference to a `const`.

```hymeko
const BASE   = 2.0;
const CIRCLE = (2.0 * pi);
const DECAY  = (exp(1.0));
used (BASE * 10.0);
```

Any other function name parses but fails at resolution — `sin(1.0)` gives
`compile failed: Parse("const: undefined const reference \`sin\`")` and exit 1 — deliberately, so a
typo is a compile error rather than a silent zero.

## Tags

`<angle, bracketed>` lists attach to declarations, edges, arcs and to individual signed references:

```hymeko
tagged_node <alpha, beta> { x 1.0; }
tagged_stmt <unit> 5.0;
```

Tags are metadata that survives into the IR (`AnnoR::tags`); the framework uses `<isa>` on a base
reference to mean inheritance.

## Edges and signed incidences

An `@` sigil makes a declaration an **edge**. Inside it, a parenthesised list of signed references is
an **arc** — the hypergraph incidence:

```hymeko
@e1 {
    ( +a, -b ) ;                       // + and - incidences
    ( +a, -b, ~c ) ;                   // ~ is neutral
    ( + <parent> a, - <child> b ) ;    // references carry their own tags
    ( +a [0.0, 0.0, 0.1], -b ) ;       // …and their own values ("weights")
}
```

Three things ride on one incidence, and they are independent:

| | what it is |
|---|---|
| sign | direction: `+` / `-` / `~` (three-valued, not a number) |
| value | arbitrary — a number, a string, a list, a nested list, or a reference |
| tags | metadata |

The sign is *not* a weight. A weight of `0.87` and a `+` incidence are orthogonal facts.

## The meta-model pattern

HyMeKo has no built-in vocabulary of robots or circuits. A meta-model declares the vocabulary; a
model instantiates it by base reference. This is why `: base` and `<isa>` matter more than any
keyword:

```hymeko
// vocabulary
elements {
    meta_element {}
    link: + <isa> meta_element {}
    @joint {}
}
axes {
    axis_definition {}
    AXIS_Z: + <isa> axis_definition { ax [0.0, 0.0, 1.0]; }
}
@rev_joint: + <isa> elements.joint {}
```

Consumers then query *by inheritance* — `Predicate::node().and(Predicate::inherits("link"))` — so
anything that inherits `link` is a link, whatever it is called.

## Worked example: a mechanism

The convention a kinematic joint follows, which is the easiest thing to get wrong:

```hymeko
@j1: rev_joint {
    (+ base [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0]], - upper, - axes.AXIS_Z);
}
```

- `+ base` — the **parent** link, and its value is the joint origin `[[xyz], [rpy in degrees]]`
- `- upper` — the **child** link
- `- axes.AXIS_Z` — the axis, a reference to something inheriting `axis_definition`

`hymeko_geometry` reads exactly this and produces link poses; the fixture at
`transformation/hymeko_geometry/tests/data/two_link_arm.hymeko` is a complete, compiling example, and
`tests/end_to_end.rs` drives it from source to poses.

## What the language does not have

Worth knowing before trying to express it:

- no conditionals, loops or user-defined functions — the only computation is constant folding;
- no arithmetic on anything but `const` numerics (no vector algebra in the source);
- no types beyond the inheritance relation — `link` is a declaration to inherit from, not a schema
  that validates fields, so a misspelled attribute name is not an error, it is simply a declaration
  nobody reads;
- no visibility or namespacing beyond imports and `using`.

The last point is the one that bites: `mas 2.0;` compiles cleanly and silently means nothing. The
CLI's hierarchy dump is the check — the declaration will be there under a name you did not intend.
