# ADR 0011: Procedural, Functional, and Data-Oriented by Default

- Status: accepted
- Date: 2026-07-10
- Related: ADR 0030 (native public standard-library architecture)

## Context

Pop Lang needs native classes to replace Lua/Luau table conventions, but native
classes could accidentally turn the language and public library into an OOP-first ecosystem
with deep hierarchies, static utility classes, method chains, and universal
object assumptions.

## Decision

Pop Lang is procedural, functional, and data-oriented by default. The preferred
abstraction order is functions, records/unions, collections/algorithms,
namespaces/modules, composition, small interfaces, then classes/inheritance only
when identity, mutable lifecycle, or real runtime dispatch requires them.

Records are immutable values by default and support typed `with` updates. Tagged
unions model alternatives. Namespace functions and type-companion functions are
first-class public APIs. There is no universal `Object` or requirement that
functions live in classes.

Classes are sealed by default. Method syntax remains available but is not the
standard organization mechanism. Public API analyzers can warn about OOP-heavy
shapes with reviewable fixes.

## Consequences

- The standard library emphasizes functions, data, small protocols, and generic
  algorithms.
- Classes remain powerful but require an architectural reason.
- Records/unions and exhaustive matching become important early milestones.
- Optimizers should eliminate value-update copies where safe.
- Documentation/examples lead with non-class designs.

## Alternatives considered

### Make Pop Lang class-oriented like C#/Java

Rejected because it conflicts with Luau's lightweight feel, introduces ceremony,
and encourages hierarchy/dispatch where data/functions are clearer.

### Remove classes entirely

Rejected because identity, encapsulated mutable state, native/foreign resources,
and runtime polymorphism benefit from a real nominal class model.
