# ADR 0037: Typed Rust Foundation-Adapter Attribute

- Status: accepted
- Date: 2026-07-12
- Depends on: ADR 0018, ADR 0024, ADR 0035, and ADR 0036
- Supersedes: none

## Context

The native bootstrap functions implemented in Rust repeat several facts across
the function definition, bootstrap schema, backend symbol selection, and tests.
That repetition makes a small native adapter harder to add and lets its Rust ABI
drift from the typed Pop contract.

Automatically treating every annotated Rust function as a Pop declaration would
create a second language/API-definition path. Filesystem scanning, linker-section
registration, or runtime name lookup would also hide ownership and weaken the
explicit foundation-library inventory required by ADR 0035.

## Decision

The host implementation provides a Rust procedural attribute named `poplib` for
the two reserved foundation-library implementation crates. Its canonical form is:

```rust
#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "print",
    parameters(Int),
    results(),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_print_int(value: i64) {
    // Native adapter body.
}
```

`bubble` is exactly `Standard` or `Internal`. The namespace and function or
intrinsic name are compile-time binding metadata, not runtime lookup keys. The
parameter, result, and effect lists use a closed vocabulary shared with trusted
bootstrap/reference metadata. The initial native ABI mapping is intentionally
small: Pop `Int` maps to Rust `i64`, Pop `String` and trusted managed references
map to the accepted bootstrap `u64` handle, and an empty result list maps to
Rust `()`.

The attribute:

- requires a public, non-generic `extern "C"` function;
- emits the fixed unmangled native symbol named by that Rust function;
- generates an immutable `NativeExport` descriptor beside the function;
- emits a Rust function-pointer type assertion so an ABI/signature mismatch is
  a compile-time error;
- rejects unknown bubbles, types, effects, malformed arguments, duplicated
  fields, and unsupported Rust item shapes.

The generated descriptor does not itself create a Pop Item, assign a
`StandardFunctionId`, `SymbolId`, or intrinsic ID, change visibility, or add a
prelude binding. A build-time verifier must match it exactly to one accepted
public `Pop.Standard` declaration/bootstrap entry or one trusted `Pop.Internal`
intrinsic schema entry before linking. Missing, duplicate, or mismatched
bindings are toolchain errors. `Pop.Standard` portable algorithms remain normal
`.pop` source and do not use this attribute merely for convenience.

Each foundation crate exposes one explicit static descriptor slice. Extending
an existing adapter module adds its generated descriptor to that module's slice;
the thin crate root composes the reviewed module slices. Procedural expansion
does not scan directories, write generated files, use linker inventory sections,
or register values at runtime.

Two standard-library-only host crates implement this facility:

- `pop-library-bridge` owns the closed descriptor types and re-exports the
  attribute;
- `pop-library-macros` owns the standard-library-only procedural expansion and
  uses no third-party parser or code-generation dependencies.

They are Rust implementation support crates, not Pop Lang Bubbles, Packages,
Modules, namespaces, public library tiers, or source syntax. They do not change
the fixed `Pop.Internal` and `Pop.Standard` Bubble identities.

## Consequences

- Native adapter declarations become short and locally reviewable while their
  Rust ABI is checked by the compiler.
- The same descriptor shape can feed bootstrap validation and later `.poplib`
  implementation metadata without introducing a second semantic identity.
- Contributors still need accepted architecture and Pop metadata for a new
  public API or trusted intrinsic; the annotation removes Rust glue, not review.
- The explicit slice requires one visible inventory entry for each adapter and
  prevents hidden registration.
- The initial closed ABI vocabulary rejects richer types until their ownership,
  representation, effects, GC behavior, and cross-backend contract are accepted.

## Alternatives considered

### Let `#[poplib]` create public Pop declarations

Rejected because Rust source would become a second public language definition,
bypass Pop visibility/documentation checks, and make source identity depend on a
host implementation detail.

### Infer bindings from Rust names

Rejected because abbreviations such as `std` and ABI suffixes are not canonical
Pop namespace/function identities, and overloads cannot be bound safely from a
name convention.

### Discover annotated functions automatically

Rejected because filesystem scans, linker sections, and runtime registries hide
the reviewed ownership inventory and are less deterministic or portable than an
explicit typed slice.

## Required conformance tests

- both `Standard` and `Internal` descriptors can be generated;
- the generated function remains callable through its exact C ABI;
- descriptor bubble, binding, signature, effects, and native symbol are exact;
- wrong Rust parameter/result types fail compilation;
- private, non-C-ABI, generic, malformed, duplicated, and unknown descriptor
  inputs fail compilation;
- the two foundation crates retain explicit descriptor inventories;
- the standard print adapters use the attribute and no longer repeat manual
  `no_mangle` declarations;
- no runtime registration, string dispatch, generated-file scan, or third-party
  macro dependency is introduced;
- architecture tests retain exactly two foundational Pop Bubbles and the
  accepted Rust workspace dependency direction.

## Documents/components affected

Rust workspace boundaries, base libraries, foundation-library contributor
guidance, architecture conformance tests, bootstrap metadata validation,
`pop-internal`, and `pop-standard`.
