# ADR 0012: Compact Standard Prelude and Contextual Names

- Status: accepted
- Date: 2026-07-10

## Context

The first BCL-inspired catalog copied too many .NET-style namespaces and long
type names. Requiring many `using Pop...` directives would make ordinary Pop
Lang noisier than Lua/Luau and repeat context in names.

## Decision

Normal projects automatically reference `Pop.Standard` and receive exactly one
fixed curated prelude from trusted `@Prelude` declarations. It exposes
high-frequency types/functions plus selected child namespace names. Child
members stay qualified; the entire root namespace is not blindly imported.

Common code writes `Json.encode`, `Io.open`, `Math.min`, `Text.Builder`, and
`Http.Client` without imports. Core collections use `List`, `Table`, `Set`,
`Iterable`, and `Sequence`; async uses `Task` and `CancelToken`.

Namespaces provide context, so APIs avoid redundant names such as `JsonValue`,
`StringBuilder`, `HttpClient`, `Dictionary`, `HashSet`, and
`CancellationToken`. External libraries still require explicit `using`, and
projects cannot configure arbitrary global usings. Unsafe `Pop.Interop` stays
explicit.

## Consequences

- Ordinary standard-library code needs zero imports.
- The prelude is a compatibility surface and must remain small/stable.
- Namespace qualification prevents child APIs from flooding file scope.
- Some familiar .NET BCL names intentionally change.
- Documentation and autocomplete group short names under contextual namespaces.

## Alternatives considered

### Require explicit using for every standard namespace

Rejected because it creates repetitive headers and weakens Pop Lang's lightweight
character.

### Import all standard members globally

Rejected because it causes collisions, harms discovery, and makes the prelude
unbounded.

### Copy .NET names exactly

Rejected because Pop Lang adopts BCL design lessons, not its OOP/history-driven
nomenclature.
