# ADR 0066: Lazy Sequence Bounds and Composition

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0032, ADR 0053, ADR 0054, and ADR 0061
- Supersedes: none

## Context

`Pop.Sequence.map` and `filter` establish one-object lazy adapters, while the
terminal operations consume a source directly. Common pipelines still need
handwritten iterator classes to bound a source, discard a prefix, retain a
predicate-controlled prefix, or traverse two sources in order.

ADR 0061 deliberately deferred these operations until their count policy,
single-pass state, evaluation order, failed-predicate consumption, and
allocation costs were explicit. Those decisions do not require equality,
ordering, views, async behavior, or a new language feature.

## Decision

`Pop.Sequence` adds five ordinary portable functions:

```luau
public function take<T, TSource: Iterable<T>>(
    source: TSource,
    count: Int
): Iterator<T>
public function drop<T, TSource: Iterable<T>>(
    source: TSource,
    count: Int
): Iterator<T>
public function takeWhile<T, TSource: Iterable<T>>(
    source: TSource,
    predicate: function(value: T): Boolean
): Iterator<T>
public function dropWhile<T, TSource: Iterable<T>>(
    source: TSource,
    predicate: function(value: T): Boolean
): Iterator<T>
public function concat<
    T,
    TFirst: Iterable<T>,
    TSecond: Iterable<T>
>(first: TFirst, second: TSecond): Iterator<T>
```

`take` yields at most `count` items. `drop` discards at most `count` items and
then yields the rest. A count less than or equal to zero is normalized to zero:
`take` is empty and `drop` preserves the source. This total, nonallocating
normalization avoids a second error channel for a computed bound while keeping
the result deterministic.

`takeWhile` yields items until its predicate first returns false. The failing
item is consumed and discarded, and the adapter remains exhausted permanently;
it never resumes later source items. `dropWhile` discards items while its
predicate returns true, yields the first false item, and then yields all later
items without calling the predicate again.

`concat` yields every remaining item from `first`, followed by every remaining
item from `second`. Adapter construction acquires both iterators in parameter
order, matching existing `source:iterator()` acquisition semantics. It never
interleaves sources and never requests a second-source item before the first is
exhausted.

Each function is lazy, single-pass, and preserves source order. Construction is
O(1) and allocates exactly one adapter-state object, excluding allocation or
dispatch performed by source iterator acquisition and caller-created closures.
Each requested item is O(1) amortized, except that one `drop` or `dropWhile`
request may examine an arbitrary prefix. The adapters use O(1) state and never
materialize source items.

Count decrement uses ordinary checked `Int` subtraction only while the count is
positive, so normalization cannot underflow. Predicates are invoked exactly
once for each examined prefix item and in source order.

The functions are ordinary generic `.pop` implementations. They gain no
bootstrap identity, compiler recognition, HIR/MIR operation, backend lowering,
runtime adapter, or native transition.

## Consequences

- Common bounded and concatenated pipelines remain a few direct calls.
- Negative computed counts have safe, portable zero semantics.
- Predicate evaluation and the consumed failing item are observable and fixed.
- Laziness avoids collection allocation while one adapter allocation remains
  explicit in documentation.
- Eager acquisition of both `concat` iterators is simple but can expose both
  sources' documented acquisition cost at construction time.

## Alternatives considered

### Return `Result` for a negative count

Rejected because a negative bound has an unambiguous empty/no-drop
normalization and is commonly produced by arithmetic. A Result would add a
failure path without protecting memory, authority, or data integrity.

### Trap on a negative count

Rejected because it makes a safe bound computation fatal and provides no
additional static guarantee.

### Resume `takeWhile` after a failed predicate

Rejected because it would no longer describe a prefix and would produce
surprising results across repeated iterator calls.

### Delay acquisition of the second `concat` iterator

Deferred. It needs an optional/state-union field whose representation and
cleanup behavior should not be introduced only to hide a documented iterator
acquisition. The current contract remains lazy in item traversal.

### Add zip, windows, chunks, or sorting now

Rejected because they require tuple ownership, partial-window policy, buffer
reuse, or ordering contracts not needed by this slice.

## Required conformance tests

- empty, unit, and multi-item sources cover every adapter;
- zero, negative, exact, short, and excessive counts have exact results;
- `takeWhile` proves permanent exhaustion after consuming its first failure;
- `dropWhile` proves the predicate stops after its first failure;
- `concat` proves source order and empty-side behavior;
- callbacks and count types are rejected statically when invalid;
- adapters remain lazy and allocate no materialized collection;
- cross-Bubble generic capsules specialize without widening visibility;
- MIR interpreter and LLVM execution agree; and
- architecture tests reject duplicate Rust/compiler/backend implementations.

## Documents/components affected

Sequence source and checked documentation, public catalog and examples,
foundation metadata conformance, MIR interpreter and LLVM tests, closed design
decisions, and the implementation roadmap.
