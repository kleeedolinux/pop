# ADR 0007: C#-Style Namespace and Library Separation

- Status: accepted
- Date: 2026-07-10
- Amended by: ADR 0017

## Context

Pop Lang needs clear terminology and mechanics for source files, namespaces,
packages, compiled libraries, build references, runtime loading, and future
plugins. A Luau `require`-returns-table model does not provide these boundaries.

## Decision

Files declare a file-scoped `PascalCase` namespace. Semicolon-free `using`
directives shorten compile-time names and never load/execute code.

Normal builds automatically reference `Pop.Standard` and expose only its trusted
curated `@Prelude` declarations. Child namespace members remain qualified;
external libraries still require explicit `using`.

Package manifests reference versioned library Bubbles. Self-describing `.poplib`
artifacts carry Bubble identity, dependency, platform-target, ABI,
public-reference metadata, and implementation artifacts. The runtime default
`BubbleContext` maps each resolved identity to one loaded implementation;
isolated contexts are a future plugin capability. ADR 0017 defines the complete
Item/Module/Bubble/Package/Workspace and `pop` CLI model.

Reference metadata is loadable by the compiler without implementation code and
without runtime reflection.

## Consequences

- Name organization is independent of file and binary layout.
- Dependency resolution/loading errors are distinct from namespace errors.
- Build/reference operations do not execute dependency code.
- Native and VM artifacts share logical identity and manifest rules.
- The toolchain must define manifests, locks, metadata verification, and ABI
  compatibility.

## Alternatives considered

### Luau-style `require` returning module values

Rejected because it turns namespace/API structure and initialization into
runtime value behavior.

### Infer dependencies from `using`

Rejected because source name convenience should not perform version resolution,
downloads, or runtime probing.
