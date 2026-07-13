# ADR 0049: Nominal Scalar Enums

- Status: Accepted
- Date: 2026-07-13

## Context

Pop Lang reserves and names enum declarations and cases, but the bootstrap
compiler only indexes an `enum` declaration as an opaque type-space name.
Treating enums as integers would lose nominal safety; treating them as tagged
unions would collapse two language concepts with different representation and
payload contracts.

## Decision

The initial enum form is a closed nominal set of payload-free cases:

```luau
public enum Color
    Red
    Green
    Blue
end
```

Cases are referenced as `Color.Red`. Each case has a stable typed `EnumCaseId`
and the exact nominal `Color` type. Declaration order assigns a zero-based
`UInt32` runtime discriminant, but there is no implicit conversion between an
enum and an integer. Explicit representation conversions are deferred.

Enums support equality and inequality only when both operands have the exact
same enum type. Ordering and arithmetic are rejected. An enum remains distinct
from tagged unions, singleton integer types, and aliases in semantic types,
HIR, MIR declarations, and verification.

MIR carries a backend-neutral enum constant operation with the declaration and
case identities. LLVM may lower the verified value to `i32`; the MIR
interpreter retains the typed discriminant. No PLRI operation or runtime name
lookup is required. The experimental C backend remains fail-closed because
enum declarations are outside its deliberately narrow declaration-free subset.

## Consequences

- Enum values are compact while remaining nominal and statically checked.
- Adding, removing, or reordering cases changes the declaration contract.
- Payload-bearing alternatives continue to use tagged unions.
- User-assigned discriminants, flags enums, iteration, and integer conversion
  require later explicit decisions.

## Conformance requirements

Tests must cover parsing, stable case identities/order, exact nominal typing,
case resolution, equality/inequality, foreign/unknown cases, rejection of
arithmetic and cross-enum comparison, HIR/MIR verification and text, interpreter
and LLVM behavior, and fail-closed C handling.
