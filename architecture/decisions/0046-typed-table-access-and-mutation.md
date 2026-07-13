# ADR 0046: Typed Table Access and Mutation

- Status: accepted
- Date: 2026-07-13
- Depends on: ADR 0001, ADR 0002, ADR 0003, ADR 0008, ADR 0022, ADR 0024

## Context

Pop Lang defines tables as invariant statically typed associative collections
and already supports typed table literals. The architecture does not yet define
lookup, missing-key behavior, mutation, growth, or deterministic iteration
order. Those rules must be closed before source indexing becomes stable.

## Decision

For `Table<K, V>`, ordinary indexing and assignment use Luau-shaped punctuation
while preserving exact static types:

```luau
local scores: {[String]: Int} = { alice = 10 }
local score: Int? = scores["alice"]
scores["bruno"] = 12
```

- `table[key]` requires exactly `K` and returns `V?`; a missing key returns
  `nil`.
- `table[key] = value` requires exactly `K` and `V`, inserts a missing key, and
  replaces the value for an existing key.
- The table, key, and value expressions each evaluate exactly once in that
  order.
- Insertion order is deterministic. Replacing an existing value does not move
  its key. The later `Iterable` surface may expose this order without making a
  table structurally equal to another table.
- The initial key set is the closed set of statically typed values with accepted
  canonical equality and hashing. The compiler rejects a key type until both
  contracts exist; it never falls back to host or runtime-dynamic equality.
- Assigning `nil` is not deletion. Optional values store `nil` as an ordinary
  typed value, so deletion requires a future explicit table operation.

Tables grow when insertion requires capacity. Growth preserves table identity,
existing entries, insertion order, precise object maps, roots, and required
SATB/generational barriers. Allocation failure follows the ordinary typed PLRI
panic path. Arrays remain fixed-length under ADR 0034; growable sequential
collections are `List<T>` rather than arrays with hidden resize behavior.
The bootstrap native ABI advances to version 1.6 with closed typed table get and
set operations.

HIR retains typed table get/set operations. MIR exposes backend-neutral
`tableGet` and `tableSet` operations with explicit key/value types, optional
result type, allocation effects, and managed-reference maps. Backends consume
those operations without reconstructing source lookup or using string member
resolution. Compile-time evaluation remains immutable and rejects table
construction or mutation.

## Consequences

- Missing keys are handled through ordinary optional narrowing rather than
  sentinel values or exceptions.
- Mutation can add entries without changing the invariant table type.
- Deterministic order makes interpreter/native differential testing possible.
- The runtime must support precise table storage growth behind stable managed
  identity.

## Alternatives considered

### Grow arrays on indexed assignment

Rejected by ADR 0034 because it hides allocation and conflates arrays with
lists and tables.

### Return a default value for a missing key

Rejected because not every `V` has a default and a sentinel would erase the
distinction between absence and a stored value.

### Delete entries by assigning `nil`

Rejected because `V?` may legitimately contain `nil` and assignment must retain
one exact static value type.

### Use host-language hashing or dynamic equality

Rejected because backend behavior could diverge and unsupported key types would
gain an operational dynamic escape hatch.

## Required conformance tests

- exact key/value typing and optional lookup results;
- missing, present, replacement, insertion, and stored-optional behavior;
- table/key/value once-only evaluation and deterministic insertion order;
- rejection of unsupported key types and incompatible reads/writes;
- verified HIR/MIR construction and text round trips;
- precise managed key/value maps, growth roots, and write barriers;
- MIR interpreter, optimized MIR, and native LLVM differential results;
- C-backend fail-closed behavior until its declared capability set expands;
- regression proof that arrays remain fixed-length and tables do not become
  records, classes, modules, or runtime name dictionaries.

## Documents/components affected

Language model, type system, HIR, MIR, PLRI, bootstrap runtime, MIR interpreter,
LLVM backend, optimizer, diagnostics, and conformance tests.
