# ADR 0061: First Sequence Aggregates

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0032, ADR 0053, and ADR 0054
- Supersedes: none

## Context

ADR 0053 establishes the nominal iteration protocol and the first four
`Sequence` functions: lazy `map` and `filter`, eager `fold`, and materializing
`collect`. The public catalog also reserves searching and aggregation, but does
not yet define which additional operations can become stable without equality,
ordering, optional-element ambiguity, or new iterator-state types.

The initial foundation needs useful terminal operations that exercise ordinary
portable generic Pop code without adding a compiler-known function, runtime
adapter, hidden collection, or native transition.

## Decision

`Pop.Sequence` adds these three public functions:

```luau
Sequence.any(source, predicate)
Sequence.all(source, predicate)
Sequence.count(source)
```

Their exact generic signatures use the existing `Iterable<T>` bound:

```luau
public function any<T, TSource: Iterable<T>>(
    source: TSource,
    predicate: function(value: T): Boolean
): Boolean

public function all<T, TSource: Iterable<T>>(
    source: TSource,
    predicate: function(value: T): Boolean
): Boolean

public function count<T, TSource: Iterable<T>>(source: TSource): Int
```

`any` consumes from left to right and returns at the first item for which the
predicate is true. It returns false for an empty source. `all` consumes from
left to right and returns at the first item for which the predicate is false.
It returns true for an empty source. Each predicate is called exactly once for
every item examined and never for an item after the result is known.

`count` consumes the complete source and returns its item count as `Int`. Its
increment uses ordinary checked `Int` addition, so an unrepresentable count
raises the existing `IntegerOverflow` trap rather than wrapping or widening.

All three operations are eager, single-pass, preserve the source's iteration
order, allocate no collection or iterator adapter of their own, and perform no
native/runtime transition beyond operations already required by the source
iterator. Acquiring an iterator remains governed by ADR 0053 and may expose the
source's documented allocation or interface-dispatch cost.

`find` is not part of this decision because `T?` cannot distinguish a missing
item from a present `nil` when `T` is optional. `contains` and sorting wait for
the equality/ordering contracts. `take`, `drop`, `zip`, windows, and chunks wait
for their exact lazy-state, bounds, and allocation contracts.

The functions are ordinary `.pop` implementations. Their names do not receive
bootstrap IDs, HIR/MIR operations, backend lowering, native adapters, or
compiler recognition.

## Consequences

- Common existential, universal, and cardinality queries need one direct call.
- Short-circuit behavior is observable and portable across backends.
- These aggregates add no hidden materialization or adapter allocation.
- Search and equality APIs are not forced into an ambiguous optional contract.
- Counting a source larger than `Int` can represent traps deterministically.

## Alternatives considered

### Express every query through `fold`

Rejected because `fold` cannot short-circuit. Requiring it for existential and
universal queries would execute unnecessary user code and make the common call
less clear.

### Return `T?` from `find`

Rejected because an iterator can contain an optional item. `nil` cannot encode
both a present optional item and exhaustion; this is the same ambiguity that
ADR 0053 avoids with `Iteration<T>`.

### Add all planned sequence algorithms at once

Rejected because equality, ordering, bounds, owned/view storage, and adapter
allocation are separate public contracts. Catalog placement does not authorize
their signatures or costs.

### Implement aggregates in Rust

Rejected because these are portable algorithms over an accepted static
protocol. A Rust body would duplicate semantics, prevent normal source-level
specialization, and add no required capability.

## Required conformance tests

- empty, single-item, and multi-item sources cover exact results;
- `any` and `all` prove left-to-right short-circuiting and exact predicate call
  counts;
- `count` covers empty and nonempty sources and retains checked `Int` addition;
- inferred and explicit generic calls use the exact `Iterable<T>` contract;
- invalid sources and predicates fail statically without dynamic fallback;
- reference metadata and portable specialization capsules include all three
  ordinary public functions without bootstrap identities;
- MIR interpreter and LLVM execution produce equal results; and
- architecture tests reject a duplicate Rust aggregate implementation,
  compiler-name recognition, backend operation, or native adapter.

## Documents/components affected

Sequence source and tests, standard-library catalog and examples, foundation
source conformance, cross-backend execution tests, closed design decisions, and
the implementation roadmap.
