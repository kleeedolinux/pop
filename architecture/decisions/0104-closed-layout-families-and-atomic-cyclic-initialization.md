# ADR 0104: Closed Layout Families and Atomic Cyclic Initialization

- Status: accepted
- Date: 2026-07-26
- Depends on: ADR 0072, ADR 0080, ADR 0100, ADR 0101, and ADR 0102
- Supersedes: the remaining per-allocation pointer-map construction for
  self-recursive closures and native iterator results

## Context

ADR 0100 removes repeated pointer-map construction for ordinary compiler-owned
fixed-layout allocations. Two gaps remain.

First, a self-recursive closure cannot put its final self token in the ordinary
initializer array before allocation returns. LLVM therefore used a generic
mapped allocation and separate field stores, rebuilding the known map and
exposing partially initialized cyclic state to mutation-barrier machinery.

Second, runtime-sized homogeneous arrays and interleaved tables cannot have one
finite per-length reference-slot array emitted by the compiler. Native iterator
results and table-entry tuples also used small per-call vectors even though
their patterns belong to a closed family.

## Decision

`ObjectMap` has four closed immutable representations:

- sparse canonical slots for genuinely irregular fixed layouts;
- scalar, with no reference slots;
- homogeneous references; and
- strided references with a fixed first slot and nonzero stride.

Homogeneous and strided maps retain constant-size physical metadata regardless
of logical length. Membership, logical reference count, and ordered iteration
are computed from the formula. No per-element bitmap or slot vector is
materialized.

Compiler-owned self-recursive closure environments retain their ordinary
immutable allocation-site descriptor. LLVM additionally emits one immutable
canonical self-slot constant for that same site and calls the additive ABI
1.22 entry
`pop_rt_allocate_initialized_self_referential_object_at_site`. The initializer
contains zero placeholders at the declared self slots. Under one runtime
allocation authority, the collector validates those slots, allocates and
initializes the private object, installs exact self edges, and only then
publishes the token. No generic field store, mutation barrier, pointer-map
rebuild, or observable partial cycle occurs.

ABI 1.22 also includes the closed `pop_rt_iteration_make` constructor for
native collection iteration. It selects only the statically known scalar or
single-payload-reference layout, validates the closed item/end status, and
atomically initializes the two-slot `Iteration<T>` representation. It is not a
general layout tag or dynamic object facility.

Runtime-sized arrays, interleaved tables, and table-entry tuples select their
closed formula map directly. Fixed trusted runtime carriers may use canonical
scalar or fixed sparse maps. Compiler-owned irregular Pop shapes continue to
require ADR 0100 allocation-site descriptors; runtime layout families do not
authorize an unchecked fallback for them.

## Consequences

- Self-recursive closures use the same site identity and page-shared layout as
  other closures.
- Cyclic initialization is atomic and barrier-free while unpublished.
- Homogeneous arrays and interleaved tables use constant-size map metadata.
- Native iteration no longer allocates and sorts a reference-slot vector or
  performs separate initialization stores.
- ABI 1.22 is additive. Earlier ABI 1 descriptors remain exact and production
  ABI 2 retains the same semantic operations.

## Alternatives considered

### Publish the closure and set its self edge afterward

Rejected because partial cyclic state could escape and would require ordinary
SATB/generational barriers.

### Materialize a bitmap for every array or table length

Rejected because the compiler/runtime already knows a constant-size formula.

### Add a universal runtime layout tag

Rejected because it would become a dynamic construction escape. The iterator
entry is one closed nominal representation, and Pop-owned irregular layouts
still use exact site descriptors.

## Required conformance tests

- malformed, duplicate, scalar, out-of-bounds, or nonzero bootstrap self slots
  fail before publication;
- a valid self allocation exposes its scalar payload and exact self edge;
- LLVM emits one static descriptor and self-slot constant, calls ABI 1.22, and
  emits neither generic mapped allocation nor separate self field stores;
- homogeneous array and interleaved table maps retain constant physical
  metadata while logical membership/count/iteration remain exact;
- native iterator construction uses the closed atomic ABI entry and no generic
  two-slot mapped allocation;
- table-entry tuple maps use scalar, homogeneous, or strided formulas without a
  per-entry vector; and
- paired heap performance evidence remains within ADR 0099 budgets.

## Documents/components affected

Allocation and runtime ABI architecture, PLRI object maps, native ABI version
and symbols, allocation-site validation, collector atomic initialization,
LLVM descriptor/lowering code, native iteration/table adapters, conformance
tests, roadmap, and benchmark evidence.
