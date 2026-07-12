# ADR 0012: Compact Standard Prelude and Contextual Names

- Status: accepted
- Date: 2026-07-10
- Superseded in part by: ADR 0030 and ADR 0031 (public catalog and names)

## Context

The first public catalog copied too many foreign-style namespaces and long type
names. Requiring many `using Pop...` directives would make ordinary Pop Lang
noisier than Lua/Luau and repeat context in names.

## Decision

> **Supersession note:** The fixed, small prelude decision remains accepted.
> ADR 0030 and ADR 0031 replace this record's illustrative public catalog and
> naming examples with the native tiered public-library contract.

Normal projects automatically reference `Pop.Standard` and receive exactly one
fixed curated prelude from trusted `@Prelude` declarations. It exposes
high-frequency types/functions plus selected child namespace names. Child
members stay qualified; the entire root namespace is not blindly imported.

Common code writes selected prelude-qualified operations without imports. The
exact prelude and contextual namespace names are defined by ADR 0031. Core
collections use `List`, `Table`, `Set`, `Iterable`, and `Sequence`; async uses
`Task` and `CancelToken`.

Namespaces provide context, so APIs avoid redundant names such as `JsonValue`,
`StringBuilder`, `HttpClient`, `Dictionary`, `HashSet`, and
`CancellationToken`. External libraries still require explicit `using`, and
projects cannot configure arbitrary global usings. Unsafe `Pop.Interop` stays
explicit.

## Consequences

- Ordinary standard-library code needs zero imports.
- The prelude is a compatibility surface and must remain small/stable.
- Namespace qualification prevents child APIs from flooding file scope.
- Public names follow the native naming contract rather than foreign conventions.
- Documentation and autocomplete group short names under contextual namespaces.

## Alternatives considered

### Require explicit using for every standard namespace

Rejected because it creates repetitive headers and weakens Pop Lang's lightweight
character.

### Import all standard members globally

Rejected because it causes collisions, harms discovery, and makes the prelude
unbounded.

### Copy .NET names exactly

Rejected because Pop Lang uses its own naming contract rather than foreign
OOP/history-driven nomenclature.
