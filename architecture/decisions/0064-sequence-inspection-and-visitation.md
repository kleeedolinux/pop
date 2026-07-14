# ADR 0064: Sequence Inspection and Visitation

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0032, ADR 0053, ADR 0054, and ADR 0061
- Supersedes: none

## Context

`Pop.Sequence` can transform, filter, collect, fold, count, and test predicates,
but common inspection and visitation still require handwritten loops. Returning
`T?` from an inspection function would repeat the ambiguity already rejected
for iterator steps: a present optional item and an absent item can both appear
as `nil`.

`Iteration<T>` has the right representation for presence, but source-level
exhaustive `match` does not yet accept that reserved compiler-supplied union.
Publishing `first(source): Iteration<T>` before consumers can inspect it would
create an unusable public API and silently expand compiler scope.

## Decision

`Pop.Sequence` adds six ordinary portable functions:

```luau
public function isEmpty<T, TSource: Iterable<T>>(source: TSource): Boolean
public function firstOr<T, TSource: Iterable<T>>(source: TSource, fallback: T): T
public function lastOr<T, TSource: Iterable<T>>(source: TSource, fallback: T): T
public function each<T, TSource: Iterable<T>>(
    source: TSource,
    action: function(value: T)
)
public function none<T, TSource: Iterable<T>>(
    source: TSource,
    predicate: function(value: T): Boolean
): Boolean
public function countWhere<T, TSource: Iterable<T>>(
    source: TSource,
    predicate: function(value: T): Boolean
): Int
```

`isEmpty` consumes at most one item. `firstOr` returns the first item or the
already evaluated fallback and also consumes at most one item. Neither attempts
to rewind a single-pass source. `lastOr` consumes the complete source, retaining
only the latest item, and returns its fallback when empty.

`each` calls `action` exactly once per item from left to right. `none` returns
false at the first matching item and true for an empty source. `countWhere`
consumes the full source, calls its predicate once per item, and increments one
checked `Int` count only for matching items.

All functions allocate no collection or adapter storage of their own and use
O(1) space excluding source-owned iterator state. `isEmpty` and `firstOr` are
O(1) in examined items; `lastOr`, `each`, and `countWhere` are O(n); `none` is
O(n) worst case and short-circuits. Source iterator acquisition, interface
dispatch, cleanup, and allocation retain ADR 0053.

The functions are ordinary generic `.pop` implementations. They gain no
bootstrap identity, compiler name recognition, HIR/MIR operation, runtime
adapter, or backend lowering.

An inspection API returning `Iteration<T>` remains deferred until a focused
language/compiler change makes the reserved union exhaustively matchable in
ordinary source. That work must cover typing, HIR/MIR, diagnostics, portable
capsules, and both primary backends before such a result is public.

## Consequences

- Common inspection works for optional element types because presence is
  selected by traversal, not encoded in `T?`.
- Callers choose fallback values explicitly without an error, trap, or sentinel.
- Common terminal operations remain one direct call with exact traversal cost.
- Single-pass sources remain visibly consuming; no API promises replay.
- Reserved `Iteration<T>` matching remains an explicit compiler roadmap item.

## Alternatives considered

### Return `T?` from `first` and `last`

Rejected because `Iterable<T?>` cannot distinguish a nil item from exhaustion.

### Return `Iteration<T>` immediately

Deferred because ordinary source cannot yet exhaustively match the reserved
union. Implementing that language capability is larger than an ordinary
library-algorithm contribution.

### Return Result for empty input

Rejected for default inspection because emptiness is ordinary state and an
explicit fallback is shorter. A later no-fallback API can use `Iteration<T>`
after the match boundary is implemented.

### Add lazy bounds, windows, and zipping in the same decision

Rejected because those operations introduce new iterator-state layouts,
count/bounds policy, tuple state, and allocation contracts.

## Required conformance tests

- empty, single-item, and multi-item sources cover every exact result;
- `isEmpty` and `firstOr` prove at-most-one stepping;
- `lastOr` preserves source order and consumes the complete source;
- `each` and `countWhere` prove exact left-to-right predicate/action counts;
- `none` proves empty semantics and short-circuiting;
- optional element types remain values rather than absence sentinels;
- invalid action/predicate signatures fail statically without fallback;
- cross-Bubble generic capsules specialize without widening visibility;
- MIR interpreter and LLVM execution agree; and
- architecture tests reject duplicate Rust/compiler/backend implementations.

## Documents/components affected

Sequence source and checked documentation, public catalog and examples,
foundation metadata conformance, MIR interpreter and LLVM tests, closed design
decisions, and the implementation roadmap.
