# ADR 0034: Fixed Array Construction and Optimization

- Status: accepted
- Date: 2026-07-12
- Depends on: ADR 0001, ADR 0003, ADR 0008, ADR 0022, ADR 0024, ADR 0032

## Context

Pop Lang already has invariant typed arrays, literals, optional indexed reads,
checked indexed writes, canonical MIR collection operations, and precise array
pointer maps. Source code cannot yet allocate a large initialized array, query
its length, or request a trapping non-optional read. Consequently ordinary
numeric array loops either cannot be expressed or cross the bootstrap handle
ABI for every element.

## Decision

`Array<T>` is a fixed-length mutable collection with contiguous homogeneous
storage. It is distinct from tables, lists, bytes, records, and classes. Indexes
are one-based.

The first complete core surface is:

```luau
local values = Array.create<<Int>>(count, 0)
local count = Array.length(values)
local optional = values[index]
local value = Array.get(values, index)
values[index] = value
Array.fill(values, 0)
```

- `Array.create<<T>>(length, initialValue)` evaluates its arguments once,
  rejects a negative length with `BoundsViolation`, allocates one `Array<T>`,
  and initializes every element to `initialValue`.
- `Array.length(array)` returns the fixed length as `Int` in O(1).
- `array[index]` returns `T?`; an out-of-bounds read returns `nil`.
- `Array.get(array, index)` returns `T`; an out-of-bounds read traps with
  `BoundsViolation`.
- indexed assignment traps with `BoundsViolation` when out of bounds.
- `Array.fill(array, value)` replaces every element and returns no value.
- zero-length arrays are valid; arrays never grow implicitly.

Construction allocates O(length) storage and initializes in O(length) time.
Length is O(1); get and set are O(1); fill is O(length). Scalar operations do
not allocate. Managed-element initialization, fill, and writes preserve precise
pointer maps and required SATB/generational barriers.

HIR and MIR retain typed backend-neutral array construction, length, optional
get, checked get, set, and fill operations. MIR makes allocation, bounds traps,
managed mutation, and GC effects explicit.

Portable optimization may remove bounds checks only with a proof. A backend may
scalar-replace or use private native storage for a non-escaping scalar array,
and may use a scoped pin for direct contiguous access, provided identity,
initialization, traps, safe points, roots, and cleanup remain equivalent. Raw
managed pointers never enter HIR, MIR, PLRI values, or public source.

## Consequences

- Large numeric arrays become expressible without generated literals.
- The LLVM backend can produce direct counted memory loops while the MIR
  interpreter and future VM retain the same semantic operations.
- Managed arrays cannot use scalar fast paths that omit barriers.
- General collection algorithms remain `Sequence` operations; this decision
  does not turn `Array` into a utility namespace or a growable list.

## Alternatives considered

### Grow arrays on indexed assignment

Rejected because it conflates arrays with lists/tables and makes bounds and
allocation costs implicit.

### Default-initialize without an explicit value

Rejected because it introduces a new default-value contract for every `T` and
risks partially initialized managed storage.

### Expose raw managed pointers publicly

Rejected because the moving nursery requires handles or scoped pins and because
raw pointers would couple source/MIR to one backend representation.

## Required conformance tests

- static generic construction, invariant element typing, and negative lengths;
- zero/nonzero length, optional and checked reads, checked writes, and fill;
- exact evaluation order and one-based bounds behavior;
- scalar and managed pointer-map/barrier behavior;
- HIR/MIR verification and text round trips for every operation;
- MIR-interpreter/native differential results and traps;
- optimization negatives for escaping, managed, and unproved-index arrays;
- optimized LLVM proving direct scalar memory access and removed proven bounds
  checks without removing required loop safe points.

## Documents/components affected

Language model, HIR, MIR, `Pop.Internal`, `Pop.Standard`, PLRI, bootstrap runtime,
MIR interpreter, LLVM backend, optimizer, documentation, and conformance tests.
