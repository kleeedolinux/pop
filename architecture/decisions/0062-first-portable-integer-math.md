# ADR 0062: First Portable Integer Math

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0032 and ADR 0040
- Supersedes: the Rust-only `Math` bootstrap prototype from ADR 0035

## Context

The public catalog places scalar numeric operations under `Pop.Math`, but the
repository only contains a Rust host prototype for checked addition and
minimum. Ordinary Pop integer operators already have checked, backend-neutral
semantics, so small integer algorithms do not need a native adapter or a
compiler-known function.

A broad generic numeric API is premature. Generic code cannot apply numeric
operators to an unconstrained `T`, and the first generic-bound syntax accepts
only one existing nominal interface. Ordinary source overloads and the complete
numeric protocol family are not yet stable public contracts.

## Decision

`Pop.Math` initially exposes four functions for the canonical `Int` type:

```luau
Math.min(left, right)
Math.max(left, right)
Math.abs(value)
Math.gcd(left, right)
```

Their exact signatures are:

```luau
public function min(left: Int, right: Int): Int
public function max(left: Int, right: Int): Int
public function abs(value: Int): Int
public function gcd(left: Int, right: Int): Int
```

The concise names are established mathematical terms already reserved by the
public catalog. They do not authorize arbitrary truncation elsewhere.

`min` and `max` return the selected input, including when both values are
equal. `abs` returns the nonnegative magnitude and uses ordinary checked
negation. It therefore raises the existing `IntegerOverflow` trap for the one
`Int` value whose positive magnitude is not representable.

`gcd` returns the nonnegative greatest common divisor using Euclid's algorithm.
`gcd(0, 0)` is zero. Negative inputs are converted through the same checked
`abs` contract, so an unrepresentable magnitude raises `IntegerOverflow`
instead of widening, wrapping, or allocating a larger integer.

All four functions are O(1) except `gcd`, which performs O(log(min(|left|,
|right|))) remainder steps. They allocate no managed storage, perform no
interface or dynamic dispatch, do not suspend, and cross no native/runtime
boundary. They are ordinary `.pop` functions specialized and optimized through
the normal HIR/MIR pipeline.

`clamp` waits for an accepted invalid-bound contract. Float, decimal, arbitrary
precision, rational, and complex operations wait for their exact value,
overload, NaN/order, allocation, and error contracts. This decision does not
make the initial `Int` signatures generic or infer a numeric protocol.

## Consequences

- Common integer math gains short direct calls backed by portable Pop source.
- Checked arithmetic remains the single semantic source for overflow.
- The conflicting Rust `Math` prototype can be removed.
- Numeric-family expansion cannot silently introduce erased generics or
  backend-defined overload behavior.
- `gcd` documents the unrepresentable-magnitude edge instead of allocating a
  hidden big integer.

## Alternatives considered

### Keep the Rust prototype as the public implementation

Rejected because no native capability is needed. It would duplicate checked
integer semantics and make another backend reproduce a Rust-defined contract.

### Add one generic numeric interface now

Rejected because arithmetic, ordering, zero, conversion, checked overflow, and
result types do not form one already accepted nominal interface. Designing it
only to reuse four small bodies would create a premature abstraction.

### Add every numeric type with distinct long function names

Rejected because names such as `minimumInt64` repeat type context, impair
discovery, and freeze an overload workaround into the API.

### Define `clamp` by silently swapping invalid bounds

Rejected because normalization would hide invalid caller input. A later
contract must choose a typed error, trap, or statically proven range without
guessing here.

## Required conformance tests

- `min` and `max` cover less, equal, greater, and negative inputs;
- `abs` covers zero, positive, negative, and checked minimum-value overflow;
- `gcd` covers zero, common factors, coprime values, negative inputs, symmetry,
  and unrepresentable magnitudes;
- non-`Int` calls fail statically without implicit conversion or dynamic
  fallback;
- public reference metadata preserves the exact signatures and ordinary Bubble
  identity without assigning bootstrap IDs;
- MIR interpreter and LLVM executions agree, including checked traps; and
- architecture tests reject a duplicate Rust module, bootstrap function ID,
  native adapter, or compiler/backend recognition of these names.

## Documents/components affected

Math source and tests, standard-library catalog and implementation inventory,
foundation source conformance, cross-backend execution tests, contributor
guidance, closed design decisions, and the roadmap.
