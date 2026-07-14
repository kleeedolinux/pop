# ADR 0076: Sequence Index, Last Match, and Reduction

- Status: accepted
- Date: 2026-07-14
- Depends on: ADRs 0031, 0053, 0054, 0061, 0064, and 0075
- Supersedes: none

## Context

The executable `Sequence` prototype covers first/last fallback inspection,
first-match search, projection, aggregation, and lazy composition. Daily code
still needs handwritten loops to select an item by position, retain the last
predicate match, retain its index, or reduce a non-empty sequence from its
first item.

These operations need no new equality, ordering, optional, tuple, storage,
runtime, or backend contract. They can preserve the existing explicit-fallback
style until `Iteration<T>` is exhaustively usable in ordinary source.

## Decision

`Pop.Sequence` adds four portable prototypes:

```luau
public function elementAtOr<T, TSource: Iterable<T>>(source: TSource, index: Int, fallback: T): T
public function findLastOr<T, TSource: Iterable<T>>(source: TSource, predicate: function(T): Boolean, fallback: T): T
public function indexLastOr<T, TSource: Iterable<T>>(source: TSource, predicate: function(T): Boolean, fallback: Int): Int
public function reduceOr<T, TSource: Iterable<T>>(source: TSource, combine: function(T, T): T, fallback: T): T
```

Indexes are one-based. `elementAtOr` stops at the requested item. A nonpositive
index returns the fallback without acquiring or traversing the source.

`findLastOr` and `indexLastOr` traverse the source once and evaluate the
predicate exactly once for every item, retaining the last match. `indexLastOr`
uses checked `Int` indexing and traps on overflow through the existing integer
contract. Neither function applies the predicate to its fallback.

`reduceOr` returns the fallback for an empty source. Otherwise the first item
becomes the initial state and `combine` is called left to right exactly once for
each later item. The fallback is not combined with a non-empty source.

All four functions are eager, single-pass, allocate no storage of their own,
do not suspend, perform no native transition, and use only statically resolved
iteration and function calls. They remain outside the prelude with `prototype`
status.

## Consequences

- Indexed lookup, last-match search, and first-item reduction become direct
  calls without materialization.
- Callback counts, traversal order, short-circuiting, and fallback use are
  deterministic and backend-neutral.
- No-fallback forms remain gated on exhaustive source use of `Iteration<T>`.
- Equality search, tuple-bearing adapters, buffering, sorting, and parallelism
  remain separate work.

## Alternatives considered

### Add optional-returning forms now

Rejected for this slice because ordinary source cannot yet exhaustively consume
the reserved `Iteration<T>` union. A parallel optional convention would
duplicate the accepted fallback model.

### Implement indexed lookup through `drop` and `firstOr`

Rejected because a fused terminal traversal avoids allocating the lazy `drop`
adapter and can return without acquiring the source for invalid indexes.

### Use `fold` for first-item reduction

Rejected because `fold` requires a caller-provided initial state. Combining the
fallback into non-empty input would change common reduction semantics.

## Required conformance tests

- empty, singleton, in-range, out-of-range, and nonpositive index behavior;
- nonpositive indexes do not acquire or traverse the source;
- first/last/multiple/no-match behavior and exact predicate call counts;
- empty, singleton, ordered multi-item reduction and exact `n - 1` combine
  calls;
- generic `String` and record items;
- callback and argument mismatches fail statically;
- API identities append without changing the prelude;
- MIR interpreter and native LLVM execution agree; and
- architecture tests reject Rust duplicates, native adapters, dynamic
  fallback, materialization, or compiler-known IDs.

## Documents/components affected

Sequence source, API baseline, core catalog, examples, closed decisions,
foundation tests, MIR/LLVM execution tests, architecture conformance, and the
roadmap.
