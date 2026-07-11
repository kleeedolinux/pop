# ADR 0003: Strong Static Typing Without Dynamic Values

- Status: accepted
- Date: 2026-07-10

## Context

The first architecture draft described Pop Lang as gradually typed and retained
dynamic values at explicit boundaries. That conflicts with the intended
language: Luau-inspired syntax and inference, but strong static guarantees and
predictable native/VM compilation.

## Decision

Every valid Pop Lang runtime value and operation has a compile-time-proven type.
The language has no operational `dynamic`, `any`, or equivalent escape type.
Inference is a way to discover a static type, not a mode that disables checking.

Heterogeneous values use explicit unions, interfaces, tuples, or typed schema
trees. External data is decoded into declared types. FFI and unsafe code remain
statically typed even when their correctness obligations are manual.

HIR and MIR contain no dynamic member lookup, dynamic calls, dynamic boxes, or
conversion from arbitrary runtime strings to program symbols.

## Consequences

- All backends receive complete operand, result, and dispatch types.
- Luau programs relying on `any`, changing table shapes, or runtime member names
  require migration.
- Serialization and foreign-data APIs need schemas or tagged typed data models.
- Runtime reflection cannot rely on an untyped universal value.
- Compiler diagnostics must explain failed inference instead of falling back.

## Alternatives considered

### Gradual typing with explicit dynamic boundaries

Rejected because dynamic values can spread, require runtime lookup/checking
machinery, and weaken the intended language guarantee.

### An unsafe dynamic type

Rejected. Unsafe operations may relax memory-safety proof obligations, but they
do not disable static type checking or introduce dynamic dispatch.

