# ADR 0075: Sequence Projection and Composition

- Status: accepted
- Date: 2026-07-14
- Depends on: ADRs 0031, 0053, 0054, 0061, 0064, 0066, and 0067
- Supersedes: none

## Context

The first `Sequence` slices cover mapping, filtering, bounds, concatenation,
inspection, visitation, and direct integer aggregation. Daily programs still
repeat loops for predicate search, indexed search, projected integer
aggregation, projected extrema, adding one boundary value, and running
accumulation. These operations can be expressed with the already accepted
iterator, closure, generic-class, and checked-`Int` contracts.

Equality-based search, tuple-bearing adapters, sorting, and generic numeric
algorithms still depend on unresolved protocols or representations and are not
part of this slice.

## Decision

`Pop.Sequence` adds these portable prototypes:

```luau
public function findOr<T, TSource: Iterable<T>>(source: TSource, predicate: function(T): Boolean, fallback: T): T
public function indexOr<T, TSource: Iterable<T>>(source: TSource, predicate: function(T): Boolean, fallback: Int): Int
public function sumBy<T, TSource: Iterable<T>>(source: TSource, select: function(T): Int): Int
public function productBy<T, TSource: Iterable<T>>(source: TSource, select: function(T): Int): Int
public function minByOr<T, TSource: Iterable<T>>(source: TSource, select: function(T): Int, fallback: T): T
public function maxByOr<T, TSource: Iterable<T>>(source: TSource, select: function(T): Int, fallback: T): T
public function append<T, TSource: Iterable<T>>(source: TSource, value: T): Iterator<T>
public function prepend<T, TSource: Iterable<T>>(source: TSource, value: T): Iterator<T>
public function scan<T, TSource: Iterable<T>, TState>(source: TSource, initial: TState, combine: function(TState, T): TState): Iterator<TState>
```

`findOr` and `indexOr` stop at the first matching item. Indexes are one-based,
matching arrays, lists, and ranges; the fallback is returned only when no item
matches. `sumBy` and `productBy` project and combine left to right with checked
`Int` arithmetic and empty identities zero and one.

`minByOr` and `maxByOr` evaluate the projection exactly once per visited item,
return the first item with the selected extreme key, and use the fallback only
for an empty source. They do not compare or project the fallback.

`append`, `prepend`, and `scan` are lazy single-pass iterators. Each allocates
one typed iterator state object and no materialized collection. `scan` yields
one state after each source item and does not yield the initial state by itself.
All operations preserve source order, do not suspend, perform no native
transition, and use only statically resolved iterator/function calls.

The new rows are outside the prelude and remain `prototype` until the ADR 0058
evidence gate is complete.

## Consequences

- Frequent search and projection pipelines become one direct call.
- Advanced callers retain lazy iteration and exact allocation behavior.
- Checked overflow and evaluation order remain backend-neutral.
- Equality protocols, generic arithmetic, tuple adapters, sorting, grouping,
  buffering, and parallelism remain separate slices.

## Alternatives considered

### Add one universal query object

Rejected because it would obscure allocation and dispatch behind an object
graph instead of preserving direct functions and iterators.

### Return optional values from only these search functions

Deferred until no-fallback inspection has one accepted representation. The
existing explicit-fallback convention remains consistent.

### Implement projections through composed map and aggregate calls

Rejected for terminal operations because a fused single pass avoids allocating
an intermediate iterator state and states exact projection evaluation.

## Required conformance tests

- empty, singleton, found/not-found, first-match, and one-based-index behavior;
- projection call counts and first-tie preservation;
- checked sum/product overflow in MIR interpreter and LLVM;
- append/prepend/scan order, laziness, and repeated `next` exhaustion;
- generic String and record items prove the APIs are not integer-only except for
  projection keys/results;
- callback/type mismatches fail statically;
- API baseline identities append without changing the prelude;
- complete checked XML documentation includes cost/allocation contracts; and
- architecture tests reject Rust duplicates, native adapters, compiler-known
  IDs, dynamic fallback, or eager materialization.

## Documents/components affected

Sequence source, API baseline, core catalog, examples, contributor inventory,
foundation tests, cross-Bubble metadata tests, MIR/LLVM execution, architecture
conformance, and the roadmap.
