# ADR 0111: Essential Integer Math Completion

- Status: accepted
- Date: 2026-07-26
- Depends on: ADRs 0032, 0040, 0051, 0062, 0065, 0074, 0110, and 0112
- Supersedes: the unresolved `clamp` invalid-bound and integer exponent policies
  in ADRs 0062 and 0065

## Context

The first `Pop.Math` prototype supplies direct checked `Int` minimum, maximum,
magnitude, greatest/least common divisor, sign, and coprimality. Downstream
numeric, layout, parser, time, and network libraries also need bounded
selection, efficient integer powers, and an explicit alternative to the
language's truncating division/remainder pair.

`clamp` was intentionally delayed because silently swapping invalid bounds
would hide caller input. Integer exponentiation was delayed because negative
exponents do not have an exact integer result. Neither case needs a dynamic
numeric protocol, native adapter, allocation, or new runtime error hierarchy.

## Decision

`Pop.Math` adds four ordinary non-generic `Int` functions:

```luau
public function clamp(value: Int, lower: Int, upper: Int): Int?
public function power(base: Int, exponent: Int): Int?
public function floorDivide(dividend: Int, divisor: Int): Int
public function floorRemainder(dividend: Int, divisor: Int): Int
```

### Clamping

`clamp` returns `nil` when `lower > upper`. Otherwise it returns `lower` when
`value < lower`, `upper` when `value > upper`, and the original value when it is
inside the inclusive range. Bounds are never silently swapped.

The optional result represents whether one valid closed interval exists; it
does not introduce a nominal option wrapper or an exception. The operation is
O(1), allocates no storage, and evaluates each argument once.

### Integer power

`power` returns `nil` for a negative exponent because the exact result is not
generally an integer. It defines `power(base, 0) == 1`, including `0^0`, and
uses exponentiation by squaring for positive exponents.

Multiplication retains ordinary checked `Int` semantics. An unrepresentable
intermediate or final result raises `IntegerOverflow`; the function never
wraps, saturates, widens, converts to floating point, or allocates a big
integer. It performs O(log exponent) checked multiplications and O(1) storage.
It does not square the factor after the final exponent bit, avoiding an
irrelevant overflow for cases such as `power(Int.max, 1)`.

### Floor division and remainder

`floorDivide` returns the mathematical quotient rounded toward negative
infinity. `floorRemainder` returns the corresponding remainder satisfying:

```text
dividend == floorDivide(dividend, divisor) * divisor
            + floorRemainder(dividend, divisor)
```

For a nonzero remainder, its sign matches the divisor. Both functions preserve
the existing `DivisionByZero` and signed minimum divided by negative-one trap
behavior before any adjustment. They are O(1), allocate nothing, and use only
ordinary checked `Int` operations.

The names use complete words. No `pow`, `div`, or `mod` aliases are added.

## Optimization and portability

All four bodies live in the conventionally discovered `Pop.Math` source Module.
They lower through ordinary typed HIR and canonical MIR. The compiler,
interpreter, LLVM backend, runtime, and native standard adapter inventories do
not recognize their source names specially.

The direct comparisons and floor adjustment are constant work. Exponentiation
by squaring replaces linear repeated multiplication. No performance timing is
stabilized by this ADR; checked operation counts and allocation absence are
deterministic cost contracts.

## Consequences

- Common range and integer-power work no longer needs repeated local policy.
- Invalid intervals and negative integer exponents are explicit optional
  outcomes rather than normalization, floating conversion, or panic.
- Floor arithmetic is available without changing the language's existing
  truncating `/` and `%` operators.
- Float transcendental, decimal, arbitrary-precision, rational, and complex
  contracts remain later `Math` work under ADR 0110.

## Alternatives considered

### Swap invalid clamp bounds

Rejected because it hides invalid input and contradicts the earlier accepted
boundary.

### Return a new nominal error for both optional outcomes

Rejected because each function has one absence condition and callers need no
payload to recover. `T?` already has exact static narrowing and fallback
semantics.

### Use linear exponentiation

Rejected because the asymptotic cost is avoidable and unsuitable for a
foundation implementation.

### Change `/` and `%` globally

Rejected because that would alter existing language semantics. Explicit Math
functions make the rounding policy visible.

## Required conformance tests

- `clamp` covers invalid, below, inside, boundary, and above cases without
  swapping bounds;
- `power` covers negative, zero, zero-base, positive, odd/even, maximum-base
  exponent one, and checked overflow cases;
- floor division/remainder cover every sign combination, exact division,
  divisor magnitude greater than dividend, zero division, and signed overflow;
- public documentation records optional/trap, allocation, and complexity
  behavior;
- the API baseline appends exact prototype signatures without widening the
  prelude;
- public reference metadata preserves ordinary source identities;
- MIR interpreter and LLVM execution agree on values, absence, and traps; and
- architecture tests reject a duplicate Rust module, bootstrap function,
  intrinsic, native adapter, or compiler/backend name recognition.

## Documents/components affected

Math source, checked documentation, standard API baseline, core catalog,
examples, closed decisions, foundation tests, interpreter/LLVM differential
tests, architecture tests, and roadmap.
