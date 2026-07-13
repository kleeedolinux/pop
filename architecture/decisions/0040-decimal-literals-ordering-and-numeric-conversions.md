# ADR 0040: Decimal Literals, Complete Ordering, and Numeric Conversions

- Status: accepted
- Date: 2026-07-13
- Supersedes: none

## Context

Pop Lang already defines fixed-width integer and IEEE floating-point types, but
the executable source slice accepts only digit-only numeric literals. An
expected float type can reinterpret such a literal, while ordinary decimal
spellings such as `1.5` do not parse. Numeric ordering also lacks `<=` and `>=`,
and source has no explicit operation for converting between numeric types.

Leaving conversion to ordinary function overloads would hide a compiler-known
semantic operation and make constant folding, trap behavior, and backend
equivalence difficult to verify. Importing an `as` operator would add a
Rust/C#-shaped spelling to a Luau-first language, while using Luau's `::` type
assertion spelling for value-changing conversion would give familiar
punctuation a different meaning.

## Decision

Decimal floating-point literals use ordinary decimal notation with an optional
fraction and base-ten exponent. A spelling containing a decimal point or
exponent is always floating-point. It has the expected `Float32` or `Float64`
type when one is available and otherwise defaults to `Float` (`Float64`). A
decimal floating-point literal never implicitly converts to an integer. Digit
separators remain `_` and must occur between digits.

`<=` and `>=` join `<` and `>` in the numeric ordering precedence group. Both
operands must have the same statically known numeric type. IEEE comparisons are
ordered: every ordering comparison with NaN is false.

An explicit numeric conversion uses target-type call syntax:

```luau
local ratio = Float64(count)
local narrowed = Int32(total)
```

The target must be a built-in fixed-width numeric type or the accepted `Int`,
`Float`, or `Byte` alias, and the call has exactly one numeric argument. This is
a compiler-known conversion expression, not source or runtime name lookup and
not an ordinary overload. A nearer value declaration cannot replace a numeric
type target.

Conversions have these portable semantics:

- integer to integer is checked and traps with `NumericConversion` when the
  value is outside the target range;
- integer to float uses IEEE round-to-nearest, ties-to-even in the target
  format;
- float to integer truncates toward zero and traps with `NumericConversion` for
  NaN, infinity, or a value outside the target range;
- `Float32` to `Float64` is exact, while `Float64` to `Float32` uses the target
  IEEE rounding behavior;
- converting a numeric value to its own canonical type preserves the value.

Typed analysis records the source and target numeric kinds. HIR retains a
backend-neutral numeric-conversion node, and MIR uses explicit conversion
operations. The MIR verifier checks operand/result kinds and the interpreter,
LLVM backend, and every other supporting backend implement the same semantics.
Constant conversion may be folded only when it preserves the same result or
trap.

## Consequences

- Ordinary floating-point source no longer needs digit-only literals plus an
  expected annotation.
- Numeric representation changes are visible and searchable at the call site.
- Checked narrowing has one deterministic failure instead of host-language
  casts or backend-specific behavior.
- Built-in numeric type names gain a narrow expression role without becoming
  runtime constructor values or a general type-reflection mechanism.
- Backends must support the conversion operations or reject them through an
  explicit capability boundary; they cannot substitute host cast behavior.

## Alternatives considered

### Add an `as` conversion operator

Rejected because it imports a Rust/C#-shaped surface form where a short,
type-directed Luau-shaped call is sufficient.

### Use `::` for numeric conversion

Rejected because Luau programmers read `::` as a type assertion. Numeric
conversion changes the runtime representation and may trap, so reusing that
spelling would be misleading.

### Provide only standard-library conversion functions

Rejected because fixed-width conversion semantics affect typed HIR, canonical
MIR, constant evaluation, traps, and backend equivalence. A library call would
hide rather than remove that compiler contract.

### Permit implicit numeric widening

Rejected for this slice. Even apparently safe widening changes overload and
inference behavior. Numeric literals remain context-sensitive, while conversions
of existing values stay explicit.

## Required conformance tests

- lexer/parser tests cover fractional and exponent literals, separators, and
  malformed spellings without consuming member-access punctuation;
- type tests cover default `Float64`, expected `Float32`, integer-target
  rejection, same-type operands, and nonnumeric conversion rejection;
- positive and negative tests cover `<=`/`>=`, including IEEE NaN behavior;
- every fixed-width source/target pair covers boundaries, truncation, rounding,
  NaN, infinity, and out-of-range conversion;
- HIR/MIR construction, verification, deterministic text, and round trips cover
  all four conversion families without dynamic or backend-specific operations;
- the MIR interpreter and LLVM backend agree for successful and trapping
  conversions, and the C experiment either agrees or rejects an explicitly
  unsupported capability before emission;
- architecture regressions continue to reject implicit numeric fallback,
  `Any`, `Dynamic`, and runtime type-name conversion.

## Documents/components affected

Language model, intermediate representations, closed design questions, type
system, syntax and nomenclature, runtime trap vocabulary, compiler front end,
typed bodies, HIR, MIR, optimizer, interpreter, LLVM backend, C backend,
diagnostics, and cross-backend conformance tests.
