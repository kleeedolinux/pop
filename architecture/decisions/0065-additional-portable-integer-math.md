# ADR 0065: Additional Portable Integer Math

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0032, ADR 0040, and ADR 0062
- Supersedes: none

## Context

ADR 0062 establishes the first `Int`-specific portable Math surface without
pretending that a generic numeric protocol already exists. Sign, least common
multiple, and coprimality are useful integer operations that can reuse those
checked semantics without new types, errors, allocation, or runtime support.

The overflow order matters. A least-common-multiple implementation that
multiplies before division can trap even when the mathematical result is
representable, while implicit widening would contradict fixed-width `Int`.

## Decision

`Pop.Math` adds:

```luau
public function sign(value: Int): Int
public function lcm(left: Int, right: Int): Int
public function coprime(left: Int, right: Int): Boolean
```

`sign` returns -1, 0, or 1 according to the value's mathematical sign.

`lcm` returns the nonnegative least common multiple. If either argument is zero,
it returns zero without taking either magnitude; therefore `lcm(Int.min, 0)` is
zero. Otherwise it computes checked magnitudes and performs division by `gcd`
before checked multiplication. An unrepresentable magnitude or result raises
the existing `IntegerOverflow` trap. It never widens, wraps, saturates, or
allocates a larger integer.

`coprime` returns whether `gcd(left, right) == 1`. Consequently `(0, 1)` and
`(1, 0)` are coprime while `(0, 0)` is not. It preserves the checked magnitude
behavior of `gcd`.

`sign` is O(1). `lcm` and `coprime` are O(log n) through `gcd`. All three use
O(1) storage, allocate nothing, perform no dynamic/interface dispatch, do not
suspend, and cross no native/runtime boundary. They are ordinary `.pop`
functions.

## Consequences

- More common number-theory work uses short typed calls.
- Division-before-multiplication avoids avoidable intermediate overflow without
  weakening final checked overflow.
- Zero has an explicit LCM contract even beside the unrepresentable magnitude
  of `Int.min`.
- Broader numeric types still wait for their own overload, representation, and
  cost decisions.

## Alternatives considered

### Multiply before dividing in `lcm`

Rejected because the intermediate product can overflow when the final result
would fit.

### Widen internally or return a big integer

Rejected because that changes the result type and hides allocation. A later
arbitrary-precision family will have explicit value and cost contracts.

### Return a Result for overflow

Rejected for this slice because ordinary `Int` arithmetic already has the
accepted checked trap contract. These functions compose the same operations
rather than creating a second overflow channel.

### Add `clamp` and exponentiation now

Rejected because invalid clamp bounds and negative/inexact exponent policy are
still separate public contracts. Placement in Math does not answer them.

## Required conformance tests

- `sign` covers negative, zero, and positive values including `Int.min`;
- `lcm` covers zero, signs, common factors, coprime values, symmetry, avoidable
  intermediate overflow, and unrepresentable results;
- `coprime` covers zero, signs, common factors, and coprime inputs;
- non-Int calls fail statically without conversion or fallback;
- MIR interpreter and LLVM agree on values and traps; and
- architecture tests reject duplicate Rust/bootstrap/compiler/backend
  implementations.

## Documents/components affected

Math source and checked documentation, public catalog and examples, foundation
conformance, cross-backend execution tests, closed design decisions, and the
implementation roadmap.
