# ADR 0086: Canonical FFI Layout Identities and Source Catalogs

- Status: accepted
- Date: 2026-07-15
- Supersedes: none
- Extends: ADR 0055, ADR 0081, ADR 0082, ADR 0083, and ADR 0084

## Context

ADR 0082 defines target-selected ABI layouts and full SHA-256 layout
fingerprints. ADR 0084 requires every canonical MIR Bubble to carry a validated
layout catalog keyed by a stable nonzero `FfiAbiLayoutId`. Neither decision
defines how an accepted source ABI type obtains that compact identity or how
the trusted `Ffi.C.Layout` attachment reaches the catalog.

Using session-local `TypeId` values, declaration order, a backend type, or a
host compiler layout would make artifacts nondeterministic and could let the
interpreter and LLVM select different storage. Comparing only a truncated hash
would also make a collision silently reinterpret bytes. Source lowering needs
one deterministic bridge from typed declarations to the already accepted MIR
catalog.

## Decision

### Canonical descriptors and compact identities

Every accepted FFI ABI storage type has one canonical UTF-8 JSON descriptor for
an exact target and ABI. Scalar descriptors use this logical schema and key
order:

```json
{"schemaVersion":1,"target":"x86_64-unknown-linux-gnu","abi":"C","abiType":"Int64","size":8,"alignment":8}
```

`abiType` uses the complete canonical Pop FFI ABI spelling. Integer and float
spellings name their exact width or selected C kind. Pointer spellings preserve
mutable/read-only and required/optional distinctions and recursively name their
element ABI type. Function-pointer spellings contain the closed synchronous
parameter and result packs plus ABI. Handle spellings contain `Ffi.Handle` and
the stable managed payload type identity, never a runtime type name.

`Ffi.C.Layout` records retain ADR 0082's ordered record descriptor. A nested
record field uses `layout:<full lowercase fingerprint>` as its `abiType`.
Canonical strings use JSON escaping, decimal integers have no leading zero,
there is no insignificant whitespace, and there is no trailing newline.

The full layout fingerprint is lowercase SHA-256 of those exact bytes.
`FfiAbiLayoutId` is the unsigned big-endian value of the first eight digest
bytes. Zero is invalid. If two unequal full fingerprints in one artifact have
the same compact identity, compilation fails deterministically; it never
renumbers by discovery order. Artifact metadata always carries and compares the
full fingerprint, descriptor facts, and compact identity, so the compact value
is an execution key rather than the sole integrity proof.

Consistent with ADR 0055, the reviewed SHA-256 implementation remains inside
project/artifact/compiler-driver ownership. Portable MIR constructs the exact
canonical descriptor and validates the supplied lowercase fingerprint shape,
compact prefix, nonzero identity, and collision rules, but does not gain a
third-party hashing dependency or select a physical artifact encoding. The
artifact owner computes the digest before finalizing the catalog.

### Trusted source-to-catalog bridge

The trusted `Ffi.C.Layout` compiler attribute is resolved on records before HIR
construction completes. HIR preserves the resolved attribute identity on the
record declaration; spelling alone or a shadowing user attribute has no
authority. The target-selected catalog builder walks the closed ABI types used
by foreign declarations and canonical FFI operations, recursively includes
their dependencies, computes their descriptors, and emits entries ordered by
`FfiAbiLayoutId`.

An attributed record is accepted only when every field has one accepted ABI
storage layout and its target metadata agrees on field name, order, size,
alignment, offset, ABI, target, full fingerprint, and compact identity.
Unannotated records and managed-reference-bearing fields remain rejected.

HIR and MIR operations carry the exact element `TypeId` plus the resulting
`FfiAbiLayoutId`; the catalog separately binds that local semantic identity to
the stable artifact layout. A backend consumes this binding and never hashes a
type, parses an attribute, or recomputes a layout.

### Public layout records across Bubble boundaries

When a public foreign declaration uses a trusted `Ffi.C.Layout` record by
value, `reference.metadata` carries one closed public layout-record projection
in addition to the function contract. The projection retains the producer
`SymbolIdentity`, public Module/namespace/name, declaration-ordered field names
and exact recursive reference types. Only public trusted layout records that
are reachable from public signatures enter this projection; ordinary,
`internal`, and `private` records remain absent.

For each exact compilation target and foreign ABI used by that public surface,
the projection also carries the reachable canonical layout entries: compact
identity, exact target and ABI, semantic type identity, size, alignment,
declaration-order field plan and offsets, canonical descriptor, and full
fingerprint. Scalar child entries are included so the consumer receives a
closed catalog rather than recomputing a producer claim. The reference loader
reconstructs the public record schemas in its isolated type/HIR reference
arena, recomputes and verifies the canonical descriptors and fingerprints, and
rejects an unknown record identity, malformed field plan, target/ABI mismatch,
compact collision, or unequal duplicate before name resolution or backend
entry. A consumer then remaps record and field identities into its session-local
arenas while preserving the producer `SymbolIdentity`; it never merges the
producer Module into the consumer Bubble.

Imported foreign declarations bind their by-value parameter/result layouts
only from that verified reference catalog. The binding is copied into canonical
MIR with collision-free consumer-local `TypeId` and `FieldId` values. Foreign
record marshalling uses each catalog field's verified declaration-order
`source_index` as the managed record slot and its exact offset as the ABI
storage location; it does not require the producer declaration to appear among
the consumer Bubble's declarations. A missing catalog entry or selected-target
mismatch fails lowering rather than causing a backend layout guess.

### Source FFI operations

The compiler recognizes the exact `Ffi.Buffer` operations fixed by ADR 0082
only when the verified `Pop.Ffi` Bubble is a direct dependency and ordinary
name resolution found no user declaration at that path. Type checking applies
the ordinary static generic rules, proves one accepted element layout, and
creates typed HIR operations. HIR-to-MIR lowering obtains the layout only from
the target-selected catalog. Missing, mismatched, or unsupported layout facts
are compile errors; they never become a dynamic fallback or backend guess.

The same rule applies to future source `Ffi.withPin`, callback, and unsafe
pointer operations: their exact typed HIR form must precede MIR and cannot be
selected from spelling after a user declaration resolves.

## Consequences

- ABI layout identities are reproducible across compiler sessions and
  independent of local arena numbering or declaration order.
- Full fingerprints remain the artifact integrity authority while native calls
  use one compact checked key.
- `Ffi.C.Layout` becomes an explicit typed source-to-HIR contract rather than a
  backend-visible annotation.
- Source `Ffi.Buffer` operations can lower into ADR 0084 MIR without inventing
  target geometry in the type checker or backend.

## Alternatives considered

### Use `TypeId` as the runtime layout identity

Rejected because `TypeId` is a local semantic arena identity and is neither a
stable artifact identity nor target ABI metadata.

### Assign consecutive layout identities after sorting declarations

Rejected because adding an unrelated layout could renumber existing artifact
keys and because discovery order must not affect execution metadata.

### Trust the first 64 hash bits without retaining the full fingerprint

Rejected because a compact collision must fail closed rather than reinterpret
foreign storage.

### Let LLVM derive the layout from its data layout

Rejected because MIR governs every backend and the interpreter cannot depend
on LLVM or a host compiler.

## Required conformance tests

- canonical scalar, C integer, pointer, function-pointer, handle, flat record,
  and nested-record descriptor bytes, full fingerprints, and compact IDs;
- stability across declaration order and local `TypeId` allocation order;
- zero compact identity and unequal-full-fingerprint collision rejection;
- trusted `Ffi.C.Layout` attachment, shadowing, wrong-target, unsupported-field,
  unannotated-record, and managed-field rejection;
- public producer-to-consumer layout-record metadata round trips, nested-field
  ordering, exact target/ABI/fingerprint binding, and exclusion of non-public
  records;
- malformed public record identity/type/field plans, compact collisions,
  fingerprint corruption, and selected-target mismatch fail before HIR/MIR or
  backend entry;
- source `Ffi.Buffer` positive and negative typing plus HIR/MIR catalog identity
  preservation;
- catalog/artifact/generated-metadata mismatch failures before backend entry;
- MIR-interpreter and LLVM differential use of the same catalog identity.

## Documents/components affected

Type checking, trusted attributes, HIR declarations and FFI operations, target
layout computation, MIR catalog construction, `.poplib` target metadata,
generated `native-bindings.popc`, the MIR interpreter, LLVM, diagnostics, and
FFI conformance tests.
