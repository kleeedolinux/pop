# ADR 0071: Atomic Initialized Object Allocation

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0022, ADR 0024, ADR 0038, ADR 0039, and ADR 0070
- Supersedes: none

## Context

MIR already represents record and class construction as one typed operation with
an exact object map and a complete ordered field initializer set. LLVM currently
lowers that operation to one native allocation followed by a separate native
field call for every initialized slot. Each call reacquires the native runtime
lock and repeats managed-token and object-map lookup.

The sequence is semantically weaker and more expensive than the accepted GC
architecture. That architecture requires pointer fields to be initialized
before publication, permits barriers to be eliminated for first initialization
of an unpublished object, and forbids returning partially initialized objects.

## Decision

Native ABI 1 advances from version 1.10 to 1.11 with the closed
`AllocateObjectInitialized` PLRI operation and
`pop_rt_allocate_initialized_object` native spelling.

The native entry receives:

- the exact logical slot count;
- a pointer to the compiler-proven zero-based managed-reference slots plus their
  count; and
- a pointer to exactly one physical `u64` initializer per logical slot plus the
  value count.

The operation validates all lengths, pointer-map entries, and non-null managed
initializer tokens before heap publication. It constructs the complete typed
payload, places the object, installs ownership and marking metadata, and only
then returns its nonzero managed token. Failure returns the existing zero
allocation sentinel and exposes no partial object.

LLVM uses this operation for verified `RecordMake` and `ClassMake` instructions.
It evaluates every source initializer exactly once in the existing MIR order,
stores the class identity in the reserved class slot, and passes the complete
physical payload in logical slot order. It emits no post-allocation `FieldSet`
calls for those initializers. Ordinary later field mutation retains the existing
typed store and barrier path.

MIR is unchanged. Other backends implement the same atomic construction
semantics through their own representation and do not depend on the native C
spelling.

## Consequences

- New objects cannot become visible between allocation and initialization.
- First initialization needs one native transition and no ordinary mutation
  barrier per field.
- Managed references in the initial payload remain precisely described and are
  visible to tracing when the allocation is published.
- Object identity, heap retention, field access, and later mutation are
  unchanged; this does not scalar-replace escaping objects.
- Native ABI 1.10 archives remain link-compatible with already-generated 1.10
  code. Newly generated 1.11 code requires a 1.11 native archive.

## Alternatives considered

### Scalar-replace the retained object graph

Rejected because escaping objects retain observable managed identity and the
retained-object benchmark is intended to exercise real managed storage.

### Keep separate allocation and field calls

Rejected because it contradicts the accepted pre-publication initialization
model and multiplies native transitions, lookups, and barriers.

### Expose raw object payload pointers to LLVM

Rejected because managed storage addresses cannot escape through PLRI and would
violate relocation, safe-point, and precise-barrier contracts.

## Required conformance tests

- ABI 1.11 maps `AllocateObjectInitialized` to one unique native symbol;
- the native entry rejects mismatched lengths, invalid pointer maps, null input
  pointers, and invalid managed initializer tokens without returning an object;
- scalar and managed initializers are readable through ordinary typed field
  access after construction;
- LLVM record/class construction emits one initialized-allocation call and no
  initializer `FieldSet` calls;
- executable LLVM coverage preserves class identity, field values, retained
  array references, and the exact workload checksum; and
- later field mutation continues to use the ordinary checked store path.

## Documents/components affected

Runtime and ABI architecture, PLRI operation vocabulary, native ABI version and
symbol map, collector allocation construction, native exports, LLVM lowering,
native/LLVM conformance tests, implementation roadmaps, and retained-object
benchmark documentation.
