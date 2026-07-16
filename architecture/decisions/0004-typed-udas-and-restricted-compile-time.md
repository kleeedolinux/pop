# ADR 0004: Typed UDAs and Restricted Compile-Time Execution

- Status: accepted
- Date: 2026-07-10
- Amended by: ADR 0096 for the first-release retained codec projection,
  generated adapter protocol, and canonical `.popc` descriptor

## Context

Pop Lang needs user-defined declaration metadata and compile-time programming
without inheriting string mixins, source-text evaluation, unrestricted
reflection, non-reproducible builds, or a dynamic runtime representation.

## Decision

UDAs are typed immutable compile-time values attached to resolved declarations.
Compile-time functions run in a deterministic compiler interpreter over a
restricted typed HIR and capability/effect set.

The initial introspection API supports typed UDA and narrow symbol/type queries.
It has no source parser, token API, string-to-symbol conversion, global member or
type enumeration, FFI, ambient I/O, or backend access.

UDAs cannot change parsing/name binding or add declarations in the first
language version. They are not retained at runtime by default. Reflection-like
runtime use cases rely on explicit narrow metadata projections and generated
statically typed adapters. ADR 0096 authorizes only one fixed compiler-generated
sibling adapter Item for the closed `Metadata.Use.Codec` request; it does not
authorize arbitrary declaration generation.

## Consequences

- Compile-time results are cross-target, cacheable, and reproducible.
- Libraries can attach domain metadata without compiler-reserved annotations.
- The compiler needs a compile-time effect checker, interpreter, dependency
  tracker, budgets, canonical value format, and provenance-aware diagnostics.
- General procedural macros and arbitrary code generation are outside the first
  version.
- Runtime reflection cannot enumerate or manipulate arbitrary program values.

## Alternatives considered

### D-style string mixins or compile-time eval

Rejected because text injection is hard to type, cache, secure, diagnose, and
integrate with IDEs. It can also bypass normal phase and visibility reasoning.

### Run compile-time functions as host-native code

Rejected because it harms sandboxing, cross-compilation consistency,
determinism, and compiler robustness.

### Retain all attributes and type structure at runtime

Rejected because it increases artifact/runtime cost, leaks structure, prevents
dead stripping, and pressures the language toward dynamic values.
