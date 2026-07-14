# ADR 0067: Integer Sequence Aggregates

- Status: accepted
- Date: 2026-07-14
- Depends on: ADRs 0040, 0053, 0054, 0058, and 0061
- Supersedes: none

## Context

`Sequence.fold` can express numeric aggregation, but summing, multiplying, and
selecting integer bounds are frequent enough that spelling the accumulator at
every call adds noise and invites inconsistent empty-source behavior. The
current type system does not yet have an accepted numeric protocol that proves
addition, multiplication, ordering, zero, and one for a generic element type.

Returning `Int?` for a minimum or maximum would not be ambiguous because the
element type is fixed to non-optional `Int`, but explicit-fallback inspection is
already the accepted first convention in ADR 0064. Reusing it keeps call sites
and empty-source behavior uniform.

## Decision

`Pop.Sequence` adds four eager portable prototypes:

```luau
public function sum<TSource: Iterable<Int>>(source: TSource): Int
public function product<TSource: Iterable<Int>>(source: TSource): Int
public function minOr<TSource: Iterable<Int>>(source: TSource, fallback: Int): Int
public function maxOr<TSource: Iterable<Int>>(source: TSource, fallback: Int): Int
```

`sum` starts at zero and adds every item in source order. `product` starts at
one and multiplies every item in source order. Empty sources therefore return
zero and one respectively. Both use ordinary checked `Int` arithmetic and trap
with the existing integer-overflow behavior rather than wrapping, widening, or
allocating a larger representation.

`minOr` and `maxOr` return the least or greatest source item. They return the
already evaluated fallback only when the source is empty; the fallback does not
participate in comparison for a nonempty source. Equal items preserve the
first selected value, though integer equality makes that identity
unobservable.

All four functions are eager, single-pass, O(n) time, O(1) algorithm space,
allocate no storage of their own, perform no dynamic/interface dispatch beyond
the source's accepted iterator contract, do not suspend, and cross no new
native/runtime boundary.

The element contract is deliberately `Int`. These functions do not authorize
numeric type inference, structural operator constraints, erased arithmetic, or
parallel/reordered reduction. Broader numeric aggregation waits for the exact
nominal numeric protocols and floating-point ordering/reproducibility policy.

The functions append prototype API-baseline identities, remain outside the
prelude, and are ordinary `.pop` bodies without compiler-known IDs or backend
operations.

## Consequences

- Common integer aggregation becomes one direct call.
- Empty-source results are deterministic and consistent with explicit-fallback
  inspection.
- Checked overflow and left-to-right evaluation stay backend-neutral.
- The API does not freeze a premature universal numeric abstraction.
- Floating-point sum/product and generic min/max remain unresolved rather than
  inheriting unsuitable integer semantics.

## Alternatives considered

### Require `fold` for every aggregate

Rejected because the repeated accumulator closures obscure frequent operations
and make standard empty identities less discoverable.

### Add one unconstrained generic implementation

Rejected because an unconstrained type parameter does not prove arithmetic,
ordering, or identity values. Runtime operator lookup is forbidden.

### Return optional minima and maxima

Deferred in favor of the accepted `firstOr`/`lastOr` fallback convention. A
later no-fallback family should use one coherent presence representation rather
than mixing optional and `Iteration<T>` results.

### Reorder or parallelize aggregation

Rejected because checked integer overflow makes evaluation order observable.
Parallel aggregation belongs to future `Task` APIs with an explicit contract.

## Required conformance tests

- empty, single-item, positive, negative, and mixed sources cover exact values;
- fallback values are used only for empty minimum/maximum queries;
- sum and product overflow trap through MIR interpreter and LLVM;
- traversal is left-to-right and single-pass;
- non-`Int` element sources fail statically;
- the API baseline appends exact prototype identities without changing the
  prelude;
- public XML documentation states allocation and complexity; and
- architecture tests reject Rust duplicates, compiler recognition, native
  adapters, or dynamic numeric fallback.

## Documents/components affected

Sequence source and documentation, API baseline, public catalog, examples,
foundation source tests, MIR/LLVM differential tests, architecture conformance,
closed decisions, and implementation roadmap.
