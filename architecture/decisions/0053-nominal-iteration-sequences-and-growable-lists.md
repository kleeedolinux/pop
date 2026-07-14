# ADR 0053: Nominal Iteration, Lazy Sequences, and Growable Lists

- Status: accepted
- Date: 2026-07-13
- Supersedes: the generalized-iteration deferral in ADR 0042 and the
  allocation-free adapter claim in the initial `Sequence` catalog

## Context

ADR 0042 deliberately limits the first `for` form to numeric ranges because a
generalized loop cannot safely guess iterator member names, end sentinels,
disposal, multi-binding, or specialization policy. The public architecture
already reserves `Iterable<T>`, `Iterator<T>`, `Sequence`, and the growable
`List<T>` name, but it does not define their exact contracts.

Using `T?` as an iterator step would make an optional element indistinguishable
from the end of iteration. Copying Lua's iterator triples would reintroduce a
dynamic calling convention and untyped multiple results. Treating arrays as
growable would contradict ADR 0034.

## Decision

### Closed protocol identities

`Pop.Standard` supplies three reserved generic nominal identities in the
curated prelude:

```luau
public union Iteration<T>
    Item(value: T)
    End
end

public interface Iterable<T>
    function iterator(): Iterator<T>
end

public interface Iterator<T>
    function iterator(): Iterator<T>
    function next(): Iteration<T>
end
```

`Iteration<T>` is distinct from `T?`, `Result`, a tuple, and a Boolean sentinel.
Its exact public cases are `Iteration.Item(value)` and `Iteration.End`.
`Iterator<T>` has a fixed nominal refinement relationship to `Iterable<T>`;
its `iterator()` method returns the same iteration session. This fixed relation
does not introduce structural interface conformance or general implicit
interface inheritance.

Every protocol use resolves the reserved `BuiltinTypeId`, exact `T`, stable
method slot, and either a concrete implementation or a verified nominal
interface dispatch. There is no string lookup for `iterator` or `next`, no
metamethod, no truthy sentinel, and no dynamic fallback. User types participate
only through an explicit, statically verified implementation of the exact
generic interface instance.

### Generalized `for`

The generalized source form is:

```luau
for value in values do
    consume(value)
end
```

The source expression is evaluated exactly once. Its static type must implement
one exact `Iterable<T>` or `Iterator<T>` instance. An iterable's `iterator()` is
called once before the loop. Each iteration calls `next()` exactly once and
exhaustively distinguishes `Iteration.Item` from `Iteration.End`.

One binding receives `T`. Multiple Luau-shaped bindings are allowed only when
`T` is a fixed tuple with exactly the same arity:

```luau
for key, value in entries do
    consume(key, value)
end
```

They destructure that one tuple item; they are not dynamic multiple results.
Bindings are immutable, body-local, and freshly defined for each item.
Evaluation, assignment, capture, `break`, `continue`, `return`, result
propagation, and panic cleanup retain the existing deterministic rules.

Generalized `for` performs no implicit `close`, disposal, finalization, or
resource ownership transfer. The loop owns only the iteration session it
obtains. Resource-backed iteration must place the resource in an explicit
lexical scope and use its documented `defer` cleanup. Breaking or unwinding a
loop therefore cannot select a hidden protocol or allocate a cleanup callback.
Suspending/async iteration is a separate structured-concurrency contract.

Generic code may use generalized iteration only when its source type has a
statically proven `Iterable<T>`/`Iterator<T>` constraint. The concrete
constraint syntax and portable cross-Bubble encoding are delivered by the
generic-boundary roadmap slice; absence of that encoding never permits dynamic
protocol lookup.

### Built-in collection order

The following trusted implementations have fixed observable order:

- `Array<T>` yields elements from index one through its fixed length;
- `List<T>` yields elements from index one through its current length;
- `Table<K, V>` yields `(K, V)` tuples in ADR 0046 insertion order;
- `Range<TInteger>` yields its accepted inclusive numeric progression; and
- an `Iterator<T>` yields its remaining items from its current single-pass
  state.

Mutation that changes a collection's length or key set while its iterator is
active is rejected statically when alias/effect analysis can prove it and
otherwise raises the closed `ConcurrentModification` trap at the next iterator
operation. Replacing an existing element/value without changing length/order is
visible to subsequent items. This rule is identical across backends.

### Growable `List<T>`

`List<T>` is an invariant, one-based, mutable sequential collection with stable
managed identity. It is distinct from fixed `Array<T>`, tables, sets, and
iterator adapters. Its first-release core surface is:

```luau
local values = List.create<<Int>>()
local reserved = List.withCapacity<<Int>>(count)
List.add(values, 42)
local count = List.length(values)
local optional = values[index]
local value = List.get(values, index)
values[index] = value
```

- `create` allocates an empty list.
- `withCapacity` rejects a negative capacity with `BoundsViolation`, allocates
  an empty list, and reserves at least that many element slots.
- `add` appends one value, growing storage when required while preserving list
  identity, order, precise roots, and barriers.
- `length` is O(1).
- ordinary indexing returns `T?`; `List.get` and indexed replacement trap with
  `BoundsViolation`; replacement never grows the list.

Capacity and growth factors are private. Capacity is not semantic identity and
is not exposed by the initial API. Allocation failure follows the existing
panic/unwind contract. Arrays never grow as a consequence of this decision.

### `Sequence` adapters

The first deterministic namespace functions are:

```luau
Sequence.map(source, transform)
Sequence.filter(source, predicate)
Sequence.fold(source, initial, combine)
Sequence.collect(source)
```

They infer exact generic arguments from the nominal source and callable
signatures; ambiguity is a static error. `map` and `filter` return lazy,
single-pass `Iterator` adapters. They preserve source order, call user code at
most once per examined item, and do not materialize an output collection.
`fold` consumes eagerly from left to right and shortens no calls. `collect`
materializes into a new `List<T>` in source order.

Creating a lazy adapter may allocate one small typed iterator-state object in
the initial class-based implementation. That allocation is part of the checked
documentation/cost contract; it is not hidden collection materialization.
Portable optimization may stack-promote or scalar-replace a nonescaping adapter
without changing identity, effects, cleanup, or safe points. Ordinary adapter
algorithms ultimately live in `.pop` Modules; the compiler recognizes only the
nominal protocol identities and generalized-loop construct, not function names
such as `Sequence.map`.

### HIR, MIR, and backends

HIR retains a typed generalized-loop node with exact item type, binding shape,
reserved protocol identities, and resolved callable dispatch. Canonical MIR
lowers it to ordinary statically identified calls, `Iteration` discriminant
tests, dominated payload projection, block arguments, branches, cleanup edges,
and a safe point on every backedge. A reserved built-in-interface call carries
the exact `BuiltinTypeId` and stable protocol method ID because reserved
`Pop.Standard` interfaces are not user-declared `InterfaceId` values; these ID
domains must not be collapsed. MIR adds no string-based iterator operation and
does not ask a backend to reconstruct the source protocol.

`List<T>` uses distinct backend-neutral typed HIR/MIR operations for create,
reserve construction, length, optional/checked get, replace, and append. Growth
effects, allocation failure, root liveness, and managed write barriers are
explicit. The interpreter, optimized MIR, and LLVM preserve identical order,
mutation detection, traps, and cleanup. The experimental C backend rejects
unsupported protocol/list operations explicitly.

The bootstrap native ABI 1.9 exposes statically selected List create, length,
optional/checked get, checked replace, and append adapters. Creation receives a
nonnegative reserved capacity and the exact homogeneous element-reference map.
Reads use a status plus an out payload so zero-valued scalar elements remain
distinct from absence or failure. Capacity and storage addresses never enter
MIR or source-visible identity; growth preserves the managed List handle and
performs the required precise barriers.

## Consequences

- Optional values can be iterated without conflating `nil` with exhaustion.
- Generalized loops remain Luau-shaped while using one closed static protocol.
- Tuple multi-binding stays exact and does not revive dynamic variadics.
- Resource cleanup remains lexical and visible rather than becoming iterator
  magic.
- Arrays stay fixed while `List<T>` owns sequential growth and its costs.
- Lazy adapter allocation is documented honestly and can be optimized without
  promising an unimplemented zero-allocation object model.

## Alternatives considered

### Use `T?` as the iterator step

Rejected because `Iterator<T?>` could not distinguish an `Item(nil)` from end.

### Use Lua iterator triples

Rejected because generic callable/state/control triples depend on a dynamic
multiple-result convention and implicit call protocol.

### Implicitly close iterators

Rejected because not every iterator owns a resource and hidden disposal would
conflict with lexical `defer`, panic cleanup, and explicit ownership.

### Grow arrays

Rejected by ADR 0034 because fixed length, bounds proofs, and contiguous-array
costs are separate from growable-list semantics.

### Promise allocation-free class-based adapters

Rejected because it would make the documented cost contract false. Nonescaping
optimization remains permitted after semantic correctness is established.

## Required conformance tests

- stable protocol/step identities, exact generic arity, case/method slots, and
  no dynamic lookup;
- parser/type tests for one expression, one/tuple bindings, immutable scope,
  exact protocol implementation, optional items, and malformed/dynamic forms;
- once-only iterable acquisition and `next`, empty/single/multiple iteration,
  ordering, nested loops, loop control, and lexical cleanup;
- explicit no-disposal behavior and resource-backed `defer` examples;
- array/list/table/range order plus structural-mutation detection;
- list create/reserve/append/growth/index/replace/bounds/root/barrier behavior;
- lazy map/filter order and call counts, eager fold, materializing collect, and
  documented adapter allocation;
- HIR/MIR verifier negatives, text round trips, optimizer preservation, precise
  safe points/roots, and interpreter/LLVM differential tests; and
- explicit experimental-C rejection plus regression scans for `Iter`, dynamic
  member calls, Lua iterator triples, and implicit array growth.

## Documents/components affected

Language model, syntax/nomenclature, closed questions, type system, bootstrap
metadata, standard-library architecture/catalog/examples, HIR, MIR, runtime
collections, interpreter, LLVM, C capability validation, diagnostics,
documentation, and conformance matrices.
