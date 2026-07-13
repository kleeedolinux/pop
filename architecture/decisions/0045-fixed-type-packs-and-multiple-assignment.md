# ADR 0045: Fixed Type Packs and Multiple Assignment

- Status: accepted
- Date: 2026-07-13
- Depends on: ADR 0005, ADR 0013, and ADR 0019

## Context

Pop Lang promises Luau-like multiple returns and multiple assignment while
forbidding untyped variadics. The architecture fixes fully static type packs but
does not yet define fixed-pack source syntax, arity adjustment, assignment
ordering, or lowering. Copying Lua's implicit `nil` padding and discarded extra
values would weaken mistakes into accepted programs, while independently
lowering each assignment could expose earlier stores to later expressions.

## Decision

A parenthesized function result annotation denotes one fixed result pack:

```luau
private function divide(value: Int): (Int, Int)
    return value / 2, value % 2
end
```

The pack has an exact statically known arity and element type at every use.
`return first, second` is syntax for constructing that fixed pack. An explicit
tuple expression of the same type is also accepted as the single returned pack.
A result annotation without parentheses continues to denote one result, and an
omitted annotation continues to denote the empty result pack.

Multiple local declaration and assignment retain Luau's comma-shaped surface:

```luau
local quotient: Int, remainder: Int = divide(value)
left, right = right, left
```

Each binding may have its own optional annotation. The right-hand side is either
one fixed-pack expression with exactly as many elements as targets or an exact
comma-separated list of scalar expressions. Pop Lang does not pad missing
values with `nil`, discard extra values, or expand an arbitrary last expression.
Those arity mismatches are static errors.

Multiple-assignment targets use the ordinary assignment target rules. Mutable
locals and captures, mutable declared class fields, and mutable array elements
are valid; parameters, numeric `for` bindings, records, and other immutable
values remain invalid. Local declarations introduce fresh mutable lexical
bindings only after all initializers have been checked.

Evaluation and mutation have this exact order:

1. resolve and evaluate target locations from left to right, including each
   field receiver and array/index expression exactly once;
2. evaluate every right-hand-side expression from left to right;
3. check or project the exact fixed pack without runtime type discovery;
4. perform stores from left to right.

Thus `left, right = right, left` swaps values, duplicate targets are permitted
with the last store winning, and no store is visible to right-hand-side
evaluation. A failure while locating a target or evaluating a right-hand side
performs no later evaluation or store. A failure during a store preserves any
earlier completed stores, matching ordinary source-order effects.

Typed analysis and HIR retain fixed-pack construction/projection and grouped
assignment targets long enough to preserve evaluation order and single target
evaluation. Canonical MIR represents the pack as one typed tuple-like value,
uses `tupleMake` and statically indexed `tupleGet`, and emits ordinary typed
stores and barriers. No backend reconstructs comma adjustment or performs
runtime arity/type lookup.

The restricted compile-time evaluator supports fixed-pack construction,
returns, direct calls, and multiple local bindings through the same exact tuple
types. Multiple assignment remains mutation and is rejected at compile time.

Variadic type-pack tails and generic type-pack parameters remain fully static
architectural requirements but are not introduced by this ADR. Their surface
syntax, inference, ABI, and metadata encoding require a later accepted ADR.

## Consequences

- Common swaps and multi-result calls remain concise and Luau-shaped.
- Every arity and projected element type is known before HIR construction.
- Fixed packs use the existing tuple representation without collapsing tuples
  into tables or introducing a dynamic variadic carrier.
- MIR interpreter, LLVM, and future VM backends consume the same explicit tuple
  construction, projection, and store sequence.

## Alternatives considered

### Copy Lua value adjustment

Rejected because implicit `nil` padding and silently discarded values hide
statically detectable arity errors and complicate exact type-pack reasoning.

### Lower each target and value as an ordinary assignment immediately

Rejected because earlier writes would become visible to later right-hand-side
expressions and effectful field/array targets could be evaluated more than once.

### Add a dynamic variadic runtime object

Rejected as a release-blocking Lua regression. Fixed packs have exact types and
canonical tuple operations; future variadic tails must also remain typed.

## Required conformance tests

- parser acceptance of fixed result packs, comma returns, annotated multiple
  locals, and multiple assignment;
- positive typing for heterogeneous packs, swaps, captures, fields, and arrays;
- rejection of missing/extra values, mismatched element types, invalid targets,
  and pack use where one scalar is required;
- exact left-to-right target/value evaluation, once-only field/array target
  evaluation, swap behavior, and duplicate-target store order;
- HIR preservation and MIR `tupleMake`/`tupleGet` construction, verification,
  deterministic text, and round trips;
- construction/optimized MIR interpreter and LLVM differential behavior;
- permanent absence of dynamic variadic values, runtime arity lookup, and
  backend-specific HIR/MIR.

## Documents/components affected

Language model, compiler pipeline, intermediate representations, roadmap,
closed decisions, Luau relationship, type system, syntax/nomenclature,
conformance matrix, parser, type checker, capture analysis, HIR, MIR,
compile-time restrictions, MIR interpreter, LLVM, and C capability reporting.
