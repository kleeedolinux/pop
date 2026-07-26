# ADR 0112: Explicit Optional Return Injection

- Status: accepted
- Date: 2026-07-26
- Depends on: ADRs 0001, 0003, and 0051
- Supersedes: none

## Context

The accepted type architecture makes a type a subtype of a union containing
that type and names optional injection as a distinct HIR conversion. The
compiler implemented optional lookup, presence testing, extraction, defaulting,
and propagation, but ordinary `T` and `nil` return expressions were still
required to equal `T?` exactly.

That gap prevented a source function from constructing the present or absent
result of its declared optional return without allocating a collection or
calling an unrelated runtime operation. Library APIs such as checked clamping
and exact integer powers require direct optional construction.

## Decision

When a function declares one `T?` result, a return expression of exact type `T`
is implicitly injected as a present optional. A return expression of exact type
`nil` constructs an absent optional. Other unrelated types remain rejected.

Typed runtime HIR records the present conversion as
`OptionalInject{value, optionalType}`. Absence remains the ordinary `Nil`
expression carrying the expected optional result type. Canonical MIR lowers
the present conversion to:

```text
optionalMake value -> T?
```

`optionalMake` is backend-neutral, has no allocation or effects, evaluates its
operand once, and preserves present zero and `false` values. The MIR verifier
requires the operand type to equal the one non-`nil` member of the result
optional. `NilConstant` with an optional result type remains the absent form.

The MIR interpreter represents a present optional with its existing typed
visible value and absence with `nil`. LLVM uses its existing private presence
bit plus payload representation. The experimental runtime-free C backend may
continue to reject optional result types deterministically.

This first implementation applies injection at return boundaries. Additional
implicit conversion sites require their own tests and must preserve evaluation,
ownership, lifetime, and overload-selection rules rather than reusing return
logic silently.

## Consequences

- Source functions can return both branches of `T?` directly.
- HIR and MIR preserve the conversion explicitly; backends never infer it from
  result types or source syntax.
- Optional construction remains allocation-free and distinguishes present
  zero/`false` from absence.
- The change does not introduce an `Option` wrapper, unchecked extraction,
  dynamic union operation, or runtime type lookup.

## Alternatives considered

### Require a standard-library constructor

Rejected because optionality is a language union and its representation is
already compiler-governed. A library call would disguise a type conversion as
runtime dispatch.

### Allocate a one-element collection and use optional lookup

Rejected because it changes allocation, effects, and complexity solely to work
around a missing type conversion.

### Reuse a generic dynamic union box

Rejected because Pop Lang has no operational dynamic value and optional layout
must remain statically known.

## Required conformance tests

- type checking accepts exact `T` and `nil` returns into `T?` and rejects
  unrelated values;
- typed HIR contains an explicit present injection and a typed absent value;
- canonical MIR contains `optionalMake`, round-trips through text, and rejects
  a payload whose type differs from the optional inner type;
- MIR interpreter and LLVM execution distinguish present zero from absence;
- optional construction performs no allocation and gains no effect; and
- architecture scans keep the HIR/MIR names and documentation synchronized.

## Documents/components affected

Type checking, typed bodies, HIR, canonical MIR, MIR verification and text,
compile-time rejection, MIR interpreter, LLVM backend, C capability behavior,
type/IR architecture, closed decisions, and conformance tests.
