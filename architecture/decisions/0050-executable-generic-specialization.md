# ADR 0050: Executable Generic Specialization

- Status: accepted
- Date: 2026-07-13
- Supersedes: none

## Context

Pop Lang already parses function type parameters and Luau-direction explicit
generic calls, while the accepted architecture requires HIR to retain semantic
generic arguments and assigns specialization or typed sharing to MIR lowering.
The bootstrap compiler nevertheless treated user generic calls as unknown
names. Record and tagged-union declarations also lacked type parameters.

Executable generics need a closed initial contract that cannot fall back to
dynamic values, erased unchecked calls, universal tables, or backend-specific
HIR. The first implementation also needs to remain small enough to validate on
the MIR interpreter and native backend before adding typed code sharing.

## Decision

Functions, records, and tagged unions may declare an ordered list of invariant
type parameters with Luau-shaped syntax:

```luau
private function identity<T>(value: T): T
    return value
end

private record Box<T>
    value: T
end

private union Choice<T>
    Value(value: T)
    Empty
end
```

Generic function and union-case calls use the existing explicit generic-call
direction, for example `identity<<Int>>(1)` and
`Choice.Value<<String>>("ready")`. Generic record values use ordinary expected
type context, for example `local value: Box<Int> = { value = 1 }`.

Every call supplies exactly the declared number of type arguments. Each
argument resolves to a compiler-proven semantic type and substitutes the
corresponding type parameter through parameters, results, local annotations,
aggregate fields, union payloads, and nested generic calls. The bootstrap does
not infer omitted type arguments.

HIR records the source generic identity and ordered semantic type arguments.
Canonical MIR initially fully specializes every reachable concrete generic
function and concrete generic record/union representation. Equivalent
instantiations are deduplicated deterministically. MIR contains only canonical
concrete types and direct typed calls; no type parameter, dynamic dictionary,
runtime type argument, or string lookup reaches a backend.

Generic definitions that are never instantiated do not require an executable
MIR body or runtime representation. Generic cross-Bubble reference metadata and
typed dictionary sharing remain deferred until their serialization and ABI
contracts are accepted. The experimental C backend continues to reject generic
data and any specialized MIR outside its accepted scalar subset.

## Consequences

- Generic algorithms and generic immutable data become executable without a
  dynamic escape hatch.
- Full specialization gives the LLVM backend concrete layouts and direct calls
  suitable for ordinary optimization.
- Code-size sharing remains a later MIR optimization and cannot change source
  semantics.
- Type-argument inference and portable generic references are not implied by
  this initial contract.

## Required conformance tests

- Syntax tests cover ordered type parameters on functions, records, and unions.
- Positive tests cover nested generic calls, generic records, generic union
  construction and matching, and repeated-instantiation deduplication.
- Negative tests cover missing, extra, unknown, and mismatched type arguments.
- HIR tests prove that semantic generic arguments are retained.
- MIR tests prove that executable signatures, data layouts, and calls contain
  only concrete types.
- MIR-interpreter and LLVM differential tests execute the same specialized
  program. The C backend must reject unsupported generic data deterministically.

## Documents/components affected

- `architecture/04-intermediate-representations.md`
- `architecture/07-implementation-roadmap.md`
- `architecture/08-open-design-questions.md`
- `architecture/08.1-closed-design-questions.md`
- `architecture/12-type-system-architecture.md`
- `architecture/13-syntax-and-nomenclature.md`
- syntax, resolution, typed bodies, HIR, MIR, interpreter, LLVM, and C
  capability validation
- architecture conformance and backend differential tests
