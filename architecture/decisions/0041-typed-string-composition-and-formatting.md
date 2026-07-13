# ADR 0041: Typed String Composition and Formatting

- Status: accepted
- Date: 2026-07-13
- Depends on: ADR 0001, ADR 0003, ADR 0005, ADR 0022, ADR 0030, and ADR 0032

## Context

Pop Lang already has immutable UTF-8 `String` values and typed integer/string
output overloads, but source programs cannot yet compose text, express escaped
characters, interpolate values, or explicitly format primitive values. The
Luau relationship requires concatenation and backtick interpolation to retain
their familiar surface. The standard-library architecture also reserves
`format` for conversion to text while rejecting a universal `Object`, automatic
`toString`, runtime reflection, and hidden dynamic dispatch.

Leaving the accepted surface at the level of “preserve interpolation” is not
precise enough to type-check, lower, optimize, or compare across backends. In
particular, interpolation must not become an operational dynamic escape hatch,
and native formatting must not silently use target locale or backend-specific
spellings.

## Decision

String composition has the following closed source contract:

- `left .. right` concatenates exactly two `String` operands and produces an
  owned immutable UTF-8 `String`;
- backtick strings use Luau-shaped `{expression}` interpolation and evaluate
  segments from left to right exactly once;
- interpolation accepts only `String`, `Boolean`, and the fixed-width integer
  and floating-point primitives;
- `String(value)` is the explicit compiler-known formatting form for that same
  closed set, with `String(String)` as identity;
- these forms are resolved from static types and never call a member by name,
  enumerate a runtime type, or fall back to a universal formatting operation.

Ordinary single- and double-quoted literals accept the portable escapes `\\`,
`\"`, `\'`, `\n`, `\r`, `\t`, `\0`, two-digit `\xHH`, and Unicode scalar
`\u{H...}` with one through six hexadecimal digits. Backtick strings accept
the same escapes plus `\`` and `\{`/`\}` for literal interpolation
punctuation. Source newlines are not permitted inside any initial string form.
Unknown, truncated, non-scalar, surrogate, and out-of-range escapes are syntax
errors; they are never copied through literally as recovery semantics.

Primitive formatting is locale-independent and deterministic:

- booleans are `true` and `false`;
- integers use base ten with a leading minus only for negative values and no
  grouping;
- finite floats use the shortest decimal spelling that round-trips to the same
  format, preserve a negative-zero sign, use a lowercase `e` when scientific
  notation is selected, and spell the non-finite values `nan`, `inf`, and
  `-inf`.

No initial interpolation format specifier or ambient locale is accepted.
Locale-sensitive and application-specific formatting remains an explicit
typed library operation. Adding another interpolatable type requires an
accepted closed static formatting protocol or a new typed compiler/library
contract; it cannot widen this decision to `Any`, `Dynamic`, `Object`, or
runtime inspection.

HIR preserves typed string concatenation, primitive formatting, and ordered
interpolation composition. Canonical MIR uses backend-neutral `StringConcat`
and `StringFormat` operations whose input kind is verified from the operand
type. These operations may allocate and therefore carry allocation and safe-
point effects. Backends may fold literal composition or use private helpers,
but must produce the same UTF-8 bytes. Empty-string identity, adjacent literal
folding, and constant primitive formatting are valid optimizations when they do
not remove evaluation or observable failure effects.

The runtime owns UTF-8 allocation and the native helpers needed by LLVM. The C
backend may use a backend-private owned-string representation and helpers, but
its output must match canonical MIR. Neither backend exposes native symbols or
storage layout in HIR/MIR or source resolution.

The closed `StringConcat` and `StringFormat` PLRI operations advance the
bootstrap native ABI from version 1.4 to 1.5. The format helper receives a
compiler-selected closed primitive tag and raw primitive bits; this is ABI
marshalling of an already verified static kind, not runtime type discovery.

## Consequences

- Common composition remains concise and Luau-shaped.
- Interpolation is convenient without introducing implicit conversion in
  ordinary calls or operators.
- Formatting allocation is visible in typed IR effects and is consistent
  across the MIR interpreter, LLVM, and supported C slice.
- User-defined formatting and locale policy remain future typed library or
  protocol work rather than an accidental runtime convention.
- Literal decoding becomes one shared compiler contract instead of separate
  ad-hoc unquoting in attributes, compile time, HIR/MIR, and backends.

## Alternatives considered

### Add a universal `toString` method

Rejected because Pop Lang has no universal root object or automatic inherited
methods, and runtime dispatch would violate strong static typing.

### Accept every interpolation value and inspect it at runtime

Rejected because this is `Dynamic` under formatting syntax and would make
backend-neutral verification impossible.

### Use `+` for concatenation

Rejected because `..` is the natural Luau operator and keeps numeric addition
unambiguous.

### Delegate primitive formatting to the ambient C or host locale

Rejected because results would depend on process state and differ across
backends and targets.

## Required conformance tests

- positive and negative lexer tests cover every accepted escape, UTF-8 scalar,
  malformed escape, unterminated literal, and unescaped interpolation brace;
- parser and type tests cover `..`, empty/literal composition, nested
  interpolation expressions, static segment types, left-to-right single
  evaluation, and rejection of unsupported values;
- explicit `String(value)` tests cover every primitive kind and reject records,
  arrays, tables, classes, functions, and wrong arity;
- HIR/MIR dumps and verifiers retain typed backend-neutral operations with
  allocation/safe-point effects and reject kind/type mismatches;
- optimizer tests prove literal folding, empty identity, and preservation of
  effectful operands;
- MIR-interpreter, LLVM/native, and supported C tests agree for ASCII,
  non-ASCII, escapes, integer boundaries, negative zero, finite float boundary
  cases, NaN, and infinities;
- architecture regressions reject universal formatting, runtime type-name
  lookup, dynamic fallback operations, JavaScript template syntax, and
  backend-specific HIR/MIR.

## Documents/components affected

Language model, Luau relationship, syntax/nomenclature, type checking,
compile-time constants, HIR, MIR, effect verification, optimizer, MIR
interpreter, LLVM and C backends, PLRI/native runtime, standard-library text
catalog, closed decisions, diagnostics, and architecture regression tests.
